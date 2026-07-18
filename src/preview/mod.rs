pub mod archive;
pub mod media;
pub mod text;
pub mod xlsx;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
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

impl PreviewKind {
    /// Rough resident size for cache accounting. Not exact (styling
    /// overhead, allocator slop), just enough to keep the byte-bounded
    /// cache in the right ballpark.
    fn approx_bytes(&self) -> usize {
        match self {
            PreviewKind::Text(t) => t
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.len() + 32).sum::<usize>())
                .sum(),
            PreviewKind::Image(_, img) => (img.width() as usize) * (img.height() as usize) * 4,
            PreviewKind::Meta(s) => s.len(),
        }
    }
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
pub(crate) fn sanitize_plain(s: &str) -> String {
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

/// Borrow `t`'s styled spans instead of deep-cloning their string content.
/// Callers that need an owned `Text` to hand to a ratatui widget (e.g.
/// `Paragraph::new`) but only have a `&Text` from an `Arc<PreviewKind>`
/// cache hit can use this: it reallocates the container `Vec`s (cheap —
/// pointer/len/style copies) instead of cloning every span's `String`
/// (the actual cost for a highlighted file with thousands of spans).
pub fn borrow_text<'a>(t: &'a ratatui::text::Text<'static>) -> ratatui::text::Text<'a> {
    ratatui::text::Text {
        alignment: t.alignment,
        style: t.style,
        lines: t
            .lines
            .iter()
            .map(|l| ratatui::text::Line {
                style: l.style,
                alignment: l.alignment,
                spans: l
                    .spans
                    .iter()
                    .map(|s| ratatui::text::Span {
                        style: s.style,
                        content: std::borrow::Cow::Borrowed(s.content.as_ref()),
                    })
                    .collect(),
            })
            .collect(),
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

/// Byte-budget ceiling alongside `cap`: a handful of large decoded images
/// or highlighted files can blow past a reasonable entry cap in RSS long
/// before hitting 128 entries. ponytail: fixed constant, not a config
/// knob — `cap` already covers the "user wants a smaller cache" case;
/// revisit only if 128 MiB proves wrong in practice.
const BYTE_BUDGET: usize = 128 * 1024 * 1024;

/// Path-keyed preview cache backed by a single worker thread. No
/// filesystem access happens on the caller (render) thread — the worker
/// stats the file itself when it dequeues a request. Freshness comes from
/// external invalidation (`invalidate_dir`/`clear`) driven by fs-watch
/// events and op/shell-out completions, not per-frame mtime checks.
pub struct PreviewCache {
    map: HashMap<PathBuf, (SystemTime, Arc<PreviewKind>)>,
    in_flight: HashSet<PathBuf>,
    /// Insertion order, for FIFO eviction once `cap`/`byte_budget` is hit.
    order: VecDeque<PathBuf>,
    /// Sum of `approx_bytes()` for everything currently cached.
    total_bytes: usize,
    /// Max resident bytes before FIFO eviction.
    byte_budget: usize,
    /// Max cached entries before FIFO eviction (from config).
    cap: usize,
    /// Most-recently-requested path. The worker checks this before doing
    /// any real work so a fast-moving cursor doesn't backlog the single
    /// render thread with previews nobody is looking at anymore.
    want: Arc<Mutex<Option<PathBuf>>>,
    req_tx: Sender<(PathBuf, u64)>,
    res_rx: Receiver<(PathBuf, Option<(SystemTime, PreviewKind)>)>,
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
        let (req_tx, req_rx) = mpsc::channel::<(PathBuf, u64)>();
        let (res_tx, res_rx) = mpsc::channel::<(PathBuf, Option<(SystemTime, PreviewKind)>)>();
        let want: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let want_worker = want.clone();
        std::thread::spawn(move || {
            while let Ok((path, max_bytes)) = req_rx.recv() {
                // Stale-skip: if the cursor moved on before we even got to
                // this request, don't render it — but always reply so the
                // caller's `in_flight` bookkeeping stays correct.
                let is_current = want_worker
                    .lock()
                    .map(|w| w.as_deref() == Some(path.as_path()))
                    .unwrap_or(true);
                if !is_current {
                    let _ = res_tx.send((path, None));
                    continue;
                }
                let mtime = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let kind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render(&path, max_bytes)
                }))
                .unwrap_or_else(|_| PreviewKind::Meta(format!(
                    "{}\n(preview failed)",
                    path.display()
                )));
                let _ = res_tx.send((path, Some((mtime, kind))));
            }
        });
        Self {
            map: HashMap::new(),
            in_flight: HashSet::new(),
            order: VecDeque::new(),
            total_bytes: 0,
            byte_budget: BYTE_BUDGET,
            cap,
            want,
            req_tx,
            res_rx,
        }
    }

    /// True if `path` is currently cached. Pure map lookup — no I/O. Used
    /// by the UI settle gate to skip the placeholder when a cached
    /// preview can be returned synchronously.
    pub fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(path)
    }

    /// Return the cached preview, if any. Never blocks, never stats —
    /// just an `Arc` clone (cheap refcount bump, not a deep copy of the
    /// underlying `Text`/`Image`).
    pub fn get(&self, path: &Path) -> Option<Arc<PreviewKind>> {
        self.map.get(path).map(|(_, v)| v.clone())
    }

    /// Queue a render for `path` if it isn't already cached or in flight.
    /// Fire-and-forget: the worker does the stat and the render off
    /// thread; poll `drain()` and then `get()` to pick up the result.
    pub fn request(&mut self, path: &Path, max_bytes: u64) {
        if let Ok(mut w) = self.want.lock() {
            *w = Some(path.to_path_buf());
        }
        if self.map.contains_key(path) || self.in_flight.contains(path) {
            return;
        }
        self.in_flight.insert(path.to_path_buf());
        let _ = self.req_tx.send((path.to_path_buf(), max_bytes));
    }

    /// Move any completed worker results into the cache. Returns `true` if
    /// a preview actually landed (caller should redraw). Stale-skipped
    /// requests (worker sent `None`) don't count — nothing changed.
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok((path, res)) = self.res_rx.try_recv() {
            self.in_flight.remove(&path);
            if let Some((mtime, kind)) = res {
                self.insert_result(path, mtime, kind);
                got = true;
            }
        }
        got
    }

    /// Drop cache entries whose parent is `dir` — one fs-event's worth.
    /// Cheap: a filtered retain over the map, no I/O.
    pub fn invalidate_dir(&mut self, dir: &Path) {
        let mut freed = 0usize;
        self.map.retain(|p, (_, kind)| {
            let keep = p.parent() != Some(dir);
            if !keep {
                freed += kind.approx_bytes();
            }
            keep
        });
        self.total_bytes = self.total_bytes.saturating_sub(freed);
        self.order.retain(|p| p.parent() != Some(dir));
    }

    /// Drop everything. Used after a child process (editor, `!`, a
    /// `[commands]` shell-out) had the terminal and may have rewritten
    /// anything — cheaper and safer than trying to enumerate what changed.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.total_bytes = 0;
    }

    /// Insert a freshly-rendered result and enforce the cap/byte budget.
    /// Shared by `drain()` and (in tests) direct injection.
    fn insert_result(&mut self, path: PathBuf, mtime: SystemTime, kind: PreviewKind) {
        let kind = Arc::new(kind);
        let bytes = kind.approx_bytes();
        match self.map.insert(path.clone(), (mtime, kind)) {
            Some((_, old)) => {
                self.total_bytes = (self.total_bytes + bytes).saturating_sub(old.approx_bytes());
            }
            None => {
                self.order.push_back(path);
                self.total_bytes += bytes;
            }
        }
        // FIFO evict while over budget, but never evict the entry we just
        // inserted (guarded by `order.len() > 1`) — a single oversized
        // entry is allowed to exceed the budget rather than never cache.
        while (self.total_bytes > self.byte_budget || self.order.len() > self.cap)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front() {
                if let Some((_, k)) = self.map.remove(&old) {
                    self.total_bytes = self.total_bytes.saturating_sub(k.approx_bytes());
                }
            }
        }
    }
}

#[cfg(test)]
impl PreviewCache {
    /// Test-only: same as `with_cap` but with an explicit byte budget, so
    /// eviction tests don't need to synthesize hundreds of MiB of content.
    fn with_cap_and_budget(cap: usize, byte_budget: usize) -> Self {
        let mut c = Self::with_cap(cap);
        c.byte_budget = byte_budget;
        c
    }

    /// Test-only hook: insert a result directly, bypassing the worker, so
    /// byte-budget eviction can be exercised with synthetic sizes instead
    /// of real (multi-hundred-MiB) files.
    fn insert_for_test(&mut self, path: PathBuf, kind: PreviewKind) {
        self.insert_result(path, SystemTime::UNIX_EPOCH, kind);
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

    /// Poll `drain()` until `path` is cached (or a deadline passes).
    fn drain_until_cached(c: &mut PreviewCache, path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !c.contains(path) && std::time::Instant::now() < deadline {
            c.drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn cache_returns_same_value_twice() {
        let d = tempdir().unwrap();
        let f = d.path().join("c.txt");
        fs::write(&f, "x").unwrap();
        let mut c = PreviewCache::new();
        // First request: not cached, work dispatched off-thread. get()
        // returns nothing until drain() picks up the worker's reply.
        c.request(&f, 1024);
        assert!(c.get(&f).is_none());
        drain_until_cached(&mut c, &f);
        // Now cached — two consecutive get()s return the same value (an
        // Arc clone, not a re-render).
        let a = c.get(&f).expect("should be cached");
        let b = c.get(&f).expect("should still be cached");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn request_is_async_then_drains_into_cache() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        let mut c = PreviewCache::new();
        c.request(&f, 1024);
        assert!(c.get(&f).is_none());
        drain_until_cached(&mut c, &f);
        assert!(matches!(c.get(&f).as_deref(), Some(PreviewKind::Text(_))));
    }

    #[test]
    fn request_never_stats_on_caller_thread() {
        // structural: request() on a nonexistent path must not error and
        // must resolve via drain() to a Meta("missing"-style) entry
        // produced by the worker, not by the caller stat-ing anything.
        let mut c = PreviewCache::with_cap(8);
        let missing = Path::new("/nonexistent/x");
        c.request(missing, 1024);
        drain_until_cached(&mut c, missing);
        assert!(c.contains(missing));
        assert!(matches!(c.get(missing).as_deref(), Some(PreviewKind::Meta(_))));
    }

    #[test]
    fn eviction_is_byte_bounded() {
        // 10 entries at ~1MiB each against a 3MiB budget: only the most
        // recent few survive, and total_bytes never exceeds the budget.
        let mut c = PreviewCache::with_cap_and_budget(100, 3 * 1024 * 1024);
        for i in 0..10 {
            let path = PathBuf::from(format!("/fake/{i}.txt"));
            c.insert_for_test(path, PreviewKind::Meta("x".repeat(1024 * 1024)));
        }
        assert!(c.total_bytes <= 3 * 1024 * 1024, "total_bytes = {}", c.total_bytes);
        assert!(c.order.len() < 10, "old entries should have been evicted");
        // Most recent entry always survives.
        assert!(c.contains(Path::new("/fake/9.txt")));
        // Oldest entries were evicted.
        assert!(!c.contains(Path::new("/fake/0.txt")));
    }

    #[test]
    fn eviction_never_drops_a_lone_oversized_entry() {
        // A single entry bigger than the whole budget is still cached —
        // the eviction loop refuses to evict the only entry.
        let mut c = PreviewCache::with_cap_and_budget(100, 1024);
        c.insert_for_test(PathBuf::from("/fake/big.txt"), PreviewKind::Meta("x".repeat(4096)));
        assert!(c.contains(Path::new("/fake/big.txt")));
    }

    #[test]
    fn invalidate_dir_drops_children() {
        let mut c = PreviewCache::with_cap(8);
        c.insert_for_test(PathBuf::from("/tmp/d/f"), PreviewKind::Meta("f".into()));
        c.insert_for_test(PathBuf::from("/tmp/d/g"), PreviewKind::Meta("g".into()));
        c.insert_for_test(PathBuf::from("/tmp/other/h"), PreviewKind::Meta("h".into()));
        assert!(c.contains(Path::new("/tmp/d/f")));
        c.invalidate_dir(Path::new("/tmp/d"));
        assert!(!c.contains(Path::new("/tmp/d/f")));
        assert!(!c.contains(Path::new("/tmp/d/g")));
        // Untouched sibling directory survives.
        assert!(c.contains(Path::new("/tmp/other/h")));
    }

    #[test]
    fn clear_drops_everything() {
        let mut c = PreviewCache::with_cap(8);
        c.insert_for_test(PathBuf::from("/tmp/a"), PreviewKind::Meta("a".into()));
        c.insert_for_test(PathBuf::from("/tmp/b"), PreviewKind::Meta("b".into()));
        c.clear();
        assert!(!c.contains(Path::new("/tmp/a")));
        assert!(!c.contains(Path::new("/tmp/b")));
        assert_eq!(c.total_bytes, 0);
    }

    #[test]
    fn stale_request_is_skipped_by_worker() {
        // Simulates cursor-hold: request A, then immediately move `want`
        // to B before A is dequeued. A's worker response must still
        // arrive (as None, clearing in_flight) without rendering it.
        let d = tempdir().unwrap();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let mut c = PreviewCache::with_cap(8);
        c.request(&a, 1024);
        c.request(&b, 1024); // moves `want` to b before the worker gets to a
        drain_until_cached(&mut c, &b);
        // Give the worker a moment to also process (and skip) a's request.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while c.in_flight.contains(&a) && std::time::Instant::now() < deadline {
            c.drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!c.in_flight.contains(&a), "stale request must still clear in_flight");
        assert!(c.contains(&b));
    }
}
