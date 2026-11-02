//! Footer metadata panel. Cheap `lstat` facts are read on the UI thread once
//! per cursor move; slow extras (folder size, exiftool tags) come from one
//! background worker so a big folder or a slow tool never blocks the loop.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub type Rows = Vec<(String, String)>;

/// Folder walks stop after this long and report a lower bound.
const WALK_BUDGET: Duration = Duration::from_secs(10);
/// Tags asked from exiftool; anything the file lacks is simply absent.
const EXIF_TAGS: &[&str] = &[
    "-ImageSize", "-Duration", "-Make", "-Model", "-LensModel", "-DateTimeOriginal",
    "-ExposureTime", "-FNumber", "-ISO", "-FocalLength", "-GPSPosition", "-Artist",
    "-Title", "-AvgBitrate", "-AudioSampleRate", "-PageCount",
];

pub struct MetaCache {
    basic: Option<(PathBuf, Rows)>,
    extra: HashMap<PathBuf, Rows>,
    in_flight: HashSet<PathBuf>,
    req_tx: mpsc::Sender<PathBuf>,
    res_rx: mpsc::Receiver<(PathBuf, Option<Rows>)>,
    exiftool: bool,
}

impl MetaCache {
    pub fn new(exiftool: bool) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(mut path) = req_rx.recv() {
                // Only the newest request matters; answer the skipped ones
                // with `None` so the UI stops counting them as in flight.
                while let Ok(newer) = req_rx.try_recv() {
                    let _ = res_tx.send((std::mem::replace(&mut path, newer), None));
                }
                let rows = extra_rows(&path, exiftool);
                if res_tx.send((path, Some(rows))).is_err() {
                    break;
                }
            }
        });
        MetaCache {
            basic: None,
            extra: HashMap::new(),
            in_flight: HashSet::new(),
            req_tx,
            res_rx,
            exiftool,
        }
    }

    /// Stat facts for `path`, recomputed only when the path changes.
    pub fn basic(&mut self, path: &Path) -> &Rows {
        if self.basic.as_ref().map(|(p, _)| p.as_path()) != Some(path) {
            self.basic = Some((path.to_path_buf(), basic_rows(path)));
        }
        &self.basic.as_ref().unwrap().1
    }

    pub fn extra(&self, path: &Path) -> Option<&Rows> {
        self.extra.get(path)
    }

    /// Does `path` have slow extras worth computing?
    pub fn wants_extra(&self, path: &Path, is_dir: bool) -> bool {
        is_dir || (self.exiftool && is_exif_candidate(path))
    }

    pub fn is_loading(&self, path: &Path) -> bool {
        self.in_flight.contains(path)
    }

    /// Queue the slow extras for `path` unless cached or already queued.
    pub fn request(&mut self, path: &Path) {
        if self.extra.contains_key(path) || self.in_flight.contains(path) {
            return;
        }
        self.in_flight.insert(path.to_path_buf());
        let _ = self.req_tx.send(path.to_path_buf());
    }

    /// Pull finished worker results in. True when something new landed.
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok((path, rows)) = self.res_rx.try_recv() {
            self.in_flight.remove(&path);
            if let Some(rows) = rows {
                // shortcut: unbounded-growth guard is a full clear at 512
                // entries; an LRU only matters if that ever shows up.
                if self.extra.len() >= 512 {
                    self.extra.clear();
                }
                self.extra.insert(path, rows);
                got = true;
            }
        }
        got
    }

    /// Something on disk changed (op finished, fs event): drop it all.
    pub fn invalidate(&mut self) {
        self.basic = None;
        self.extra.clear();
    }
}

fn is_exif_candidate(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    crate::icons::classify(crate::tree::NodeKind::File, false, &name) == crate::icons::Kind::Media
        || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

#[cfg(unix)]
fn basic_rows(path: &Path) -> Rows {
    use std::os::unix::fs::MetadataExt;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let mut rows: Rows = vec![("Name".into(), crate::ui::display_name(&name))];
    let m = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            rows.push(("Error".into(), e.to_string()));
            return rows;
        }
    };
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
        'l'
    } else if ft.is_dir() {
        'd'
    } else {
        '-'
    };
    if !ft.is_dir() {
        rows.push(("Size".into(), crate::status::human_bytes(m.len())));
    }
    rows.push(("Mode".into(), crate::status::perms_string(kind, m.mode())));
    rows.push(("Owner".into(), format!("{}:{}", user_name(m.uid()), group_name(m.gid()))));
    let at = |s: i64| std::time::UNIX_EPOCH + Duration::from_secs(s.max(0) as u64);
    rows.push(("Modified".into(), crate::status::format_datetime(at(m.mtime()))));
    rows.push(("Changed".into(), crate::status::format_datetime(at(m.ctime()))));
    if ft.is_symlink() {
        let target = std::fs::read_link(path)
            .map(|t| t.display().to_string())
            .unwrap_or_else(|e| e.to_string());
        rows.push(("Target".into(), crate::ui::display_name(&target)));
    } else if ft.is_file() {
        if let Some(mime) = mime_guess::from_path(path).first() {
            rows.push(("Type".into(), mime.essence_str().to_string()));
        }
    }
    rows
}

#[cfg(not(unix))]
fn basic_rows(path: &Path) -> Rows {
    vec![("Name".into(), path.display().to_string())]
}

// shortcut: getpwuid/getgrgid return static buffers, so these are only
// called from the UI thread (basic_rows); never from the meta worker.
#[cfg(unix)]
fn user_name(uid: u32) -> String {
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return uid.to_string();
    }
    unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) }.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn group_name(gid: u32) -> String {
    let gr = unsafe { libc::getgrgid(gid) };
    if gr.is_null() {
        return gid.to_string();
    }
    unsafe { std::ffi::CStr::from_ptr((*gr).gr_name) }.to_string_lossy().into_owned()
}

/// Worker side: folder size for a dir, exiftool tags for media.
fn extra_rows(path: &Path, exiftool: bool) -> Rows {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {
            let (bytes, items, complete) = walk_size(path, WALK_BUDGET);
            let prefix = if complete { "" } else { "≥ " };
            vec![
                ("Folder size".into(), format!("{prefix}{}", crate::status::human_bytes(bytes))),
                ("Items".into(), format!("{prefix}{items}")),
            ]
        }
        Ok(_) if exiftool && is_exif_candidate(path) => exif_rows(path),
        _ => Vec::new(),
    }
}

/// Total bytes and entry count under `root`, never following symlinks.
/// Returns `complete == false` when the time budget ran out.
pub fn walk_size(root: &Path, budget: Duration) -> (u64, u64, bool) {
    let t0 = Instant::now();
    let (mut bytes, mut items) = (0u64, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if t0.elapsed() > budget {
                return (bytes, items, false);
            }
            let Ok(m) = e.metadata() else { continue }; // DirEntry::metadata = lstat
            items += 1;
            if m.is_dir() {
                stack.push(e.path());
            } else {
                bytes += m.len();
            }
        }
    }
    (bytes, items, true)
}

fn exif_rows(path: &Path) -> Rows {
    let mut cmd = std::process::Command::new("exiftool");
    cmd.arg("-S").arg("-fast").args(EXIF_TAGS).arg("--").arg(path);
    let Some((st, out)) =
        crate::preview::media::capture_with_timeout(&mut cmd, Duration::from_secs(10), 64 * 1024)
    else {
        return Vec::new(); // missing exiftool or timeout: panel just shows the basics
    };
    if !st.success() {
        return Vec::new();
    }
    parse_exiftool(&String::from_utf8_lossy(&out))
}

/// `exiftool -S` prints `TagName: value` per line.
fn parse_exiftool(out: &str) -> Rows {
    out.lines()
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim().to_string(), crate::ui::display_name(v.trim())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_size_counts_files_and_dirs_without_following_links() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("a"), vec![0u8; 100]).unwrap();
        std::fs::write(d.path().join("sub/b"), vec![0u8; 50]).unwrap();
        std::os::unix::fs::symlink("/", d.path().join("root-link")).unwrap();
        let (bytes, items, complete) = walk_size(d.path(), Duration::from_secs(5));
        assert!(complete);
        assert_eq!(items, 4); // a, sub, sub/b, root-link
        assert_eq!(bytes, 150 + std::fs::symlink_metadata(d.path().join("root-link")).unwrap().len());
    }

    #[test]
    fn parses_exiftool_short_output() {
        let rows = parse_exiftool("ImageSize: 4000x3000\nModel: X100V\njunk line\n");
        assert_eq!(
            rows,
            vec![
                ("ImageSize".to_string(), "4000x3000".to_string()),
                ("Model".to_string(), "X100V".to_string()),
            ]
        );
    }

    #[test]
    fn worker_answers_and_cache_fills() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), "xy").unwrap();
        let mut m = MetaCache::new(false);
        m.request(d.path());
        let t0 = Instant::now();
        while !m.drain() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let rows = m.extra(d.path()).expect("folder size computed");
        assert_eq!(rows[0], ("Folder size".to_string(), "2B".to_string()));
        assert!(!m.is_loading(d.path()));
    }
}
