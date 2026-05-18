use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PaneConfig {
    pub visible: bool,
    /// Fixed column width. None => proportional via `weight`.
    pub width: Option<u16>,
    pub weight: u16,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self { visible: true, width: None, weight: 1 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanesConfig {
    pub order: Vec<String>,
    pub bookmarks: PaneConfig,
    pub tree: PaneConfig,
    pub preview: PaneConfig,
}

impl Default for PanesConfig {
    fn default() -> Self {
        Self {
            order: vec!["bookmarks".into(), "tree".into(), "preview".into()],
            bookmarks: PaneConfig { visible: false, width: Some(24), weight: 1 },
            tree: PaneConfig { visible: true, width: None, weight: 2 },
            preview: PaneConfig { visible: true, width: None, weight: 3 },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub panes: PanesConfig,
    /// action name -> list of key chords
    pub keys: BTreeMap<String, Vec<String>>,
    pub preview_max_bytes: u64,
}

impl Config {
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c
    }

    /// Parse user TOML over defaults. On parse error, return defaults + the error string.
    pub fn load_str(toml_src: &str) -> (Self, Option<String>) {
        match toml::from_str::<Config>(toml_src) {
            Ok(mut c) => {
                if c.preview_max_bytes == 0 {
                    c.preview_max_bytes = 1_048_576;
                }
                (c, None)
            }
            Err(e) => (Config::with_defaults(), Some(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_three_panes_in_order() {
        let c = Config::with_defaults();
        assert_eq!(c.panes.order, vec!["bookmarks", "tree", "preview"]);
        assert!(!c.panes.bookmarks.visible);
        assert!(c.panes.preview.visible);
        assert_eq!(c.preview_max_bytes, 1_048_576);
    }

    #[test]
    fn user_toml_overrides_only_specified_fields() {
        let src = r#"
            [panes.bookmarks]
            visible = true
        "#;
        let (c, err) = Config::load_str(src);
        assert!(err.is_none());
        assert!(c.panes.bookmarks.visible);
        // unspecified field keeps default
        assert_eq!(c.panes.tree.weight, 2);
    }

    #[test]
    fn malformed_toml_returns_defaults_and_error() {
        let (c, err) = Config::load_str("this is = = not toml");
        assert!(err.is_some());
        assert_eq!(c.panes.order.len(), 3);
    }
}
