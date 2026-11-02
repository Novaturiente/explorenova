//! Sidebar row model shared by navigation, the Enter/pin key handlers, and
//! the bookmarks-pane renderer. Three sections stacked in one flat,
//! selectable list: auto-detected well-known dirs, user-pinned paths, and
//! mounted disks/shares.

use crate::app::App;
use crate::devices::Device;
use crate::tree::NodeKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Bookmarks,
    Pinned,
    Disks,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Bookmarks => "bookmarks",
            Section::Pinned => "pinned",
            Section::Disks => "disks",
        }
    }
}

pub struct Row {
    pub section: Section,
    pub label: String,
    pub path: Option<PathBuf>,
    pub device: Option<Device>,
    /// Classification captured here, at build time, so the renderer can
    /// pick an icon and colour without touching the filesystem — and so
    /// it picks the SAME ones the tree does for that path.
    pub node_kind: NodeKind,
    pub exec: bool,
}

const WELL_KNOWN_NAMES: &[&str] =
    &["Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos"];

/// `$HOME` plus whichever standard subdirs exist. Fixed English names —
/// no `user-dirs.dirs` parsing, so a localized `$HOME` layout won't match.
pub fn well_known_dirs() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return out;
    };
    if home.is_dir() {
        out.push(("Home".to_string(), home.clone()));
    }
    for name in WELL_KNOWN_NAMES {
        let p = home.join(name);
        if p.is_dir() {
            out.push((name.to_string(), p));
        }
    }
    out
}

/// Same rules as `Arena::classify`: lstat first so a symlink stays a
/// symlink, then the exec bit. The sidebar used to derive the kind from
/// `is_dir()` alone and pass `exec: false`, so one file could show two
/// different icons depending on which pane you looked at.
fn classify_path(p: &Path) -> (NodeKind, bool) {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return (NodeKind::File, false);
    };
    if m.file_type().is_symlink() {
        return (NodeKind::Symlink, false);
    }
    if m.is_dir() {
        return (NodeKind::Dir, false);
    }
    #[cfg(unix)]
    let exec = {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let exec = false;
    (NodeKind::File, exec)
}

fn label_for(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| p.display().to_string())
}

/// Flatten Bookmarks ++ Pinned ++ Disks into one selectable list, in that
/// order. Stats each pinned path once (for its label and its icon), so
/// the draw path must call [`build_cached`], not this.
pub fn build(app: &App) -> Vec<Row> {
    let mut rows = Vec::new();
    for (label, path) in &app.well_known {
        rows.push(Row {
            section: Section::Bookmarks,
            label: label.clone(),
            path: Some(path.clone()),
            device: None,
            // well_known_dirs() only yields paths that passed is_dir().
            node_kind: NodeKind::Dir,
            exec: false,
        });
    }
    for p in &app.state.pinned {
        let (node_kind, exec) = classify_path(p);
        rows.push(Row {
            section: Section::Pinned,
            label: crate::ui::display_name(&crate::ui::short_bookmark_label(p)),
            path: Some(p.clone()),
            device: None,
            node_kind,
            exec,
        });
    }
    for d in &app.disks {
        let label = d
            .label
            .clone()
            .or_else(|| d.mountpoint.as_deref().map(label_for))
            .unwrap_or_else(|| d.device.clone());
        rows.push(Row {
            section: Section::Disks,
            label,
            path: d.mountpoint.clone(),
            device: Some(d.clone()),
            node_kind: NodeKind::Dir,
            exec: false,
        });
    }
    rows
}

/// The inputs `build` reads. Cheap to clone and compare (no syscalls),
/// which is the whole trick: comparing this per frame replaces a
/// `symlink_metadata` per row per frame.
type CacheKey = (Vec<(String, PathBuf)>, Vec<PathBuf>, Vec<Device>);

thread_local! {
    static ROW_CACHE: std::cell::RefCell<Option<(CacheKey, Rc<Vec<Row>>)>> =
        const { std::cell::RefCell::new(None) };
}

/// `build`, memoised on its inputs — for the draw path. Running `build`
/// every frame meant a stat per sidebar row per frame, so a single pinned
/// path on a dead NFS/SMB mount blocked the whole render loop until the
/// mount timed out.
///
/// ponytail: thread_local rather than a field on `App` because rendering
/// is single-threaded; move it onto `App` if a second render thread ever
/// appears.
pub fn build_cached(app: &App) -> Rc<Vec<Row>> {
    let key: CacheKey = (
        app.well_known.clone(),
        app.state.pinned.clone(),
        app.disks.clone(),
    );
    ROW_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, rows)) = c.as_ref() {
            if *k == key {
                return rows.clone();
            }
        }
        let rows = Rc::new(build(app));
        *c = Some((key, rows.clone()));
        rows
    })
}

/// (section, start index, row count) for each of the 3 sections, in the
/// same flat-index order `build` produces. Used to jump the cursor to
/// the next section's first row (Shift-Tab).
pub fn section_ranges(rows: &[Row]) -> Vec<(Section, usize, usize)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for s in [Section::Bookmarks, Section::Pinned, Section::Disks] {
        let count = rows.iter().filter(|r| r.section == s).count();
        out.push((s, offset, count));
        offset += count;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::devices::DeviceKind;
    use crate::state::State;
    use crate::tree::Arena;

    fn test_app() -> App {
        let d = tempfile::tempdir().unwrap();
        let arena = Arena::new(d.path()).unwrap();
        App::new(arena, Config::with_defaults(), State::default())
    }

    #[test]
    fn empty_app_has_no_pinned_or_disk_rows() {
        let app = test_app();
        let rows = build(&app);
        assert!(rows.iter().all(|r| r.section != Section::Pinned));
        assert!(rows.iter().all(|r| r.section != Section::Disks));
    }

    #[test]
    fn cached_build_reuses_rows_until_an_input_changes() {
        let mut app = test_app();
        app.state.pinned = vec![PathBuf::from("/pinned/a")];
        let a = build_cached(&app);
        let b = build_cached(&app);
        assert!(Rc::ptr_eq(&a, &b), "unchanged inputs must not re-stat");
        app.state.pinned.push(PathBuf::from("/pinned/b"));
        let c = build_cached(&app);
        assert!(!Rc::ptr_eq(&a, &c), "changed inputs must rebuild");
        assert_eq!(c.iter().filter(|r| r.section == Section::Pinned).count(), 2);
    }

    #[test]
    fn pinned_rows_classify_like_the_tree() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sub");
        std::fs::create_dir(&dir).unwrap();
        let link = d.path().join("lnk");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let mut app = test_app();
        app.state.pinned = vec![dir.clone(), link.clone()];
        let rows = build(&app);
        let pinned: Vec<&Row> = rows.iter().filter(|r| r.section == Section::Pinned).collect();
        assert_eq!(pinned[0].node_kind, NodeKind::Dir);
        // is_dir() follows the link and would have called this a Dir.
        assert_eq!(pinned[1].node_kind, NodeKind::Symlink);
    }

    #[test]
    fn sections_appear_in_order() {
        let mut app = test_app();
        app.well_known = vec![("Home".to_string(), PathBuf::from("/home/x"))];
        app.state.pinned = vec![PathBuf::from("/pinned/a")];
        app.disks = vec![Device {
            device: "/dev/sdb1".into(),
            label: Some("Backup".into()),
            size: None,
            fstype: None,
            mountpoint: Some(PathBuf::from("/mnt/backup")),
            kind: DeviceKind::Block,
        }];
        let rows = build(&app);
        let sections: Vec<Section> = rows.iter().map(|r| r.section).collect();
        assert_eq!(
            sections,
            vec![Section::Bookmarks, Section::Pinned, Section::Disks]
        );
        assert_eq!(rows[2].label, "Backup");
        assert_eq!(rows[2].path, Some(PathBuf::from("/mnt/backup")));
    }
}
