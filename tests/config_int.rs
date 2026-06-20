//! End-to-end verification of every config.toml option, driven through the
//! real public API the binary uses (Config::load_str, Keymap::build,
//! Theme::from_config, PreviewCache, settle_ms, App::new).

use explorenova::config::{Config, SAMPLE_CONFIG};
use explorenova::keymap::{Action, Keymap, Mode};
use explorenova::preview::{settle_ms, PreviewCache};
use explorenova::ui::theme::Theme;
use ratatui::style::Color;
use std::path::Path;

// ---- defaults ----------------------------------------------------------

#[test]
fn empty_config_yields_all_defaults() {
    let (c, err) = Config::load_str("");
    assert!(err.is_none());
    assert!(c.icons, "icons default true");
    assert!(!c.confirm_trash, "confirm_trash default false");
    assert_eq!(c.preview_max_bytes, 1_048_576);
    assert_eq!(c.preview.settle_cheap_ms, 150);
    assert_eq!(c.preview.settle_heavy_ms, 400);
    assert_eq!(c.preview.cache_cap, 128);
    assert_eq!(c.fs_refresh_debounce_ms, 150);
    assert_eq!(c.panes.order, vec!["bookmarks", "tree", "preview"]);
    assert!(!c.panes.bookmarks.visible);
    assert!(c.panes.tree.visible);
    assert!(c.theme.is_empty());
}

// ---- panes -------------------------------------------------------------

#[test]
fn panes_options_apply() {
    let src = r#"
        [panes]
        order = ["preview", "tree", "bookmarks"]
        [panes.bookmarks]
        visible = true
        width = 30
        weight = 5
        [panes.tree]
        visible = false
        weight = 9
    "#;
    let (c, err) = Config::load_str(src);
    assert!(err.is_none());
    assert_eq!(c.panes.order, vec!["preview", "tree", "bookmarks"]);
    assert!(c.panes.bookmarks.visible);
    assert_eq!(c.panes.bookmarks.width, Some(30));
    assert_eq!(c.panes.bookmarks.weight, 5);
    assert!(!c.panes.tree.visible);
    assert_eq!(c.panes.tree.weight, 9);
}

// ---- keys --------------------------------------------------------------

#[test]
fn keys_rebind_through_keymap() {
    // Rebind quit from "q" to "Q"; the old chord must be released.
    let src = r#"
        [keys]
        quit = ["Q"]
        down = ["j", "n"]
    "#;
    let (c, err) = Config::load_str(src);
    assert!(err.is_none());
    let km = Keymap::build(&c.keys);
    assert_eq!(km.resolve(Mode::Normal, "Q"), Some(Action::Quit));
    assert_eq!(km.resolve(Mode::Normal, "q"), None, "old quit chord released");
    // multi-chord rebind: both new chords resolve
    assert_eq!(km.resolve(Mode::Normal, "j"), Some(Action::Down));
    assert_eq!(km.resolve(Mode::Normal, "n"), Some(Action::Down));
}

// ---- scalar toggles ----------------------------------------------------

#[test]
fn scalar_options_apply() {
    let (c, _) = Config::load_str("icons = false\nconfirm_trash = true\npreview_max_bytes = 4096\n");
    assert!(!c.icons);
    assert!(c.confirm_trash);
    assert_eq!(c.preview_max_bytes, 4096);
}

// ---- preview tunables + clamps ----------------------------------------

#[test]
fn preview_tunables_apply_and_drive_settle() {
    let src = "[preview]\nsettle_cheap_ms = 80\nsettle_heavy_ms = 900\ncache_cap = 7\n";
    let (c, _) = Config::load_str(src);
    assert_eq!(c.preview.settle_cheap_ms, 80);
    assert_eq!(c.preview.settle_heavy_ms, 900);
    assert_eq!(c.preview.cache_cap, 7);
    // settle_ms uses the configured values
    assert_eq!(settle_ms(Path::new("a.txt"), c.preview.settle_cheap_ms, c.preview.settle_heavy_ms), 80);
    assert_eq!(settle_ms(Path::new("a.pdf"), c.preview.settle_cheap_ms, c.preview.settle_heavy_ms), 900);
    // cache honors the cap
    let cache = PreviewCache::with_cap(c.preview.cache_cap);
    let _ = cache; // construction with configured cap succeeds
}

#[test]
fn clamps_protect_against_bad_tunables() {
    // zero -> defaults
    let (c, _) = Config::load_str("[preview]\nsettle_cheap_ms=0\nsettle_heavy_ms=0\ncache_cap=0\n");
    assert_eq!(c.preview.settle_cheap_ms, 150);
    assert_eq!(c.preview.settle_heavy_ms, 400);
    assert_eq!(c.preview.cache_cap, 128);
    // heavy < cheap -> lifted
    let (c, _) = Config::load_str("[preview]\nsettle_cheap_ms=300\nsettle_heavy_ms=50\n");
    assert_eq!(c.preview.settle_heavy_ms, 300);
    // absurd debounce -> capped
    let (c, _) = Config::load_str("fs_refresh_debounce_ms = 999999\n");
    assert_eq!(c.fs_refresh_debounce_ms, 5000);
    // zero debounce -> default
    let (c, _) = Config::load_str("fs_refresh_debounce_ms = 0\n");
    assert_eq!(c.fs_refresh_debounce_ms, 150);
}

// ---- theme: preset, overrides, tolerance ------------------------------

#[test]
fn theme_default_is_rgb() {
    let (c, _) = Config::load_str("");
    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(t.dir, Theme::default().dir);
    assert!(matches!(t.dir, Color::Rgb(..)), "default palette is truecolor");
    assert!(w.is_empty());
}

#[test]
fn theme_terminal_preset_is_ansi() {
    let (c, _) = Config::load_str("[theme]\npreset = \"terminal\"\n");
    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(t.dir, Color::Cyan);
    assert!(w.is_empty());
}

#[test]
fn theme_field_overrides_hex_and_ansi() {
    let (c, _) = Config::load_str("[theme]\ndir = \"#102030\"\nfile = \"light_green\"\n");
    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(t.dir, Color::Rgb(16, 32, 48));
    assert_eq!(t.file, Color::LightGreen);
    // untouched field keeps default
    assert_eq!(t.git_added, Theme::default().git_added);
    assert!(w.is_empty());
}

#[test]
fn theme_override_layers_over_preset() {
    let (c, _) = Config::load_str("[theme]\npreset = \"terminal\"\ndir = \"#010203\"\n");
    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(t.dir, Color::Rgb(1, 2, 3), "override beats preset");
    assert_eq!(t.file, Color::White, "unspecified field keeps terminal preset");
    assert!(w.is_empty());
}

#[test]
fn theme_bad_values_are_tolerant_and_warn() {
    // bad color value, unknown key, rejected reset, unknown preset — all warn,
    // none fail, all leave a usable theme.
    let (c, _) = Config::load_str(
        "[theme]\npreset = \"dracula\"\ndir = \"notacolor\"\nbogus = \"blue\"\nselected_bg = \"reset\"\n",
    );
    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(w.len(), 4, "one warning per problem: {w:?}");
    // theme still usable: every field has a real default
    assert_eq!(t.dir, Theme::default().dir);
    assert_eq!(t.selected_bg, Theme::default().selected_bg);
}

#[test]
fn one_bad_color_never_discards_other_config() {
    // A theme typo must NOT reset keys/panes/scalars.
    let src = r#"
        icons = false
        confirm_trash = true
        [keys]
        quit = ["Q"]
        [theme]
        dir = "totally-bogus"
    "#;
    let (c, err) = Config::load_str(src);
    assert!(err.is_none(), "the file still parses");
    assert!(!c.icons, "scalar survives the bad color");
    assert!(c.confirm_trash);
    let km = Keymap::build(&c.keys);
    assert_eq!(km.resolve(Mode::Normal, "Q"), Some(Action::Quit), "keys survive");
    let (_t, w) = Theme::from_config(&c.theme);
    assert_eq!(w.len(), 1, "only the bad color warns");
}

// ---- App integration: theme built once, warnings surfaced -------------

#[test]
fn app_applies_theme_and_surfaces_warnings() {
    use explorenova::app::App;
    use explorenova::state::State;
    use explorenova::tree::Arena;

    let arena = Arena::new(Path::new(".")).unwrap_or_else(|_| {
        Arena::new(std::env::temp_dir().as_path()).unwrap()
    });
    let (mut config, _) = Config::load_str("[theme]\ndir = \"#0A0B0C\"\nbad = \"x\"\n");
    // normalize panes the way main does (no-op here) and build the app
    let _ = &mut config;
    let app = App::new(arena, config, State::default());
    assert_eq!(app.theme.dir, Color::Rgb(10, 11, 12), "theme applied to App");
    assert!(app.status.contains("bad"), "unknown-key warning surfaced: {:?}", app.status);
}

// ---- malformed config --------------------------------------------------

#[test]
fn malformed_config_falls_back_with_error() {
    let (c, err) = Config::load_str("this is = = not valid toml");
    assert!(err.is_some(), "error string produced for surfacing");
    // falls back to a fully usable default config
    assert_eq!(c.panes.order.len(), 3);
    assert_eq!(c.preview.cache_cap, 128);
}

// ---- sample config -----------------------------------------------------

#[test]
fn sample_config_is_valid_and_default_equivalent() {
    let (c, err) = Config::load_str(SAMPLE_CONFIG);
    assert!(err.is_none(), "shipped sample must parse: {err:?}");
    assert_eq!(c, Config::with_defaults(), "sample round-trips to defaults");
}

#[test]
fn sample_documents_every_option() {
    // The sample must mention every configurable surface so it is discoverable.
    for needle in [
        "icons", "confirm_trash", "preview_max_bytes", "fs_refresh_debounce_ms",
        "[panes", "[preview]", "settle_cheap_ms", "settle_heavy_ms", "cache_cap",
        "[keys]", "[theme]", "preset", "dir", "git_modified", "selected_bg",
        "header", "dim",
    ] {
        assert!(SAMPLE_CONFIG.contains(needle), "sample missing documentation for `{needle}`");
    }
}

// ---- kitchen sink: everything at once ---------------------------------

#[test]
fn every_option_set_together_no_interference() {
    let src = r##"
        icons = false
        confirm_trash = true
        preview_max_bytes = 2048
        fs_refresh_debounce_ms = 222

        [panes]
        order = ["tree", "preview", "bookmarks"]
        [panes.preview]
        visible = false
        weight = 4

        [preview]
        settle_cheap_ms = 60
        settle_heavy_ms = 700
        cache_cap = 16

        [keys]
        quit = ["Q"]
        refresh = ["F5"]

        [theme]
        preset = "terminal"
        dir = "#112233"
        git_modified = "yellow"
    "##;
    let (c, err) = Config::load_str(src);
    assert!(err.is_none());
    assert!(!c.icons);
    assert!(c.confirm_trash);
    assert_eq!(c.preview_max_bytes, 2048);
    assert_eq!(c.fs_refresh_debounce_ms, 222);
    assert_eq!(c.panes.order, vec!["tree", "preview", "bookmarks"]);
    assert!(!c.panes.preview.visible);
    assert_eq!(c.preview.settle_cheap_ms, 60);
    assert_eq!(c.preview.settle_heavy_ms, 700);
    assert_eq!(c.preview.cache_cap, 16);

    let km = Keymap::build(&c.keys);
    assert_eq!(km.resolve(Mode::Normal, "Q"), Some(Action::Quit));
    assert_eq!(km.resolve(Mode::Normal, "F5"), Some(Action::Refresh));

    let (t, w) = Theme::from_config(&c.theme);
    assert_eq!(t.dir, Color::Rgb(17, 34, 51), "field override beats preset");
    assert_eq!(t.git_modified, Color::Yellow, "ANSI override applies");
    assert_eq!(t.file, Color::White, "terminal preset shows through for unset field");
    assert!(w.is_empty());
}
