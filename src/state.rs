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
}
