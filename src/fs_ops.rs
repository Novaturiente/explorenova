use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum PasteMode {
    Copy,
    Move,
}

/// Trailing `/` => directory (mkdir -p). Else create file, mkdir -p its parent.
pub fn create(path: &Path) -> io::Result<()> {
    let is_dir = path
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::MAIN_SEPARATOR);
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
}
