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
    pub bookmarks: Vec<PathBuf>,
    pub last_cwd: Option<PathBuf>,
    pub pane_order: Option<Vec<String>>,
    pub bookmarks_pane: PaneOverride,
    pub tree_pane: PaneOverride,
    pub preview_pane: PaneOverride,
}

impl State {
    pub fn from_str(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn toggle_bookmark(&mut self, p: PathBuf) {
        if let Some(i) = self.bookmarks.iter().position(|b| b == &p) {
            self.bookmarks.remove(i);
        } else {
            self.bookmarks.push(p);
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

    /// Merge persisted overrides back onto a (defaults+config) pane layout.
    pub fn apply_to(&self, p: &mut crate::config::PanesConfig) {
        if let Some(o) = &self.pane_order {
            if o.len() == 3 {
                p.order = o.clone();
            }
        }
        Self::merge(&self.bookmarks_pane, &mut p.bookmarks);
        Self::merge(&self.tree_pane, &mut p.tree);
        Self::merge(&self.preview_pane, &mut p.preview);
    }

    fn merge(ov: &PaneOverride, pc: &mut crate::config::PaneConfig) {
        if let Some(v) = ov.visible {
            pc.visible = v;
        }
        if ov.width.is_some() {
            pc.width = ov.width;
        }
        if let Some(w) = ov.weight {
            pc.weight = w;
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
        s.toggle_bookmark(p.clone());
        assert_eq!(s.bookmarks, vec![p.clone()]);
        s.toggle_bookmark(p.clone());
        assert!(s.bookmarks.is_empty());
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut s = State::default();
        s.bookmarks.push(PathBuf::from("/a"));
        s.last_cwd = Some(PathBuf::from("/b"));
        let back = State::from_str(&s.to_toml());
        assert_eq!(back, s);
    }

    #[test]
    fn bad_toml_yields_default() {
        let s = State::from_str("@@@");
        assert_eq!(s, State::default());
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
        st.apply_to(&mut fresh);
        assert!(fresh.bookmarks.visible);
        assert_eq!(fresh.bookmarks.width, Some(30));
        assert_eq!(fresh.tree.weight, 5);
        assert_eq!(fresh.order[0], "preview");
    }
}
