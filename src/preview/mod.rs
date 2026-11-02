pub mod archive;
pub mod media;
pub mod text;
pub mod xlsx;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

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

/// Default largest file handed to an in-process heavy decoder (image
/// crate, calamine) when no config value is supplied (tests, and any
/// caller that doesn't care). Images are further bounded by
/// `decode_image_bounded`'s pixel limits. Configurable via
/// `[preview] heavy_max_bytes` — see `config::PreviewConfig`.
pub const HEAVY_FILE_MAX_DEFAULT: u64 = 64 * 1024 * 1024;

/// Decode with explicit limits so a decompression-bomb PNG can't OOM
/// the process. Returns None on unreadable/oversized images.
///
/// Sniffs the format from the file's magic bytes (`with_guessed_format`)
/// instead of trusting the extension `ImageReader::open` would otherwise
/// pick — a PNG saved/downloaded with a `.jpg` name is common enough
/// (browsers, screenshot tools) that trusting the extension made those
/// files fail to decode and fall back to the misleading
/// "(binary or oversized)" message.
fn decode_image_bounded(path: &Path) -> Option<image::DynamicImage> {
    let reader = image::ImageReader::open(path).ok()?;
    let mut reader = reader.with_guessed_format().ok()?;
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

/// Decide preview kind for `path` (no caching). `heavy_max_bytes` bounds
/// raw image decode + xlsx parse (see `HEAVY_FILE_MAX_DEFAULT`);
/// `max_bytes` bounds plain-text preview size.
pub fn render(path: &Path, max_bytes: u64, heavy_max_bytes: u64) -> PreviewKind {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return PreviewKind::Meta(format!("error: {e}")),
    };
    let ft = meta.file_type();
    if ft.is_dir() {
        return PreviewKind::Meta(format!("directory\n{} entries", count_entries(path)));
    }
    // Anything that isn't a regular file must never reach a reader: a
    // FIFO blocks `std::fs::read` until someone writes, and /dev/zero
    // never ends. There is ONE preview worker thread, so a single one of
    // these (just arrowing through /dev or /run) kills every later
    // preview for the rest of the session.
    if !ft.is_file() {
        use std::os::unix::fs::FileTypeExt;
        let what = if ft.is_fifo() {
            "fifo"
        } else if ft.is_socket() {
            "socket"
        } else if ft.is_block_device() {
            "block device"
        } else if ft.is_char_device() {
            "character device"
        } else {
            "special file"
        };
        return PreviewKind::Meta(format!("{}\n({what})", path.display()));
    }
    let ext = ext_lower(path);
    if IMAGE_EXTS.contains(&ext.as_str()) {
        if meta.len() > heavy_max_bytes {
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
        if meta.len() > heavy_max_bytes {
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
/// stats the file itself when it dequeues a request.
///
/// Freshness is the worker's job too. External invalidation
/// (`invalidate_path`/`invalidate_dir`/`clear`) only covers what the
/// fs-watcher sees, and watches are dropped when a directory is
/// collapsed — so a file edited while its directory was collapsed used
/// to serve its pre-edit preview forever. Every `request` therefore
/// carries the mtime the cached entry was rendered at, and the worker
/// re-renders when the file's current mtime differs. The caller still
/// never stats.
///
/// ponytail: the revalidation stat rides the existing per-frame
/// `request` (so ~5/s while idle, on the worker thread). If that ever
/// shows up in a profile, gate it on a minimum age per entry.
pub struct PreviewCache {
    /// Value is the preview plus the mtime it was rendered at
    /// (`UNIX_EPOCH` when the file could not be stat'd).
    map: HashMap<PathBuf, (Arc<PreviewKind>, std::time::SystemTime)>,
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
    req_tx: Sender<(PathBuf, u64, u64, Option<std::time::SystemTime>)>,
    res_rx: Receiver<(PathBuf, Option<(PreviewKind, std::time::SystemTime)>)>,
}

/// mtime of `p`, or `UNIX_EPOCH` when it can't be read (missing file,
/// permission denied). A sentinel rather than an `Option` so "still
/// unreadable" compares equal and doesn't re-render on every frame.
fn mtime_of(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
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
        let (req_tx, req_rx) =
            mpsc::channel::<(PathBuf, u64, u64, Option<std::time::SystemTime>)>();
        let (res_tx, res_rx) =
            mpsc::channel::<(PathBuf, Option<(PreviewKind, std::time::SystemTime)>)>();
        let want: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let want_worker = want.clone();
        std::thread::spawn(move || {
            while let Ok((path, max_bytes, heavy_max_bytes, known_mtime)) = req_rx.recv() {
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
                // Revalidate: the caller already has a preview for this
                // path and only wants a new one if the file moved on.
                let mtime = mtime_of(&path);
                if known_mtime == Some(mtime) {
                    let _ = res_tx.send((path, None));
                    continue;
                }
                let kind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render(&path, max_bytes, heavy_max_bytes)
                }))
                .unwrap_or_else(|_| PreviewKind::Meta(format!(
                    "{}\n(preview failed)",
                    path.display()
                )));
                let _ = res_tx.send((path, Some((kind, mtime))));
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
        self.map.get(path).map(|(k, _)| k.clone())
    }

    /// Queue a render for `path` if it isn't already cached or in flight.
    /// Fire-and-forget: the worker does the stat and the render off
    /// thread; poll `drain()` and then `get()` to pick up the result.
    /// `heavy_max_bytes` bounds raw image/xlsx decode size — pass
    /// `HEAVY_FILE_MAX_DEFAULT` if the caller has no configured value.
    pub fn request(&mut self, path: &Path, max_bytes: u64, heavy_max_bytes: u64) {
        if let Ok(mut w) = self.want.lock() {
            *w = Some(path.to_path_buf());
        }
        if self.in_flight.contains(path) {
            return;
        }
        // A cached entry still gets queued, but as a revalidation: the
        // worker compares `known` against the file's current mtime and
        // only re-renders on a mismatch.
        let known = self.map.get(path).map(|(_, t)| *t);
        self.in_flight.insert(path.to_path_buf());
        let _ = self
            .req_tx
            .send((path.to_path_buf(), max_bytes, heavy_max_bytes, known));
    }

    /// Move any completed worker results into the cache. Returns `true` if
    /// a preview actually landed (caller should redraw). Stale-skipped
    /// requests (worker sent `None`) don't count — nothing changed.
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok((path, res)) = self.res_rx.try_recv() {
            self.in_flight.remove(&path);
            if let Some((kind, mtime)) = res {
                self.insert_result(path, kind, mtime);
                got = true;
            }
        }
        got
    }

    /// Drop the entry for exactly `path`. What an fs-watch event actually
    /// justifies: one file changed, so only that file's preview is stale.
    pub fn invalidate_path(&mut self, path: &Path) {
        if let Some((k, _)) = self.map.remove(path) {
            self.total_bytes = self.total_bytes.saturating_sub(k.approx_bytes());
            self.order.retain(|p| p != path);
        }
    }

    /// Drop cache entries whose parent is `dir` — one fs-event's worth.
    /// Cheap: a filtered retain over the map, no I/O.
    pub fn invalidate_dir(&mut self, dir: &Path) {
        let mut freed = 0usize;
        self.map.retain(|p, (kind, _)| {
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
    fn insert_result(
        &mut self,
        path: PathBuf,
        kind: PreviewKind,
        mtime: std::time::SystemTime,
    ) {
        let kind = Arc::new(kind);
        let bytes = kind.approx_bytes();
        match self.map.insert(path.clone(), (kind, mtime)) {
            Some((old, _)) => {
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
                if let Some((k, _)) = self.map.remove(&old) {
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
        self.insert_result(path, kind, std::time::UNIX_EPOCH);
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
        match render(&f, 1024, HEAVY_FILE_MAX_DEFAULT) {
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
        assert!(matches!(render(&f, 100, HEAVY_FILE_MAX_DEFAULT), PreviewKind::Meta(_)));
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
        assert!(matches!(render(&f, 1024, HEAVY_FILE_MAX_DEFAULT), PreviewKind::Image(..)));
    }

    #[test]
    fn image_over_heavy_max_bytes_falls_back_to_meta() {
        let d = tempdir().unwrap();
        let f = d.path().join("p.png");
        image::RgbaImage::new(1, 1).save(&f).unwrap();
        // A tiny heavy_max_bytes forces the size-gate even for a 1x1 PNG,
        // proving the threshold is the caller's value, not the hardcoded
        // HEAVY_FILE_MAX_DEFAULT.
        assert!(matches!(render(&f, 1024, 1), PreviewKind::Meta(_)));
        assert!(matches!(render(&f, 1024, HEAVY_FILE_MAX_DEFAULT), PreviewKind::Image(..)));
    }

    #[test]
    fn png_content_with_jpg_extension_still_decodes() {
        // Downloaded/screenshotted images are routinely saved with a
        // mismatched extension; the decoder must sniff actual content,
        // not trust the extension, or this falls back to Meta.
        let d = tempdir().unwrap();
        let f = d.path().join("mislabeled.jpg");
        image::RgbaImage::new(4, 4).save_with_format(&f, image::ImageFormat::Png).unwrap();
        assert!(matches!(render(&f, 1024, HEAVY_FILE_MAX_DEFAULT), PreviewKind::Image(..)));
    }

    #[test]
    fn fifo_returns_promptly_instead_of_blocking_the_worker() {
        // Regression guard: `render` used to fall through to
        // `std::fs::read`, which blocks forever on a FIFO with no writer
        // and takes the single preview worker thread down with it. Run
        // it on a side thread so a regression FAILS here rather than
        // hanging the test binary.
        let d = tempdir().unwrap();
        let f = d.path().join("pipe");
        let c = std::ffi::CString::new(f.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo failed");

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(render(&f, 1024, HEAVY_FILE_MAX_DEFAULT));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(PreviewKind::Meta(m)) => assert!(m.contains("fifo"), "got {m:?}"),
            Ok(other) => panic!("expected Meta, got {other:?}"),
            Err(_) => panic!("render() blocked on a FIFO"),
        }
    }

    #[test]
    fn corrupt_image_falls_back_to_meta() {
        let d = tempdir().unwrap();
        let f = d.path().join("bad.png");
        fs::write(&f, [0u8; 8]).unwrap();
        assert!(matches!(render(&f, 1024, HEAVY_FILE_MAX_DEFAULT), PreviewKind::Meta(_)));
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
        c.request(&f, 1024, HEAVY_FILE_MAX_DEFAULT);
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
        c.request(&f, 1024, HEAVY_FILE_MAX_DEFAULT);
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
        c.request(missing, 1024, HEAVY_FILE_MAX_DEFAULT);
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
    fn invalidate_path_spares_siblings() {
        // An fs event for one file must not evict the preview the user is
        // currently looking at (its sibling) — that flashed "(loading…)"
        // on every unrelated write in the directory.
        let mut c = PreviewCache::with_cap(8);
        c.insert_for_test(PathBuf::from("/tmp/d/noise.log"), PreviewKind::Meta("n".into()));
        c.insert_for_test(PathBuf::from("/tmp/d/viewed.rs"), PreviewKind::Meta("v".into()));
        c.invalidate_path(Path::new("/tmp/d/noise.log"));
        assert!(!c.contains(Path::new("/tmp/d/noise.log")));
        assert!(c.contains(Path::new("/tmp/d/viewed.rs")));
        assert_eq!(c.total_bytes, 1);
        assert_eq!(c.order.len(), 1);
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

    /// Flatten a cached Text preview back to a plain string.
    fn cached_text(c: &PreviewCache, p: &Path) -> String {
        match c.get(p).as_deref() {
            Some(PreviewKind::Text(t)) => t
                .lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                .collect(),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn edit_missed_by_the_watcher_still_refreshes() {
        // A file edited while its directory was collapsed gets no
        // fs-watch event, so no invalidate_* call ever arrives. The
        // mtime carried on the cache entry is the only thing that can
        // catch it — without it the pre-edit preview is served forever.
        let d = tempdir().unwrap();
        let f = d.path().join("watched.txt");
        fs::write(&f, "before").unwrap();
        let mut c = PreviewCache::with_cap(8);
        c.request(&f, 1024, HEAVY_FILE_MAX_DEFAULT);
        drain_until_cached(&mut c, &f);
        assert!(cached_text(&c, &f).contains("before"));

        fs::write(&f, "after").unwrap();
        // Force a visibly different mtime — the write above may land in
        // the same filesystem timestamp tick as the first one.
        fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() + std::time::Duration::from_secs(5),
            )
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !cached_text(&c, &f).contains("after")
            && std::time::Instant::now() < deadline
        {
            c.request(&f, 1024, HEAVY_FILE_MAX_DEFAULT);
            c.drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(cached_text(&c, &f).contains("after"), "stale preview served");
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
        c.request(&a, 1024, HEAVY_FILE_MAX_DEFAULT);
        c.request(&b, 1024, HEAVY_FILE_MAX_DEFAULT); // moves `want` to b before the worker gets to a
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
