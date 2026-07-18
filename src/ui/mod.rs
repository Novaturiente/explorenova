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

/// Build (pane_name, Constraint) pairs in display order from config,
/// skipping hidden panes. Fixed `width` -> Length; else Ratio via weight.
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
    let weight_sum: u32 = visible
        .iter()
        .filter(|(_, c)| c.width.is_none())
        .map(|(_, c)| c.weight.max(1) as u32)
        .sum::<u32>()
        .max(1);
    visible
        .iter()
        .map(|(name, c)| {
            let con = match c.width {
                Some(w) => Constraint::Length(w),
                None => Constraint::Ratio(c.weight.max(1) as u32, weight_sum),
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
        let mut p = PanesConfig::default(); // bookmarks hidden by default
        let cons = layout_constraints(&p);
        assert_eq!(cons.len(), 2);
        assert_eq!(cons[0].0, "tree");
        p.bookmarks.visible = true;
        assert_eq!(layout_constraints(&p).len(), 3);
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
        let mut p = PanesConfig::default();
        p.order = vec!["x".into(), "y".into(), "z".into()];
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
        let mut p = PanesConfig::default();
        p.bookmarks.visible = true;
        p.order = vec!["preview".into(), "tree".into(), "bookmarks".into()];
        let cons = layout_constraints(&p);
        assert_eq!(cons[0].0, "preview");
        assert_eq!(cons[2].0, "bookmarks");
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
/// previewed file (path or mtime) changes — not every frame.
#[derive(Default)]
pub struct ImageCache {
    pub key: Option<(std::path::PathBuf, std::time::SystemTime)>,
    proto: Option<StatefulProtocol>,
}

/// Render an already-decoded image into `area`, caching the protocol by
/// (path, mtime). Decoding happens on the preview worker thread — this
/// only rebuilds the terminal-graphics protocol when the file changes.
pub fn render_image(
    f: &mut Frame,
    area: Rect,
    picker: &mut Picker,
    cache: &mut ImageCache,
    path: &std::path::Path,
    img: &std::sync::Arc<image::DynamicImage>,
) {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let key = (path.to_path_buf(), mtime);
    if cache.key.as_ref() != Some(&key) {
        // One pixel-buffer copy per image change (protocol wants
        // ownership), never per frame.
        cache.proto = Some(picker.new_resize_protocol((**img).clone()));
        cache.key = Some(key);
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
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());
    let header_area = outer[0];
    let body = outer[1];
    let prompt_area = outer[2];

    panes_render::render_header(f, header_area, app, &theme);

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
        v.push(("tree".to_string(), Constraint::Ratio(1, 2)));
        v.push(("tree_other".to_string(), Constraint::Ratio(1, 2)));
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

    // In-flight long op: render a floating progress modal on top of
    // everything else and return early so other modal arms don't paint
    // over it. The op completion is detected and cleared in main.rs.
    if let Some(op) = &app.op {
        overlays::render_op_modal(f, body, op, &theme);
        return;
    }

    overlays::render_overlay(f, body, app, &theme, keymap);
}
