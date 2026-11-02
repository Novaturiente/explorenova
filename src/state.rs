use crate::sort::SortState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PaneOverride {
    pub visible: Option<bool>,
    pub width: Option<u16>,
    pub weight: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    #[serde(alias = "bookmarks")]
    pub pinned: Vec<PathBuf>,
    pub last_cwd: Option<PathBuf>,
    pub pane_order: Option<Vec<String>>,
    pub bookmarks_pane: PaneOverride,
    pub tree_pane: PaneOverride,
    pub preview_pane: PaneOverride,
    pub sort: SortState,
    pub line_mode: crate::linemode::LineMode,
}

impl State {
    pub fn parse(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }

    /// Serialize for `state.toml`, or `None` if it cannot be represented.
    ///
    /// `PathBuf` refuses to serialize when the path is not valid UTF-8, and
    /// this used to be `unwrap_or_default()`, so quitting inside a directory
    /// with non-UTF-8 bytes in its name wrote an empty string over the real
    /// file and destroyed every pin, the pane layout and the sort settings.
    /// Paths that cannot round-trip are dropped here instead, so one odd
    /// directory costs one entry rather than the whole file, and the caller
    /// skips the write entirely if anything else goes wrong.
    pub fn to_toml(&self) -> Option<String> {
        let mut clean = self.clone();
        clean.pinned.retain(|p| p.to_str().is_some());
        clean.last_cwd = clean.last_cwd.take().filter(|p| p.to_str().is_some());
        toml::to_string_pretty(&clean).ok()
    }

    /// Drop pins whose path no longer exists. Only a definite `NotFound`
    /// removes one: EACCES/ESTALE/EIO mean "can't tell right now". Pins under
    /// removable-mount roots are kept: an unplugged drive or unmounted share
    /// is absent, not gone, and would lose its pins on every launch without it.
    pub fn prune_missing_pins(&mut self) {
        const MOUNT_ROOTS: &[&str] = &["/run/media", "/media", "/mnt", "/run/user"];
        self.pinned.retain(|p| {
            MOUNT_ROOTS.iter().any(|r| p.starts_with(r))
                || !matches!(
                    std::fs::symlink_metadata(p),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound
                )
        });
    }

    pub fn toggle_pin(&mut self, p: PathBuf) {
        if let Some(i) = self.pinned.iter().position(|b| b == &p) {
            self.pinned.remove(i);
        } else {
            self.pinned.push(p);
        }
    }

    /// Snapshot the live pane layout into this state (for persistence).
    pub fn capture_panes(&mut self, p: &crate::config::PanesConfig) {
        self.pane_order = Some(p.order.clone());
        self.bookmarks_pane = PaneOverride {
            visible: Some(p.bookmarks.visible),
            width: p.bookmarks.width,
            weight: Some(p.bookmarks.weight),
        };
        self.tree_pane = PaneOverride {
            visible: Some(p.tree.visible),
            width: p.tree.width,
            weight: Some(p.tree.weight),
        };
        self.preview_pane = PaneOverride {
            visible: Some(p.preview.visible),
            width: p.preview.width,
            weight: Some(p.preview.weight),
        };
    }

    /// Merge persisted overrides back onto a (defaults+config) pane layout,
    /// EXCEPT for keys the user wrote
    /// explicitly in `config.toml` (see [`crate::config::pinned_pane_keys`]).
    /// Config wins on a pinned key; state keeps persisting every other one.
    pub fn apply_to(
        &self,
        p: &mut crate::config::PanesConfig,
        pinned: &std::collections::HashSet<String>,
    ) {
        if let Some(o) = &self.pane_order {
            if o.len() == 3 && !pinned.contains("order") {
                p.order = o.clone();
            }
        }
        Self::merge(&self.bookmarks_pane, &mut p.bookmarks, "bookmarks", pinned);
        Self::merge(&self.tree_pane, &mut p.tree, "tree", pinned);
        Self::merge(&self.preview_pane, &mut p.preview, "preview", pinned);
    }

    fn merge(
        ov: &PaneOverride,
        pc: &mut crate::config::PaneConfig,
        pane: &str,
        pinned: &std::collections::HashSet<String>,
    ) {
        let free = |field: &str| !pinned.contains(&format!("{pane}.{field}"));
        if let Some(v) = ov.visible {
            if free("visible") {
                pc.visible = v;
            }
        }
        if ov.width.is_some() && free("width") {
            pc.width = ov.width;
        }
        if let Some(w) = ov.weight {
            if free("weight") {
                pc.weight = w;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_then_removes() {
        let mut s = State::default();
        let p = PathBuf::from("/tmp/x");
        s.toggle_pin(p.clone());
        assert_eq!(s.pinned, vec![p.clone()]);
        s.toggle_pin(p.clone());
        assert!(s.pinned.is_empty());
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut s = State::default();
        s.pinned.push(PathBuf::from("/a"));
        s.last_cwd = Some(PathBuf::from("/b"));
        let back = State::parse(&s.to_toml().unwrap());
        assert_eq!(back, s);
    }

    #[test]
    fn non_utf8_path_drops_one_entry_not_the_file() {
        use std::os::unix::ffi::OsStrExt;
        let mut s = State::default();
        s.pinned.push(PathBuf::from("/good"));
        s.pinned.push(PathBuf::from(std::ffi::OsStr::from_bytes(b"/bad\xff")));
        let out = s.to_toml().expect("must still serialize");
        let back = State::parse(&out);
        assert_eq!(back.pinned, vec![PathBuf::from("/good")]);
    }

    #[test]
    fn prune_drops_missing_keeps_existing_and_mounts() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State {
            pinned: vec![
                d.path().to_path_buf(),
                d.path().join("gone"),
                PathBuf::from("/run/media/nova/USB/x"),
            ],
            ..State::default()
        };
        s.prune_missing_pins();
        assert_eq!(
            s.pinned,
            vec![d.path().to_path_buf(), PathBuf::from("/run/media/nova/USB/x")]
        );
    }

    #[test]
    fn bad_toml_yields_default() {
        let s = State::parse("@@@");
        assert_eq!(s, State::default());
    }

    #[test]
    fn sort_persists_through_toml() {
        use crate::sort::{SortMode, SortState};
        let s = State {
            sort: SortState {
                mode: SortMode::Size,
                reverse: true,
                dir_first: false,
            },
            ..State::default()
        };
        let back = State::parse(&s.to_toml().unwrap());
        assert_eq!(back.sort, s.sort);
    }

    #[test]
    fn line_mode_persists() {
        let s = State { line_mode: crate::linemode::LineMode::Perms, ..State::default() };
        assert_eq!(State::parse(&s.to_toml().unwrap()).line_mode, crate::linemode::LineMode::Perms);
    }

    #[test]
    fn missing_sort_section_yields_default() {
        let s = State::parse("bookmarks = []\n");
        assert_eq!(s.sort, crate::sort::SortState::default());
    }

    #[test]
    fn old_bookmarks_key_aliases_to_pinned() {
        // Pre-rename state.toml files saved the list under `bookmarks`;
        // the alias keeps those entries alive after upgrading.
        let s = State::parse("bookmarks = [\"/tmp/x\"]\n");
        assert_eq!(s.pinned, vec![PathBuf::from("/tmp/x")]);
    }

    #[test]
    fn capture_then_apply_roundtrips_panes() {
        let mut cfg = crate::config::PanesConfig::default();
        cfg.bookmarks.visible = true;
        cfg.bookmarks.width = Some(30);
        cfg.tree.weight = 5;
        cfg.order = vec!["preview".into(), "tree".into(), "bookmarks".into()];
        let mut st = State::default();
        st.capture_panes(&cfg);
        let mut fresh = crate::config::PanesConfig::default();
        st.apply_to(&mut fresh, &std::collections::HashSet::new());
        assert!(fresh.bookmarks.visible);
        assert_eq!(fresh.bookmarks.width, Some(30));
        assert_eq!(fresh.tree.weight, 5);
        assert_eq!(fresh.order[0], "preview");
    }

    #[test]
    fn pinned_config_keys_beat_captured_state() {
        // The regression: capture_panes writes Some for every field on each
        // clean quit, so before this an explicit config.toml value was
        // silently overridden from the second run onward.
        let mut live = crate::config::PanesConfig::default();
        live.bookmarks.visible = true;
        live.tree.weight = 9;
        let mut st = State::default();
        st.capture_panes(&live);

        let src = "[panes.bookmarks]\nvisible = false\n";
        let pinned = crate::config::pinned_pane_keys(src);
        let (cfg, _) = crate::config::Config::load_str(src);
        let mut panes = cfg.panes;
        st.apply_to(&mut panes, &pinned);

        assert!(!panes.bookmarks.visible, "config.toml must win on a pinned key");
        assert_eq!(panes.tree.weight, 9, "unpinned keys still restore from state");
    }
}
