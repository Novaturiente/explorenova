//! Archive listings via `bsdtar -tf` (libarchive). One command covers
//! zip / tar / tar.gz / 7z / iso / cpio / rar / etc. We render the
//! first ~200 entries as plain text and prefix a short header so the
//! preview pane stays readable for huge archives.

use std::path::Path;
use std::process::Command;

const ARCHIVE_EXTS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "cab",
    "iso", "deb", "rpm", "jar", "war", "apk",
];

pub fn is_archive(ext_lower: &str) -> bool {
    ARCHIVE_EXTS.contains(&ext_lower)
}

/// List up to 200 entries from `path` via bsdtar. Returns a single
/// String suitable for `PreviewKind::Text` / `PreviewKind::Meta`. None
/// if bsdtar is not installed or the archive is unreadable.
pub fn list_entries(path: &Path) -> Option<String> {
    let (status, raw) = super::media::capture_with_timeout(
        Command::new("bsdtar").args(["-tf"]).arg(path),
        std::time::Duration::from_secs(10),
    )?;
    if !status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&raw);
    let mut lines: Vec<&str> = stdout.lines().take(200).collect();
    let truncated = stdout.lines().count() > lines.len();
    let header = format!(
        "{}\n{} entries{}\n\n",
        path.display(),
        stdout.lines().count(),
        if truncated { " (showing first 200)" } else { "" },
    );
    if truncated {
        lines.push("…");
    }
    Some(format!("{header}{}", lines.join("\n")))
}
