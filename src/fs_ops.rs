use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
pub enum PasteMode {
    Copy,
    Move,
}

/// Write via temp-file + rename so a crash mid-write can never leave a
/// truncated file. Used for state.toml/shares.toml — a truncated file
/// there parses as "empty" and silently loses bookmarks/shares.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?; // data hits disk BEFORE the rename can be journaled
    }
    fs::rename(&tmp, path)?;
    // Persist the rename itself; ignore failure (some fs don't support dir fsync).
    if let Some(parent) = path.parent() {
        if let Ok(d) = fs::File::open(parent) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Decide directory vs file from `path`:
/// - trailing `/` → directory (explicit, always wins)
/// - no extension (e.g. `notes`, `Makefile`) → directory
/// - has extension (e.g. `foo.txt`, `script.sh`) → file
///
/// Extensionless filenames (Makefile, README) are intentionally treated
/// as directories per user preference; use a trailing slash to make
/// the dir form explicit either way.
pub fn create(path: &Path) -> io::Result<()> {
    let s = path.as_os_str().to_string_lossy();
    let trailing_sep = s.ends_with(std::path::MAIN_SEPARATOR);
    let has_ext = path.extension().is_some();
    let is_dir = trailing_sep || !has_ext;
    if is_dir {
        fs::create_dir_all(path)
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            fs::File::create(path)?;
        }
        Ok(())
    }
}

/// Non-colliding destination: `name.ext` -> `name (1).ext`, `(2)`, ...
pub fn unique_dest(dir: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let name = Path::new(file_name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = name.extension().map(|e| e.to_string_lossy().to_string());
    for i in 1.. {
        let fname = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let c = dir.join(fname);
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

/// Like [`unique_dest`] but also avoids any path already in `claimed`,
/// so a batch of pastes sharing a stem don't resolve to the same name.
/// Inserts the chosen path into `claimed` before returning.
pub fn claim_dest(
    dir: &Path,
    file_name: &std::ffi::OsStr,
    claimed: &mut std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let taken = |p: &Path| p.exists() || claimed.contains(p);
    let direct = dir.join(file_name);
    let chosen = if !taken(&direct) {
        direct
    } else {
        let name = Path::new(file_name);
        let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = name.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut i = 1u64;
        loop {
            let cand = dir.join(format!("{stem} ({i}){ext}"));
            if !taken(&cand) {
                break cand;
            }
            i += 1;
        }
    };
    claimed.insert(chosen.clone());
    chosen
}

/// True when `dst_dir` is `src` itself or a descendant of it — pasting
/// there would move/copy a directory into itself. `starts_with` on a
/// component basis already covers the equality case.
pub fn paste_would_recurse(src: &Path, dst_dir: &Path) -> bool {
    dst_dir.starts_with(src)
}

pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    if new_name.is_empty()
        || new_name == "."
        || new_name == ".."
        || new_name.contains('/')
        || new_name.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name must be a single path component",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let np = parent.join(new_name);
    if np.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file with that name already exists",
        ));
    }
    fs::rename(path, &np)?;
    Ok(np)
}

/// Cross-filesystem single-file move: copy then remove the source. If the
/// source removal fails after a successful copy, delete the freshly-written
/// destination so a reported failure never means a silent duplicate.
pub fn move_file_cross_fs(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::copy(src, dst).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::remove_file(src) {
        let _ = std::fs::remove_file(dst);
        return Err(format!("move failed (source kept): {e}"));
    }
    Ok(())
}

/// Cancel flag + progress callback shared by [`copy_file_chunked`] and
/// [`copy_dir_recursive`]. `progress` is a delta (bytes copied since the
/// last call), not a running total — callers accumulate.
pub struct CopyCtl<'a> {
    pub cancel: &'a AtomicBool,
    pub progress: &'a dyn Fn(u64),
}

/// Chunked copy: 1 MiB reads, cancel checked per chunk, progress reported
/// per chunk. Preserves permissions best-effort. On cancel the partial
/// destination file is removed and `Err` with `ErrorKind::Interrupted` is
/// returned — never leave a silent truncated file behind.
pub fn copy_file_chunked(src: &Path, dst: &Path, ctl: &CopyCtl) -> io::Result<()> {
    use std::io::{Read, Write};
    let mut r = fs::File::open(src)?;
    let mut w = fs::File::create(dst)?;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            drop(w);
            let _ = fs::remove_file(dst); // never leave a silent truncated file
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        (ctl.progress)(n as u64);
    }
    if let Ok(meta) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, meta.permissions());
    }
    Ok(())
}

/// Recursive dir copy. Symlinks are recreated as symlinks and never
/// followed — broken and cyclic links copy fine as links, with no
/// recursion and no error. Dirs are created first; permissions are
/// preserved best-effort on both dirs and files.
pub fn copy_dir_recursive(src: &Path, dst: &Path, ctl: &CopyCtl) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)?; // never follow — broken/cyclic safe
        return Ok(());
    }
    if meta.is_file() {
        return copy_file_chunked(src, dst, ctl);
    }
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        if ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        copy_dir_recursive(&e.path(), &dst.join(e.file_name()), ctl)?;
    }
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, m.permissions());
    }
    Ok(())
}

/// Recoverable delete via system trash.
pub fn trash_path(path: &Path) -> anyhow::Result<()> {
    trash::delete(path)?;
    Ok(())
}

/// Irreversible. Caller MUST gate with a confirmed prompt (see keymap Confirm mode).
///
/// Uses `symlink_metadata` (does NOT follow symlinks): only a *real* directory
/// is recursively removed; a symlink (even one pointing at a directory) is
/// unlinked, not recursed into. This matches the user expectation that X
/// removes the visible entry, never the target of a link.
pub fn permanent_delete(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        // Regular file OR symlink: unlink only.
        fs::remove_file(path)
    }
}

/// Known archive extensions. Multi-part compounds (`tar.gz`, `tar.xz`,
/// `tar.bz2`, `tar.zst`) are detected by checking the full lowercase
/// name suffix; single extensions match the path's `extension()`.
pub fn is_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    const MULTI: &[&str] = &[".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz"];
    if MULTI.iter().any(|m| name.ends_with(m)) {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "zip" | "tar" | "tgz" | "tbz2" | "txz" | "tzst" | "7z" | "rar" | "gz" | "bz2" | "xz" | "zst"
    )
}

/// Pick a destination directory for the extracted contents. Strips
/// the archive extension(s) from the file name; if a path with that
/// name already exists, falls back to the unique-dest scheme.
pub fn extract_dest_for(archive: &Path) -> PathBuf {
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    let stem = archive
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| {
            let lower = s.to_lowercase();
            for m in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz"] {
                if let Some(idx) = lower.rfind(m) {
                    return s[..idx].to_string();
                }
            }
            // Single extension strip via Path::file_stem (preserves case).
            Path::new(s)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| s.to_string())
        })
        .unwrap_or_else(|| "extracted".to_string());
    let candidate = parent.join(&stem);
    if !candidate.exists() {
        return candidate;
    }
    unique_dest(parent, std::ffi::OsStr::new(&stem))
}

/// True if any listed entry would escape the extraction dir (zip-slip):
/// an absolute path or a `..` component. tar and unzip sanitize these
/// themselves; 7z and unrar are only as safe as the archive, so their
/// branches list entries first and refuse.
fn entries_escape(listing: &str) -> bool {
    listing.lines().any(|l| {
        let p = Path::new(l.trim());
        p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
    })
}

fn list_7z(archive: &Path) -> Option<String> {
    let out = std::process::Command::new("7z")
        .args(["l", "-ba", "-slt"])
        .arg(archive)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(
        s.lines()
            .filter_map(|l| l.strip_prefix("Path = "))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn list_unrar(archive: &Path) -> Option<String> {
    let out = std::process::Command::new("unrar")
        .args(["lb", "-y"])
        .arg(archive)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Build (but do not spawn) the `Command` that extracts `archive` into
/// `dest`. Pure builder — no process is spawned here, so callers can
/// choose to run it synchronously or drive it with a spawn/try_wait/kill
/// loop for cancellation. `dest` must already exist as a directory (the
/// caller creates it via `extract_dest_for` + `create_dir_all`).
///
/// Shells out to system tools (unzip / tar / 7z / unrar / gunzip /
/// bunzip2 / xz / zstd) per extension; tar handles its own compression
/// detection. Runs the 7z/rar zip-slip precheck (`entries_escape`) before
/// building their commands, refusing archives with absolute or `../`
/// entries. Detaches stdio (`silent`) on every extractor — the TUI owns
/// the terminal, so a child that inherits stdio can spray progress over
/// the alternate screen or steal keyboard input (e.g. unrar prompting for
/// a password).
pub fn extract_command(archive: &Path, dest: &Path) -> io::Result<std::process::Command> {
    use std::process::{Command, Stdio};
    fn silent(cmd: &mut Command) -> &mut Command {
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
    }
    let name = archive
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let ext = archive
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let mut cmd = if name.ends_with(".tar.gz")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.zst")
        || name.ends_with(".tar.lz")
        || ext == "tar"
        || ext == "tgz"
        || ext == "tbz2"
        || ext == "txz"
        || ext == "tzst"
    {
        let mut c = Command::new("tar");
        c.arg("-xf").arg(archive).arg("-C").arg(dest);
        c
    } else if ext == "zip" {
        let mut c = Command::new("unzip");
        c.arg("-q").arg("-o").arg(archive).arg("-d").arg(dest);
        c
    } else if ext == "7z" {
        if list_7z(archive).map(|l| entries_escape(&l)).unwrap_or(false) {
            return Err(io::Error::other(
                "archive refused: entries with ../ or absolute paths",
            ));
        }
        let mut c = Command::new("7z");
        c.arg("x").arg("-y").arg(format!("-o{}", dest.display())).arg(archive);
        c
    } else if ext == "rar" {
        if list_unrar(archive).map(|l| entries_escape(&l)).unwrap_or(false) {
            return Err(io::Error::other(
                "archive refused: entries with ../ or absolute paths",
            ));
        }
        let mut c = Command::new("unrar");
        c.arg("x").arg("-y").arg(archive).arg(format!("{}/", dest.display()));
        c
    } else if matches!(ext.as_str(), "gz" | "bz2" | "xz" | "zst") {
        // Single-file compressors: write decompressed bytes to dest/<stem>.
        // Stem from the real filename, not the lowercased `name`, so
        // README.GZ decompresses to README, not readme.
        let stem = archive
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("out")
            .to_string();
        let out_path = dest.join(stem);
        let tool = match ext.as_str() {
            "gz" => "gunzip",
            "bz2" => "bunzip2",
            "xz" => "unxz",
            "zst" => "zstd",
            _ => unreachable!(),
        };
        // tool -dc <archive> > out_path
        let out_file = fs::File::create(&out_path)?;
        let args: Vec<&str> = if tool == "zstd" {
            vec!["-dc"]
        } else {
            vec!["-c"]
        };
        let mut c = Command::new(tool);
        c.args(args).arg(archive).stdin(Stdio::null()).stdout(out_file).stderr(Stdio::null());
        // stdout is the redirected out_file, not null — return directly,
        // skip the generic `silent()` below which would clobber it.
        return Ok(c);
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported archive type",
        ));
    };
    silent(&mut cmd);
    Ok(cmd)
}

/// Compute the rename plan from edited names. Validates: equal counts, no
/// empty/`.`/`..`/separator-bearing names, no duplicate targets. Returns
/// only the pairs whose name actually changed. Pure — no filesystem access.
pub fn plan_bulk_rename(
    olds: &[String],
    news: &[String],
) -> Result<Vec<(String, String)>, String> {
    if olds.len() != news.len() {
        return Err(format!(
            "line count mismatch: {} names, {} lines",
            olds.len(),
            news.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for n in news {
        let t = n.trim();
        if t.is_empty() || t == "." || t == ".." || t.contains('/')
            || t.contains(std::path::MAIN_SEPARATOR)
        {
            return Err(format!("invalid name: {n:?}"));
        }
        if !seen.insert(t.to_string()) {
            return Err(format!("duplicate target name: {t}"));
        }
    }
    Ok(olds
        .iter()
        .zip(news.iter())
        .filter(|(o, n)| o.trim() != n.trim())
        .map(|(o, n)| (o.clone(), n.trim().to_string()))
        .collect())
}

/// Execute rename pairs atomically-ish: every source moves to a unique
/// temp name first, then temps move to finals. A final that collides with
/// an untracked existing file still errors (no overwrite). Handles swaps
/// and chains, unlike a naive `rename(old, new)` loop.
///
/// `pairs` are same-dir names (no separators — `plan_bulk_rename` already
/// validates that).
///
/// ponytail: phase-1 failure leaves earlier files parked under
/// `.en-bulk-*` names — visible, recoverable, not silently lost; full
/// rollback on partial failure is the upgrade path if this ever bites.
pub fn execute_bulk_rename(dir: &Path, pairs: &[(String, String)]) -> Result<(), String> {
    let olds: std::collections::HashSet<&str> = pairs.iter().map(|(o, _)| o.as_str()).collect();
    // Phase 0: a target may not collide with a file that is NOT being renamed away.
    for (_, n) in pairs {
        if dir.join(n).exists() && !olds.contains(n.as_str()) {
            return Err(format!("target exists: {n}"));
        }
    }
    // Phase 1: park sources under collision-free temp names.
    let mut temps = Vec::new();
    for (i, (o, _)) in pairs.iter().enumerate() {
        let tmp = dir.join(format!(".en-bulk-{i}-{o}"));
        std::fs::rename(dir.join(o), &tmp).map_err(|e| format!("{o}: {e}"))?;
        temps.push(tmp);
    }
    // Phase 2: temps → finals.
    for (tmp, (_, n)) in temps.iter().zip(pairs) {
        std::fs::rename(tmp, dir.join(n)).map_err(|e| format!("{n}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_atomic_persists_content() {
        let d = tempdir().unwrap();
        let p = d.path().join("state.toml");
        write_atomic(&p, "a = 1").unwrap();
        write_atomic(&p, "a = 2").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a = 2");
    }

    #[test]
    fn entries_escape_flags_traversal_and_absolute() {
        assert!(entries_escape("ok.txt\n../evil.sh"));
        assert!(entries_escape("/etc/passwd"));
        assert!(entries_escape("a/b/../../../evil"));
        assert!(!entries_escape("a.txt\nsub/dir/b.txt\nweird..name"));
        assert!(!entries_escape(""));
    }

    #[test]
    fn unique_dest_increments() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "1").unwrap();
        let u = unique_dest(d.path(), std::ffi::OsStr::new("a.txt"));
        assert_eq!(u.file_name().unwrap(), "a (1).txt");
    }

    #[test]
    fn is_archive_detects_common_extensions() {
        assert!(is_archive(Path::new("foo.zip")));
        assert!(is_archive(Path::new("foo.tar")));
        assert!(is_archive(Path::new("foo.tar.gz")));
        assert!(is_archive(Path::new("foo.TAR.XZ")));
        assert!(is_archive(Path::new("foo.7z")));
        assert!(is_archive(Path::new("foo.gz")));
        assert!(!is_archive(Path::new("foo.txt")));
        assert!(!is_archive(Path::new("foo")));
    }

    #[test]
    fn extract_dest_strips_multipart_ext() {
        let p = Path::new("/tmp/foo.tar.gz");
        let d = extract_dest_for(p);
        assert_eq!(d, PathBuf::from("/tmp/foo"));
    }

    #[test]
    fn extract_dest_strips_single_ext() {
        let p = Path::new("/tmp/bar.zip");
        let d = extract_dest_for(p);
        assert_eq!(d, PathBuf::from("/tmp/bar"));
    }

    #[test]
    fn claim_dest_avoids_already_claimed() {
        use std::collections::HashSet;
        use std::ffi::OsStr;
        let d = tempdir().unwrap();
        let mut claimed: HashSet<std::path::PathBuf> = HashSet::new();
        // Nothing on disk; two claims of the same name must differ.
        let a = claim_dest(d.path(), OsStr::new("f.txt"), &mut claimed);
        let b = claim_dest(d.path(), OsStr::new("f.txt"), &mut claimed);
        assert_ne!(a, b, "second claim must not collide with the first");
        assert_eq!(a.file_name().unwrap(), "f.txt");
        assert_eq!(b.file_name().unwrap(), "f (1).txt");
    }

    #[test]
    fn paste_recurse_detection() {
        use std::path::Path;
        // dst inside src → recursion
        assert!(paste_would_recurse(Path::new("/a/foo"), Path::new("/a/foo")));
        assert!(paste_would_recurse(Path::new("/a/foo"), Path::new("/a/foo/bar")));
        // dst is the source's own parent (paste back into same dir) → OK
        assert!(!paste_would_recurse(Path::new("/a/foo"), Path::new("/a")));
        // unrelated → OK
        assert!(!paste_would_recurse(Path::new("/a/foo"), Path::new("/b")));
    }

    #[cfg(unix)]
    #[test]
    fn move_file_cross_fs_cleans_dest_when_source_remove_fails() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir().unwrap();
        let srcdir = d.path().join("ro");
        std::fs::create_dir(&srcdir).unwrap();
        let src = srcdir.join("f.txt");
        std::fs::write(&src, b"data").unwrap();
        let dst = d.path().join("f.txt");
        // Make the source's parent read-only so remove_file(src) fails
        // AFTER copy(src,dst) has already succeeded.
        std::fs::set_permissions(&srcdir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = move_file_cross_fs(&src, &dst);
        // restore perms so tempdir cleanup works
        std::fs::set_permissions(&srcdir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err(), "remove failure must surface as Err");
        assert!(!dst.exists(), "dest must be cleaned up, not left as a duplicate");
        assert!(src.exists(), "source must remain intact");
    }

    #[test]
    fn move_file_cross_fs_happy_path_moves() {
        let d = tempdir().unwrap();
        let src = d.path().join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        let dst = d.path().join("b.txt");
        assert!(move_file_cross_fs(&src, &dst).is_ok());
        assert!(dst.exists() && !src.exists());
    }

    #[test]
    fn plan_bulk_rename_validates_and_diffs() {
        // count mismatch -> error
        assert!(plan_bulk_rename(&["a".into(), "b".into()], &["a".into()]).is_err());
        // only changed pairs returned
        let got = plan_bulk_rename(
            &["a.txt".into(), "b.txt".into()],
            &["a.txt".into(), "c.txt".into()],
        ).unwrap();
        assert_eq!(got, vec![("b.txt".to_string(), "c.txt".to_string())]);
        // duplicate new names -> error
        assert!(plan_bulk_rename(
            &["a".into(), "b".into()],
            &["same".into(), "same".into()],
        ).is_err());
        // empty new name -> error
        assert!(plan_bulk_rename(&["a".into()], &["".into()]).is_err());
        // path separator in new name -> error
        assert!(plan_bulk_rename(&["a".into()], &["x/y".into()]).is_err());
    }

    #[test]
    fn bulk_rename_swap_works() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a"), "A").unwrap();
        std::fs::write(d.path().join("b"), "B").unwrap();
        let pairs = vec![("a".to_string(), "b".to_string()), ("b".to_string(), "a".to_string())];
        execute_bulk_rename(d.path(), &pairs).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("a")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(d.path().join("b")).unwrap(), "A");
    }

    #[test]
    fn bulk_rename_chain_works() {
        // a→b, b→c: both must land (b's old content ends up at c, a's
        // old content ends up at b).
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a"), "A").unwrap();
        std::fs::write(d.path().join("b"), "B").unwrap();
        let pairs = vec![("a".to_string(), "b".to_string()), ("b".to_string(), "c".to_string())];
        execute_bulk_rename(d.path(), &pairs).unwrap();
        assert!(!d.path().join("a").exists());
        assert_eq!(std::fs::read_to_string(d.path().join("b")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(d.path().join("c")).unwrap(), "B");
    }

    #[test]
    fn bulk_rename_rejects_collision_with_untracked_file() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a"), "A").unwrap();
        std::fs::write(d.path().join("z"), "Z").unwrap();
        let pairs = vec![("a".to_string(), "z".to_string())];
        assert!(execute_bulk_rename(d.path(), &pairs).is_err());
        // untouched on rejection
        assert_eq!(std::fs::read_to_string(d.path().join("a")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(d.path().join("z")).unwrap(), "Z");
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_preserves_symlinks_and_broken_links() {
        let d = tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("real")).unwrap();
        std::fs::write(src.join("real/f.txt"), "data").unwrap();
        std::os::unix::fs::symlink("real", src.join("link-to-dir")).unwrap();
        std::os::unix::fs::symlink("nowhere", src.join("broken")).unwrap();
        let dst = d.path().join("dst");
        let cancel = AtomicBool::new(false);
        let ctl = CopyCtl { cancel: &cancel, progress: &|_| {} };
        copy_dir_recursive(&src, &dst, &ctl).unwrap();
        assert!(dst.join("real/f.txt").is_file());
        assert!(dst
            .join("link-to-dir")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(dst.join("broken")).unwrap(), Path::new("nowhere"));
    }

    #[test]
    fn chunked_copy_cancels_mid_file() {
        // 8 MiB file, cancel flag set from the progress callback after the
        // first 1 MiB chunk: expect Err(Interrupted) and the partial dst
        // removed (copy_file_chunked's own contract).
        let d = tempdir().unwrap();
        let src = d.path().join("big.bin");
        std::fs::write(&src, vec![7u8; 8 * 1024 * 1024]).unwrap();
        let dst = d.path().join("dst.bin");
        let cancel = AtomicBool::new(false);
        let progress = |_n: u64| {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let ctl = CopyCtl { cancel: &cancel, progress: &progress };
        let err = copy_file_chunked(&src, &dst, &ctl).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(!dst.exists(), "partial dst must be removed on cancel");
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_handles_cyclic_symlink() {
        // ln -s . self  → copies as a symlink, no recursion, no error.
        let d = tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::os::unix::fs::symlink(".", src.join("self")).unwrap();
        let dst = d.path().join("dst");
        let cancel = AtomicBool::new(false);
        let ctl = CopyCtl { cancel: &cancel, progress: &|_| {} };
        copy_dir_recursive(&src, &dst, &ctl).unwrap();
        assert!(dst
            .join("self")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(dst.join("self")).unwrap(), Path::new("."));
    }

    #[test]
    fn extract_command_rejects_traversal_rar() {
        // No real rar fixture needed: an archive with an unsupported
        // extension (no rar/7z tool required) still exercises the
        // unsupported-type error path; the entries_escape precheck itself
        // is covered directly above. This test pins extract_command's
        // "unsupported archive type" error for a non-archive extension.
        let d = tempdir().unwrap();
        let archive = d.path().join("notes.txt");
        std::fs::write(&archive, "hi").unwrap();
        let dest = d.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let err = extract_command(&archive, &dest).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
