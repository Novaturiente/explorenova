pub mod archive;
pub mod media;
pub mod text;
pub mod xlsx;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
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

/// Settle delay (ms) before generating a preview for `path`, used by the
/// UI debounce gate. Cheap previews (text, dir, plain image) are ready
/// fast, so a short delay keeps browsing responsive. Heavy previews that
/// spawn a subprocess or parse a container (pdf, avif, video, spreadsheet,
/// archive) block the UI thread while they run, so they get a longer
/// settle — scrolling past them never triggers the expensive work; only
/// resting on one does.
pub fn settle_ms(path: &Path) -> u64 {
    const CHEAP_MS: u64 = 150;
    const HEAVY_MS: u64 = 400;
    let ext = ext_lower(path);
    let heavy = ext == "pdf"
        || ext == "avif"
        || media::is_video(&ext)
        || xlsx::is_spreadsheet(&ext)
        || archive::is_archive(&ext);
    if heavy {
        HEAVY_MS
    } else {
        CHEAP_MS
    }
}

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
            '\u{7f}' => out.push('?'),
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

/// (path, mtime)-keyed preview cache backed by a worker thread.
pub struct PreviewCache {
    map: HashMap<(PathBuf, SystemTime), PreviewKind>,
    in_flight: HashSet<(PathBuf, SystemTime)>,
    req_tx: Sender<(PathBuf, SystemTime, u64)>,
    res_rx: Receiver<((PathBuf, SystemTime), PreviewKind)>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewCache {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(PathBuf, SystemTime, u64)>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((path, mtime, max_bytes)) = req_rx.recv() {
                // Isolate a panicking render (e.g. a malformed file tripping
                // a decoder) so one bad file never kills the worker and
                // wedges every future preview at "(loading…)".
                let kind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render(&path, max_bytes)
                }))
                .unwrap_or_else(|_| PreviewKind::Meta(format!(
                    "{}\n(preview failed)",
                    path.display()
                )));
                let _ = res_tx.send(((path, mtime), kind));
            }
        });
        Self { map: HashMap::new(), in_flight: HashSet::new(), req_tx, res_rx }
    }

    fn key(path: &Path) -> (PathBuf, SystemTime) {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        (path.to_path_buf(), mtime)
    }

    /// True if `path` is in the cache at its current mtime. Used by
    /// the UI settle gate to skip the placeholder when a cached preview
    /// can be returned synchronously.
    pub fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(&Self::key(path))
    }

    /// Return the cached preview if present; otherwise dispatch the render
    /// off-thread and return `None`. Never blocks on I/O.
    pub fn request(&mut self, path: &Path, max_bytes: u64) -> Option<PreviewKind> {
        let key = Self::key(path);
        if let Some(v) = self.map.get(&key) {
            return Some(v.clone());
        }
        if self.in_flight.insert(key.clone()) {
            let _ = self.req_tx.send((key.0, key.1, max_bytes));
        }
        None
    }

    /// Move any completed worker results into the cache. Returns `true` if
    /// at least one arrived (caller should redraw).
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok((key, kind)) = self.res_rx.try_recv() {
            self.in_flight.remove(&key);
            self.map.insert(key, kind);
            got = true;
        }
        got
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
    fn heavy_types_get_longer_settle_than_cheap() {
        // Cheap previews settle fast; heavy (subprocess/parse) types wait
        // longer so scrolling past them doesn't trigger the blocking work.
        let cheap = settle_ms(Path::new("notes.txt"));
        assert_eq!(cheap, settle_ms(Path::new("photo.png")));
        for heavy in ["doc.pdf", "bundle.zip", "sheet.xlsx", "clip.mp4", "img.avif"] {
            assert!(settle_ms(Path::new(heavy)) > cheap, "{heavy} should be heavy");
        }
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
        let mut c = PreviewCache::new();
        // First request: not cached → None, work dispatched off-thread.
        assert!(c.request(&f, 1024).is_none());
        // Drain until the worker delivers.
        let mut got = false;
        for _ in 0..200 {
            if c.drain() { got = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "worker should deliver a result");
        // Now cached — two consecutive requests return the same value.
        let a = c.request(&f, 1024).expect("should be cached");
        let b = c.request(&f, 1024).expect("should still be cached");
        // PreviewKind dropped PartialEq when Text started carrying styled
        // spans (Style doesn't compare cleanly). Compare via Debug repr.
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn request_is_async_then_drains_into_cache() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        let mut c = PreviewCache::new();
        // First request: not cached → None, work dispatched off-thread.
        assert!(c.request(&f, 1024).is_none());
        // Wait for the worker, draining until the result lands.
        let mut got = false;
        for _ in 0..200 {
            if c.drain() { got = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "worker should deliver a result");
        // Now cached → Some, synchronously.
        assert!(matches!(c.request(&f, 1024), Some(PreviewKind::Text(_))));
    }
}
