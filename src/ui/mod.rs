pub mod keyhints;
pub mod modal;
pub mod overlays;
pub mod panes;
pub mod panes_render;
pub mod theme;

use crate::config::PanesConfig;
use ratatui::layout::Constraint;

/// Short bookmark label: directory → folder name; file → `parent/name`.
/// Falls back to the absolute path for edge cases like `/` where neither
/// `file_name` nor `parent` are useful.
pub fn short_bookmark_label(p: &std::path::Path) -> String {
    let is_file = p.is_file();
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    let parent_name = p
        .parent()
        .and_then(|pp| pp.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    match (is_file, name, parent_name) {
        (true, Some(n), Some(parent)) => format!("{parent}/{n}"),
        (_, Some(n), _) => n,
        _ => p.display().to_string(),
    }
}

/// Filenames can contain \e, \r, etc — rendered raw they corrupt the frame
/// (a \r name overwrites the row from col 0; \e[31m injects live SGR).
/// ratatui does not sanitize span content; do it at the boundary.
pub fn display_name(s: &str) -> String {
    crate::preview::sanitize_plain(s)
}

/// Build (pane_name, Constraint) pairs in display order from config,
/// skipping hidden panes. Fixed `width` -> Length; else `Fill(weight)`.
pub fn layout_constraints(p: &PanesConfig) -> Vec<(String, Constraint)> {
    let mut visible: Vec<(&str, &crate::config::PaneConfig)> = Vec::new();
    for name in &p.order {
        let pc = match name.as_str() {
            "bookmarks" => &p.bookmarks,
            "tree" => &p.tree,
            "preview" => &p.preview,
            _ => continue,
        };
        if pc.visible {
            visible.push((name.as_str(), pc));
        }
    }
    // Never hand ratatui an empty constraint set (blank screen / panic).
    // A hand-edited state.toml hiding every pane, or bad pane names, falls
    // back to a full-width tree so the app is always usable.
    if visible.is_empty() {
        return vec![("tree".to_string(), Constraint::Ratio(1, 1))];
    }
    // Fill, not Ratio: a Ratio is a fraction of the WHOLE body, so with a
    // fixed-width bookmarks pane also claiming columns the proportional
    // panes over-subscribe and ratatui flattens them to equal widths (a
    // configured 2:3 rendered 1:1, making `<`/`>` look inert). Fill hands
    // out what is LEFT after the Length panes, weighted.
    visible
        .iter()
        .map(|(name, c)| {
            let con = match c.width {
                Some(w) => Constraint::Length(w),
                None => Constraint::Fill(c.weight.max(1)),
            };
            (name.to_string(), con)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanesConfig;

    #[test]
    fn hidden_pane_excluded() {
        let mut p = PanesConfig::default();
        assert_eq!(layout_constraints(&p).len(), 3); // bookmarks visible by default
        p.bookmarks.visible = false;
        let cons = layout_constraints(&p);
        assert_eq!(cons.len(), 2);
        assert_eq!(cons[0].0, "tree");
    }

    #[test]
    fn all_panes_hidden_falls_back_to_tree() {
        let mut p = PanesConfig::default();
        p.bookmarks.visible = false;
        p.tree.visible = false;
        p.preview.visible = false;
        let cons = layout_constraints(&p);
        assert_eq!(cons.len(), 1, "must never be empty");
        assert_eq!(cons[0].0, "tree");
    }

    #[test]
    fn bad_pane_names_fall_back_to_tree() {
        let p = PanesConfig {
            order: vec!["x".into(), "y".into(), "z".into()],
            ..PanesConfig::default()
        };
        let cons = layout_constraints(&p);
        assert_eq!(cons.len(), 1);
        assert_eq!(cons[0].0, "tree");
    }

    #[test]
    fn fixed_width_uses_length() {
        let mut p = PanesConfig::default();
        p.bookmarks.visible = true;
        let cons = layout_constraints(&p);
        assert!(matches!(cons[0].1, Constraint::Length(24)));
    }

    #[test]
    fn order_is_respected() {
        let mut p = PanesConfig {
            order: vec!["preview".into(), "tree".into(), "bookmarks".into()],
            ..PanesConfig::default()
        };
        p.bookmarks.visible = true;
        let cons = layout_constraints(&p);
        assert_eq!(cons[0].0, "preview");
        assert_eq!(cons[2].0, "bookmarks");
    }

    #[test]
    fn weights_split_the_space_left_by_fixed_panes() {
        // 120 cols, bookmarks fixed at 24 -> 96 remain; a 2:3 tree/preview
        // weight must land ~38/58, not the 48/48 that Ratio produced.
        let mut p = PanesConfig::default();
        p.bookmarks.visible = true;
        p.bookmarks.width = Some(24);
        p.tree.width = None;
        p.tree.weight = 2;
        p.preview.width = None;
        p.preview.weight = 3;
        let cons = layout_constraints(&p);
        let areas = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
            .split(ratatui::layout::Rect { x: 0, y: 0, width: 120, height: 10 });
        let by = |n: &str| areas[cons.iter().position(|(k, _)| k == n).unwrap()].width;
        assert_eq!(by("bookmarks"), 24);
        assert_eq!(by("tree") + by("preview"), 96);
        // 96 * 2/5 = 38.4, 96 * 3/5 = 57.6 — allow the rounding slop.
        assert!((by("tree") as i32 - 38).abs() <= 1, "tree = {}", by("tree"));
        assert!((by("preview") as i32 - 58).abs() <= 1, "preview = {}", by("preview"));
    }

    #[test]
    fn short_label_uses_folder_name_for_dirs() {
        // Existing folder — should show only the folder name.
        let d = tempfile::tempdir().unwrap();
        let label = short_bookmark_label(d.path());
        // tempdir creates a uniquely-named subdir; just assert no slash
        // and that it matches the path's file_name.
        let expected = d
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(label, expected);
    }

    #[test]
    fn short_label_uses_parent_slash_name_for_files() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("notes.txt");
        std::fs::write(&f, "x").unwrap();
        let parent_name = d.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(
            short_bookmark_label(&f),
            format!("{parent_name}/notes.txt")
        );
    }

    #[test]
    fn short_label_falls_back_for_root() {
        // `/` has no file_name and no parent file_name; fallback is the
        // raw display string.
        let label = short_bookmark_label(std::path::Path::new("/"));
        assert_eq!(label, "/");
    }

    #[test]
    fn row_name_strips_control_chars() {
        assert_eq!(display_name("bad\x1b[31mred\rname"), "bad?[31mred?name");
    }

    #[test]
    fn bookmark_label_sanitizes_control_chars() {
        let path = std::path::Path::new("/tmp/bad\x1b[31mdir\rname");
        let label = short_bookmark_label(path);
        let sanitized = display_name(&label);
        assert_eq!(sanitized, "bad?[31mdir?name");
    }

    #[test]
    fn image_cache_clear_releases_the_decoded_buffer() {
        let picker = Picker::from_fontsize((8, 16));
        let img = std::sync::Arc::new(image::DynamicImage::new_rgb8(4, 4));
        let mut c = ImageCache {
            key: Some((std::path::PathBuf::from("/x.png"), img.clone())),
            proto: Some(picker.new_resize_protocol((*img).clone())),
        };
        c.clear();
        assert!(c.key.is_none());
        assert!(c.proto.is_none(), "the pixel buffer must go too, not just the key");
    }

    #[test]
    fn viewport_start_centers_and_clamps() {
        // fits on screen → start 0
        assert_eq!(viewport_start(5, 3, 10), 0);
        // sel near top → start 0
        assert_eq!(viewport_start(100, 1, 10), 0);
        // sel in middle → centered (sel - visible/2)
        assert_eq!(viewport_start(100, 50, 10), 45);
        // sel near bottom → clamped so window ends at len
        assert_eq!(viewport_start(100, 99, 10), 90);
    }
}

use crate::app::App;
use crate::preview::PreviewCache;
use ratatui::layout::{Direction, Layout, Rect};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

/// Lazily-built terminal graphics picker (kitty/sixel/iTerm2/halfblocks).
pub fn make_picker() -> Picker {
    Picker::from_query_stdio().unwrap_or_else(|e| {
        if matches!(e, ratatui_image::errors::Errors::Timeout(_)) {
            // The capability query got no reply (e.g. tmux with
            // allow-passthrough off swallows the wrapped queries).
            // ratatui-image then LEAKS its stdin-reader thread, which sits
            // in a blocking read racing crossterm for every keystroke —
            // roughly every other keypress silently vanishes. Send a bare,
            // un-wrapped DSR: any terminal (tmux included) answers ESC[0n,
            // the leaked thread parses it as its exit signal and dies.
            // It queued on the tty before crossterm's reader exists, so
            // the reply reaches it, not the event loop.
            use std::io::Write;
            let _ = std::io::stdout().write_all(b"\x1b[5n");
            let _ = std::io::stdout().flush();
        }
        Picker::from_fontsize((8, 16))
    })
}

/// Caches the decoded image protocol so it is rebuilt only when the
/// previewed file changes — not every frame. No filesystem access: keyed
/// by path plus `Arc::ptr_eq` on the decoded image, so freshness comes
/// from the preview cache's invalidation (a new render produces a new
/// `Arc`) rather than a per-frame mtime stat.
#[derive(Default)]
pub struct ImageCache {
    pub key: Option<(std::path::PathBuf, std::sync::Arc<image::DynamicImage>)>,
    proto: Option<StatefulProtocol>,
}

impl ImageCache {
    /// Drop the decoded buffer. The protocol owns a full-resolution copy
    /// of the image — tens of MB for a 4K photo — and it was only ever
    /// replaced by the *next* image, so moving the cursor onto a text file
    /// or a directory left it resident for the rest of the session.
    pub fn clear(&mut self) {
        self.key = None;
        self.proto = None;
    }
}

/// Render an already-decoded image into `area`, caching the protocol by
/// (path, image identity). Decoding happens on the preview worker thread —
/// this only rebuilds the terminal-graphics protocol when the file changes.
pub fn render_image(
    f: &mut Frame,
    area: Rect,
    picker: &mut Picker,
    cache: &mut ImageCache,
    path: &std::path::Path,
    img: &std::sync::Arc<image::DynamicImage>,
) {
    let same = cache
        .key
        .as_ref()
        .map(|(p, i)| p == path && std::sync::Arc::ptr_eq(i, img))
        .unwrap_or(false);
    if !same {
        // One pixel-buffer copy per image change (protocol wants
        // ownership), never per frame.
        cache.proto = Some(picker.new_resize_protocol((**img).clone()));
        cache.key = Some((path.to_path_buf(), img.clone()));
    }
    match &mut cache.proto {
        Some(proto) => {
            let widget = StatefulImage::default().resize(Resize::Fit(None));
            f.render_stateful_widget(widget, area, proto);
        }
        None => {
            f.render_widget(
                Paragraph::new(format!("[unreadable image] {}", path.display())),
                area,
            );
        }
    }
}


/// First visible row index for a window of `visible` rows centered on
/// `sel` within `len` items. Matches `viewport_around`'s slice start.
pub(crate) fn viewport_start(len: usize, sel: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        return 0;
    }
    let half = visible / 2;
    let max_start = len - visible;
    sel.saturating_sub(half).min(max_start)
}

/// Slice `lines` to a viewport of `visible` rows that keeps `sel` in
/// view. Anchors selection roughly in the middle when scrolling.
pub(crate) fn viewport_around<T: Clone>(lines: &[T], sel: usize, visible: usize) -> Vec<T> {
    if visible == 0 || lines.is_empty() {
        return Vec::new();
    }
    if lines.len() <= visible {
        return lines.to_vec();
    }
    let start = viewport_start(lines.len(), sel, visible);
    lines[start..start + visible].to_vec()
}

/// Compose the full frame: top header, body (panes laid out per config,
/// with split-mode tree-tree-bookmarks overrides), bottom status bar,
/// and any active overlay (op progress / modal).
pub fn render_frame(
    f: &mut Frame,
    app: &mut App,
    cache: &mut PreviewCache,
    picker: &mut Picker,
    img_cache: &mut ImageCache,
    keymap: &crate::keymap::Keymap,
) {
    use crate::ui::overlays;
    use crate::ui::panes_render;
    let theme = app.theme.clone();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(f.area());
    let body = outer[0];
    let prompt_area = outer[1];
    let keyhints_area = outer[2];

    // Split-pane mode: hide preview, render two equal tree panes side by
    // side (bookmarks pane still respects its visible flag). Re-derives
    // constraints from scratch so a user-toggled preview pane doesn't leak in.
    let split = app.is_split();
    let cons: Vec<(String, Constraint)> = if split {
        let mut v: Vec<(String, Constraint)> = Vec::with_capacity(3);
        if app.config.panes.bookmarks.visible {
            let bk = &app.config.panes.bookmarks;
            v.push((
                "bookmarks".to_string(),
                match bk.width {
                    Some(w) => Constraint::Length(w),
                    None => Constraint::Ratio(1, 4),
                },
            ));
        }
        // Fill so the two trees split whatever the bookmarks pane leaves,
        // rather than each claiming half of the full body width.
        v.push(("tree".to_string(), Constraint::Fill(1)));
        v.push(("tree_other".to_string(), Constraint::Fill(1)));
        v
    } else {
        layout_constraints(&app.config.panes)
    };
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
        .split(body);

    for (i, (name, _)) in cons.iter().enumerate() {
        let area = chunks[i];
        match name.as_str() {
            "tree" => panes_render::render_pane_tree(f, area, app, &theme, 0, split),
            "tree_other" => panes_render::render_pane_tree(f, area, app, &theme, 1, split),
            "bookmarks" => panes_render::render_pane_bookmarks(f, area, app, &theme),
            "preview" => panes_render::render_pane_preview(
                f, area, app, &theme, cache, picker, img_cache,
            ),
            _ => {}
        }
    }

    // 3-segment status bar (Left: cursor meta/status, Center: selection,
    // Right: items + free disk). VISUAL flag is folded into Left segment.
    crate::status::render(prompt_area, f, app);
    crate::ui::keyhints::render(keyhints_area, f, app, keymap, &theme);

    // In-flight long op: render a floating progress modal on top of
    // everything else and return early so other modal arms don't paint
    // over it. The op completion is detected and cleared in main.rs.
    if let Some(op) = &app.op {
        overlays::render_op_modal(f, body, op, &theme);
        return;
    }

    overlays::render_overlay(f, body, app, &theme, keymap);
}
