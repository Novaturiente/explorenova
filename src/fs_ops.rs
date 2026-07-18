use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
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
fn extract_dest_for(archive: &Path) -> PathBuf {
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

/// Extract `archive` into a sibling directory (`unique_dest` rules).
/// Returns the destination path on success. Shells out to system tools
/// (unzip / tar / 7z / unrar / gunzip / bunzip2 / xz / zstd) per
/// extension. Tar handles its own compression detection.
pub fn extract_archive(archive: &Path) -> io::Result<PathBuf> {
    use std::process::{Command, Stdio};
    // The TUI owns the terminal: a child that inherits stdio sprays its
    // progress over the alternate screen and can steal / block on
    // keyboard input (e.g. unrar prompting for a password). Detach all
    // three streams on every extractor.
    fn silent(cmd: &mut Command) -> &mut Command {
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
    }
    let dest = extract_dest_for(archive);
    fs::create_dir_all(&dest)?;
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
    let status = if name.ends_with(".tar.gz")
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
        silent(Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&dest))
            .status()
    } else if ext == "zip" {
        silent(Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(&dest))
            .status()
    } else if ext == "7z" {
        if list_7z(archive).map(|l| entries_escape(&l)).unwrap_or(false) {
            return Err(io::Error::other(
                "archive refused: entries with ../ or absolute paths",
            ));
        }
        silent(Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest.display()))
            .arg(archive))
            .status()
    } else if ext == "rar" {
        if list_unrar(archive).map(|l| entries_escape(&l)).unwrap_or(false) {
            return Err(io::Error::other(
                "archive refused: entries with ../ or absolute paths",
            ));
        }
        silent(Command::new("unrar")
            .arg("x")
            .arg("-y")
            .arg(archive)
            .arg(format!("{}/", dest.display())))
            .status()
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
        Command::new(tool)
            .args(args)
            .arg(archive)
            .stdin(Stdio::null())
            .stdout(out_file)
            .stderr(Stdio::null())
            .status()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported archive type",
        ));
    };
    match status {
        Ok(s) if s.success() => Ok(dest),
        Ok(s) => Err(io::Error::other(format!(
            "extractor exited with status {s}"
        ))),
        Err(e) => Err(io::Error::other(format!(
            "failed to spawn extractor: {e}"
        ))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
}
