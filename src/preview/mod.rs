pub mod archive;
pub mod media;
pub mod text;
pub mod xlsx;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum PreviewKind {
    /// Pre-styled text (syntect spans → ratatui spans). Carrying a
    /// `Text<'static>` lets ratatui clip per cell so long lines never
    /// spill into adjacent panes the way raw ANSI strings used to.
    Text(ratatui::text::Text<'static>),
    Image(PathBuf), // path to displayable image (original or rasterized)
    Meta(String),
}

fn ext_lower(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

const IMAGE_EXTS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif"];

/// Replace tabs with four spaces and strip every other ASCII control
/// char (NUL, BS, ESC, BEL, …). Without this, raw control bytes in
/// archive filenames or xlsx cells leak to the terminal as cursor
/// motion / colour changes and scatter "phantom" text across panes.
fn sanitize_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if (c as u32) < 0x20 => out.push('?'),
            c if c == '\u{7f}' => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

fn plain_text(s: String) -> PreviewKind {
    use ratatui::text::{Line, Text};
    let s = sanitize_plain(&s);
    let lines: Vec<Line<'static>> = s
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    PreviewKind::Text(Text::from(lines))
}

/// Decide preview kind for `path` (no caching).
pub fn render(path: &Path, max_bytes: u64) -> PreviewKind {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return PreviewKind::Meta(format!("error: {e}")),
    };
    if meta.is_dir() {
        return PreviewKind::Meta(format!("directory\n{} entries", count_entries(path)));
    }
    let ext = ext_lower(path);
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return PreviewKind::Image(path.to_path_buf());
    }
    if ext == "avif" {
        return match media::rasterize_avif(path) {
            Some(png) => PreviewKind::Image(png),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if media::is_video(&ext) {
        return match media::video_thumbnail(path) {
            Some(png) => PreviewKind::Image(png),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if ext == "pdf" {
        return match media::rasterize_pdf(path) {
            Some(png) => PreviewKind::Image(png),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if xlsx::is_spreadsheet(&ext) {
        return match xlsx::text_from(path) {
            Some(t) => plain_text(t),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if archive::is_archive(&ext) {
        return match archive::list_entries(path) {
            Some(t) => plain_text(t),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if meta.len() > max_bytes {
        return PreviewKind::Meta(meta_string(path, &meta));
    }
    match text::highlighted(path, max_bytes) {
        Some(t) => PreviewKind::Text(t),
        None => PreviewKind::Meta(meta_string(path, &meta)),
    }
}

fn count_entries(p: &Path) -> usize {
    std::fs::read_dir(p).map(|r| r.count()).unwrap_or(0)
}

fn meta_string(p: &Path, m: &std::fs::Metadata) -> String {
    format!("{}\n{} bytes\n(binary or oversized)", p.display(), m.len())
}

/// (path, mtime)-keyed preview cache.
#[derive(Default)]
pub struct PreviewCache {
    map: HashMap<(PathBuf, SystemTime), PreviewKind>,
}

impl PreviewCache {
    /// True if `path` is in the cache at its current mtime. Used by
    /// the UI to skip rendering a placeholder when a cached preview
    /// can be returned synchronously.
    pub fn contains(&self, path: &Path) -> bool {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.map.contains_key(&(path.to_path_buf(), mtime))
    }

    pub fn get_or_render(&mut self, path: &Path, max_bytes: u64) -> PreviewKind {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let key = (path.to_path_buf(), mtime);
        if let Some(v) = self.map.get(&key) {
            return v.clone();
        }
        let v = render(path, max_bytes);
        self.map.insert(key, v.clone());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn sanitize_replaces_tabs_and_strips_control_chars() {
        let s = "a\tb\nc\x00d\x07e\x1bf";
        assert_eq!(sanitize_plain(s), "a    b\nc?d?e?f");
    }

    #[test]
    fn sanitize_preserves_unicode_and_printable() {
        let s = "héllo — 日本語";
        assert_eq!(sanitize_plain(s), s);
    }

    #[test]
    fn text_file_is_text_preview() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        match render(&f, 1024) {
            PreviewKind::Text(t) => {
                let joined: String = t
                    .lines
                    .iter()
                    .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                    .collect();
                assert!(joined.contains("hello"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn oversized_falls_back_to_meta() {
        let d = tempdir().unwrap();
        let f = d.path().join("big.txt");
        fs::write(&f, vec![b'x'; 2048]).unwrap();
        assert!(matches!(render(&f, 100), PreviewKind::Meta(_)));
    }

    #[test]
    fn image_ext_is_image_kind() {
        let d = tempdir().unwrap();
        let f = d.path().join("p.png");
        fs::write(&f, [0u8; 8]).unwrap();
        assert!(matches!(render(&f, 1024), PreviewKind::Image(_)));
    }

    #[test]
    fn cache_returns_same_value_twice() {
        let d = tempdir().unwrap();
        let f = d.path().join("c.txt");
        fs::write(&f, "x").unwrap();
        let mut c = PreviewCache::default();
        let a = c.get_or_render(&f, 1024);
        let b = c.get_or_render(&f, 1024);
        // PreviewKind dropped PartialEq when Text started carrying styled
        // spans (Style doesn't compare cleanly). Compare via Debug repr.
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
