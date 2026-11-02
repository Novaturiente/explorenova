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

const MAX_ENTRIES: usize = 200;

/// Bytes of `bsdtar -tf` output we keep. `MAX_ENTRIES` at a full
/// PATH_MAX each is far more than any real listing needs, and past it
/// we only need to know that there IS more — so the capture stops
/// accumulating instead of buffering a pathological archive's entire
/// (multi-gigabyte) name table just to discard all but 200 lines.
const MAX_OUT: usize = MAX_ENTRIES * 4096;

/// List up to 200 entries from `path` via bsdtar. Returns a single
/// String suitable for `PreviewKind::Text` / `PreviewKind::Meta`. None
/// if bsdtar is not installed or the archive is unreadable.
pub fn list_entries(path: &Path) -> Option<String> {
    let (status, raw) = super::media::capture_with_timeout(
        Command::new("bsdtar").args(["-tf"]).arg(path),
        std::time::Duration::from_secs(10),
        MAX_OUT,
    )?;
    if !status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&raw);
    let mut lines: Vec<&str> = stdout.lines().take(MAX_ENTRIES).collect();
    // Output hit the byte cap: the tail was dropped mid-stream, so the
    // last line we kept may be a partial name and the entry count is a
    // floor, not a total.
    let capped = raw.len() >= MAX_OUT;
    if capped {
        lines.pop();
    }
    let counted = stdout.lines().count();
    let truncated = counted > lines.len();
    let header = format!(
        "{}\n{}{} entries{}\n\n",
        path.display(),
        counted,
        if capped { "+" } else { "" },
        if truncated { " (showing first 200)" } else { "" },
    );
    if truncated {
        lines.push("…");
    }
    Some(format!("{header}{}", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_keeps_at_most_max_out_bytes() {
        // The cap must bound memory while still letting the child run to
        // completion (a blocked writer would hang the preview worker).
        let mut c = Command::new("sh");
        c.args(["-c", "yes abcdefghij | head -c 200000"]);
        let (status, raw) = super::super::media::capture_with_timeout(
            &mut c,
            std::time::Duration::from_secs(10),
            1024,
        )
        .expect("child should exit, not time out");
        assert!(status.success());
        assert_eq!(raw.len(), 1024);
    }
}
