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

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub panes: PanesConfig,
    /// action name -> list of key chords
    pub keys: BTreeMap<String, Vec<String>>,
    pub preview_max_bytes: u64,
    /// Show Nerd Font filetype icons in the tree.
    #[serde(default = "default_true")]
    pub icons: bool,
    /// Prompt y/N before moving to trash (`x`/Del). Permanent delete (`X`)
    /// always confirms regardless. Default off — trash is recoverable, so
    /// the fast path is the sensible default; set true for a safety gate.
    pub confirm_trash: bool,
}

impl Config {
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c.icons = true;
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

    #[test]
    fn icons_defaults_true_via_with_defaults() {
        assert!(Config::with_defaults().icons);
    }

    #[test]
    fn icons_missing_in_toml_defaults_true_else_parses() {
        let (c, e) = Config::load_str("preview_max_bytes = 99\n");
        assert!(e.is_none());
        assert!(c.icons, "missing icons key must default true");
        let (c2, _) = Config::load_str("icons = false\n");
        assert!(!c2.icons);
    }

    #[test]
    fn confirm_trash_defaults_false_and_parses() {
        assert!(!Config::with_defaults().confirm_trash, "fast trash by default");
        let (c, _) = Config::load_str("confirm_trash = true\n");
        assert!(c.confirm_trash, "opt-in safety gate parses");
    }
}
