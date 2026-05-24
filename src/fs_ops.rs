use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum PasteMode {
    Copy,
    Move,
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

pub fn paste(src: &Path, dst_dir: &Path, mode: PasteMode) -> io::Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_dest(dst_dir, name);
    match mode {
        PasteMode::Copy => {
            if src.is_dir() {
                let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                fs_extra::dir::copy(src, &dst, &opts)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            } else {
                fs::copy(src, &dst)?;
            }
        }
        PasteMode::Move => {
            match fs::rename(src, &dst) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                    // cross-device fallback
                    if src.is_dir() {
                        let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                        fs_extra::dir::move_dir(src, &dst, &opts)
                            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    } else {
                        fs::copy(src, &dst)?;
                        fs::remove_file(src)?;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(dst)
}

pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let np = parent.join(new_name);
    fs::rename(path, &np)?;
    Ok(np)
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

/// Extract `archive` into a sibling directory (`unique_dest` rules).
/// Returns the destination path on success. Shells out to system tools
/// (unzip / tar / 7z / unrar / gunzip / bunzip2 / xz / zstd) per
/// extension. Tar handles its own compression detection.
pub fn extract_archive(archive: &Path) -> io::Result<PathBuf> {
    use std::process::Command;
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
        Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&dest)
            .status()
    } else if ext == "zip" {
        Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(&dest)
            .status()
    } else if ext == "7z" {
        Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest.display()))
            .arg(archive)
            .status()
    } else if ext == "rar" {
        Command::new("unrar")
            .arg("x")
            .arg("-y")
            .arg(archive)
            .arg(format!("{}/", dest.display()))
            .status()
    } else if matches!(ext.as_str(), "gz" | "bz2" | "xz" | "zst") {
        // Single-file compressors: write decompressed bytes to dest/<stem>.
        let stem = Path::new(&name)
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
            .stdout(out_file)
            .status()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported archive type",
        ));
    };
    match status {
        Ok(s) if s.success() => Ok(dest),
        Ok(s) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("extractor exited with status {s}"),
        )),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("failed to spawn extractor: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
}
