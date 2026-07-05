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
    /// Decoded image (original or rasterized) + its source path for
    /// cache keying. Decoded HERE, on the worker thread — decoding on
    /// the render thread froze the UI for seconds on large files. Arc
    /// keeps the per-frame cache clone free.
    Image(PathBuf, std::sync::Arc<image::DynamicImage>),
    Meta(String),
}

/// Largest file we hand to an in-process heavy decoder (image crate,
/// calamine). Images are further bounded by `decode_image_bounded`'s
/// pixel limits.
const HEAVY_FILE_MAX: u64 = 64 * 1024 * 1024;

/// Decode with explicit limits so a decompression-bomb PNG can't OOM
/// the process. Returns None on unreadable/oversized images.
fn decode_image_bounded(path: &Path) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

fn image_preview(png: PathBuf, meta: &std::fs::Metadata) -> PreviewKind {
    match decode_image_bounded(&png) {
        Some(img) => PreviewKind::Image(png, std::sync::Arc::new(img)),
        None => PreviewKind::Meta(meta_string(&png, meta)),
    }
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
pub fn settle_ms(path: &Path, cheap_ms: u64, heavy_ms: u64) -> u64 {
    let ext = ext_lower(path);
    let heavy = ext == "pdf"
        || ext == "avif"
        || media::is_video(&ext)
        || xlsx::is_spreadsheet(&ext)
        || archive::is_archive(&ext);
    if heavy {
        heavy_ms
    } else {
        cheap_ms
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
        if meta.len() > HEAVY_FILE_MAX {
            return PreviewKind::Meta(meta_string(path, &meta));
        }
        return image_preview(path.to_path_buf(), &meta);
    }
    if ext == "avif" {
        return match media::rasterize_avif(path) {
            Some(png) => image_preview(png, &meta),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if media::is_video(&ext) {
        return match media::video_thumbnail(path) {
            Some(png) => image_preview(png, &meta),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if ext == "pdf" {
        return match media::rasterize_pdf(path) {
            Some(png) => image_preview(png, &meta),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if xlsx::is_spreadsheet(&ext) {
        // calamine parses the whole workbook in memory — bound it.
        if meta.len() > HEAVY_FILE_MAX {
            return PreviewKind::Meta(meta_string(path, &meta));
        }
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

/// Cap on cached previews. A `Text` entry can hold thousands of styled
/// spans, so without a bound a long browsing session leaks memory. Evicted
/// entries simply re-render on next view.
const CACHE_CAP: usize = 128;

/// (path, mtime)-keyed preview cache backed by a worker thread.
pub struct PreviewCache {
    map: HashMap<(PathBuf, SystemTime), PreviewKind>,
    in_flight: HashSet<(PathBuf, SystemTime)>,
    /// Insertion order, for FIFO eviction once `CACHE_CAP` is reached.
    order: std::collections::VecDeque<(PathBuf, SystemTime)>,
    /// Max cached entries before FIFO eviction (from config).
    cap: usize,
    req_tx: Sender<(PathBuf, SystemTime, u64)>,
    res_rx: Receiver<((PathBuf, SystemTime), PreviewKind)>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::with_cap(CACHE_CAP)
    }
}

impl PreviewCache {
    pub fn new() -> Self {
        Self::with_cap(CACHE_CAP)
    }

    pub fn with_cap(cap: usize) -> Self {
        let cap = if cap == 0 { CACHE_CAP } else { cap };
        let (req_tx, req_rx) = mpsc::channel::<(PathBuf, SystemTime, u64)>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((path, mtime, max_bytes)) = req_rx.recv() {
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
        Self {
            map: HashMap::new(),
            in_flight: HashSet::new(),
            order: std::collections::VecDeque::new(),
            cap,
            req_tx,
            res_rx,
        }
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
            if self.map.insert(key.clone(), kind).is_none() {
                // New key: track for FIFO eviction and trim if over cap.
                self.order.push_back(key);
                while self.order.len() > self.cap {
                    if let Some(old) = self.order.pop_front() {
                        self.map.remove(&old);
                    }
                }
            }
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
        let cheap = settle_ms(Path::new("notes.txt"), 150, 400);
        assert_eq!(cheap, 150);
        assert_eq!(cheap, settle_ms(Path::new("photo.png"), 150, 400));
        for heavy in ["doc.pdf", "clip.mp4", "book.avif", "sheet.xlsx", "bundle.tar.gz"] {
            assert!(settle_ms(Path::new(heavy), 150, 400) > cheap, "{heavy} should be heavy");
        }
        // custom values are honored
        assert_eq!(settle_ms(Path::new("notes.txt"), 50, 900), 50);
        assert_eq!(settle_ms(Path::new("doc.pdf"), 50, 900), 900);
    }

    #[test]
    fn cache_respects_configured_cap() {
        let c = PreviewCache::with_cap(2);
        assert_eq!(c.cap, 2);
    }

    #[test]
    fn image_ext_is_image_kind() {
        let d = tempdir().unwrap();
        let f = d.path().join("p.png");
        image::RgbaImage::new(1, 1).save(&f).unwrap();
        assert!(matches!(render(&f, 1024), PreviewKind::Image(..)));
    }

    #[test]
    fn corrupt_image_falls_back_to_meta() {
        let d = tempdir().unwrap();
        let f = d.path().join("bad.png");
        fs::write(&f, [0u8; 8]).unwrap();
        assert!(matches!(render(&f, 1024), PreviewKind::Meta(_)));
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
