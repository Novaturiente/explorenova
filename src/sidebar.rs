//! Sidebar row model shared by navigation, the Enter/pin key handlers, and
//! the bookmarks-pane renderer. Three sections stacked in one flat,
//! selectable list: auto-detected well-known dirs, user-pinned paths, and
//! mounted disks/shares.

use crate::app::App;
use crate::devices::Device;
use std::path::{Path, PathBuf};

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

fn label_for(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| p.display().to_string())
}

/// Flatten Bookmarks ++ Pinned ++ Disks into one selectable list, in that
/// order. No I/O — reads `app.well_known`/`app.disks` (both cached) and
/// `app.state.pinned`.
pub fn build(app: &App) -> Vec<Row> {
    let mut rows = Vec::new();
    for (label, path) in &app.well_known {
        rows.push(Row {
            section: Section::Bookmarks,
            label: label.clone(),
            path: Some(path.clone()),
            device: None,
        });
    }
    for p in &app.state.pinned {
        rows.push(Row {
            section: Section::Pinned,
            label: crate::ui::display_name(&crate::ui::short_bookmark_label(p)),
            path: Some(p.clone()),
            device: None,
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
        });
    }
    rows
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
