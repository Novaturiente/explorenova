pub mod modal;
pub mod panes;
pub mod theme;

use crate::config::PanesConfig;
use ratatui::layout::Constraint;

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
    use crate::config::{PaneConfig, PanesConfig};

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
}

use crate::app::App;
use crate::preview::{PreviewCache, PreviewKind};
use ratatui::layout::{Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

/// Lazily-built terminal graphics picker (kitty/sixel/iTerm2/halfblocks).
pub fn make_picker() -> Picker {
    Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)))
}

/// Caches the decoded image protocol so it is rebuilt only when the
/// previewed file (path or mtime) changes — not every frame.
#[derive(Default)]
pub struct ImageCache {
    key: Option<(std::path::PathBuf, std::time::SystemTime)>,
    proto: Option<StatefulProtocol>,
}

/// Render an image file into `area`, caching the protocol by (path, mtime).
pub fn render_image(
    f: &mut Frame,
    area: Rect,
    picker: &mut Picker,
    cache: &mut ImageCache,
    path: &std::path::Path,
) {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let key = (path.to_path_buf(), mtime);
    if cache.key.as_ref() != Some(&key) {
        match image::ImageReader::open(path).map(|r| r.decode()) {
            Ok(Ok(img)) => cache.proto = Some(picker.new_resize_protocol(img)),
            _ => cache.proto = None,
        }
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

pub fn render_frame(
    f: &mut Frame,
    app: &mut App,
    cache: &mut PreviewCache,
    picker: &mut Picker,
    img_cache: &mut ImageCache,
) {
    let cons: Vec<_> = layout_constraints(&app.config.panes);
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());
    let body = outer[0];
    let prompt_area = outer[1];
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
        .split(body);
    let theme = crate::ui::theme::Theme::default();

    for (i, (name, _)) in cons.iter().enumerate() {
        let area: Rect = chunks[i];
        match name.as_str() {
            "tree" => {
                let rows: Vec<_> = app
                    .visible
                    .iter()
                    .enumerate()
                    .map(|(idx, &id)| {
                        crate::ui::panes::tree_row(&app.arena, id, idx == app.selected, &theme)
                    })
                    .collect();
                let p = Paragraph::new(rows)
                    .block(Block::default().borders(Borders::ALL).title("tree"));
                f.render_widget(p, area);
            }
            "bookmarks" => {
                let items: Vec<_> = app
                    .state
                    .bookmarks
                    .iter()
                    .map(|b| ratatui::text::Line::from(b.display().to_string()))
                    .collect();
                f.render_widget(
                    Paragraph::new(items)
                        .block(Block::default().borders(Borders::ALL).title("bookmarks")),
                    area,
                );
            }
            "preview" => match app.visible.get(app.selected) {
                Some(&id) => {
                    let kind = cache
                        .get_or_render(&app.arena.nodes[id].path, app.config.preview_max_bytes);
                    match kind {
                        PreviewKind::Image(p) => {
                            let block =
                                Block::default().borders(Borders::ALL).title("preview");
                            let inner = block.inner(area);
                            f.render_widget(block, area);
                            render_image(f, inner, picker, img_cache, &p);
                        }
                        PreviewKind::Text(t) | PreviewKind::Meta(t) => {
                            f.render_widget(
                                Paragraph::new(t).block(
                                    Block::default().borders(Borders::ALL).title("preview"),
                                ),
                                area,
                            );
                        }
                    }
                }
                None => {
                    f.render_widget(
                        Paragraph::new(String::new()).block(
                            Block::default().borders(Borders::ALL).title("preview"),
                        ),
                        area,
                    );
                }
            },
            _ => {}
        }
    }

    // Status row: transient messages only.
    f.render_widget(Paragraph::new(app.status.clone()), prompt_area);

    // Modal overlay (input / confirm) centered over the body.
    match (&app.mode, &app.dialog) {
        (_, crate::app::Dialog::None) => {}
        (crate::keymap::Mode::Search | crate::keymap::Mode::Input, _) => {
            let title = match app.mode {
                crate::keymap::Mode::Search => "Search".to_string(),
                _ => match app.pending {
                    Some(crate::keymap::Action::PathInput) => "Go to path".to_string(),
                    Some(crate::keymap::Action::Create) => "Create".to_string(),
                    Some(crate::keymap::Action::Rename) => "Rename".to_string(),
                    Some(crate::keymap::Action::OpenWith) => {
                        "Open with (command)".to_string()
                    }
                    Some(crate::keymap::Action::ShellOnce) => "Shell".to_string(),
                    _ => "Input".to_string(),
                },
            };
            let r = crate::ui::modal::centered_rect(55, 1, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default().borders(Borders::ALL).title(title);
            let inner = block.inner(r);
            f.render_widget(block, r);
            let val = app.input.value();
            let w = inner.width.max(1) as usize;
            let nchars = val.chars().count();
            let (shown, cursor_col): (String, u16) = if nchars < w {
                (val.clone(), app.input.caret() as u16)
            } else {
                let caret = app.input.caret();
                let start = caret.saturating_sub(w - 1);
                let s: String = val.chars().skip(start).take(w - 1).collect();
                (s, (w - 1) as u16)
            };
            f.render_widget(Paragraph::new(shown), inner);
            f.set_cursor_position((inner.x + cursor_col, inner.y));
        }
        (crate::keymap::Mode::Confirm, _) => {
            let name = app
                .visible
                .get(app.selected)
                .map(|&id| {
                    app.arena.nodes[id]
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let r = crate::ui::modal::centered_rect(50, 2, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default().borders(Borders::ALL).title("Delete");
            let inner = block.inner(r);
            f.render_widget(block, r);
            f.render_widget(
                Paragraph::new(format!("{name}\n[y] yes   [n] no")),
                inner,
            );
        }
        (crate::keymap::Mode::Normal, _) => {}
    }
}
