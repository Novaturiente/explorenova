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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PreviewConfig {
    /// Settle delay (ms) before rendering a cheap preview (text/dir/image).
    pub settle_cheap_ms: u64,
    /// Settle delay (ms) before rendering a heavy preview (pdf/video/xlsx/archive/avif).
    pub settle_heavy_ms: u64,
    /// Max number of cached previews before FIFO eviction.
    /// ponytail: bounds cached *entries*, not total preview memory —
    /// in-flight renders and worker backlog stay unbounded.
    pub cache_cap: usize,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self { settle_cheap_ms: 150, settle_heavy_ms: 400, cache_cap: 128 }
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
    /// Preview timing/cache tunables.
    pub preview: PreviewConfig,
    /// Debounce (ms) for coalescing filesystem-watch events before refresh.
    pub fs_refresh_debounce_ms: u64,
    /// Raw `[theme]` table: field-name -> color string (or `preset`).
    /// Parsed tolerantly into a `Theme` at startup; bad values warn, not fail.
    pub theme: BTreeMap<String, String>,
}

impl Config {
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c.icons = true;
        c.preview = PreviewConfig::default();
        c.fs_refresh_debounce_ms = 150;
        c
    }

    /// Parse user TOML over defaults. On parse error, return defaults + the error string.
    pub fn load_str(toml_src: &str) -> (Self, Option<String>) {
        match toml::from_str::<Config>(toml_src) {
            Ok(mut c) => {
                if c.preview_max_bytes == 0 {
                    c.preview_max_bytes = 1_048_576;
                }
                if c.preview.settle_cheap_ms == 0 {
                    c.preview.settle_cheap_ms = 150;
                }
                if c.preview.settle_heavy_ms == 0 {
                    c.preview.settle_heavy_ms = 400;
                }
                if c.preview.settle_heavy_ms < c.preview.settle_cheap_ms {
                    c.preview.settle_heavy_ms = c.preview.settle_cheap_ms;
                }
                if c.preview.cache_cap == 0 {
                    c.preview.cache_cap = 128;
                }
                if c.fs_refresh_debounce_ms == 0 {
                    c.fs_refresh_debounce_ms = 150;
                }
                c.fs_refresh_debounce_ms = c.fs_refresh_debounce_ms.min(5000);
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

    #[test]
    fn preview_tunables_default_and_clamp() {
        // missing [preview] -> defaults
        let (c, _) = Config::load_str("icons = true\n");
        assert_eq!(c.preview.settle_cheap_ms, 150);
        assert_eq!(c.preview.settle_heavy_ms, 400);
        assert_eq!(c.preview.cache_cap, 128);
        assert_eq!(c.fs_refresh_debounce_ms, 150);

        // zero values clamp to defaults
        let src = "[preview]\nsettle_cheap_ms = 0\nsettle_heavy_ms = 0\ncache_cap = 0\n";
        let (c, _) = Config::load_str(src);
        assert_eq!(c.preview.settle_cheap_ms, 150);
        assert_eq!(c.preview.settle_heavy_ms, 400);
        assert_eq!(c.preview.cache_cap, 128);

        // heavy < cheap is lifted to cheap
        let (c, _) = Config::load_str("[preview]\nsettle_cheap_ms = 300\nsettle_heavy_ms = 100\n");
        assert_eq!(c.preview.settle_cheap_ms, 300);
        assert_eq!(c.preview.settle_heavy_ms, 300);

        // absurd debounce is capped
        let (c, _) = Config::load_str("fs_refresh_debounce_ms = 999999\n");
        assert_eq!(c.fs_refresh_debounce_ms, 5000);
    }

    #[test]
    fn theme_table_parses_into_map() {
        let src = "[theme]\npreset = \"terminal\"\ndir = \"#102030\"\n";
        let (c, err) = Config::load_str(src);
        assert!(err.is_none());
        assert_eq!(c.theme.get("preset").map(String::as_str), Some("terminal"));
        assert_eq!(c.theme.get("dir").map(String::as_str), Some("#102030"));
        // missing [theme] -> empty map, no error
        let (c2, _) = Config::load_str("icons = true\n");
        assert!(c2.theme.is_empty());
    }
}
