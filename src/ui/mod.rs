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

pub fn render_frame(f: &mut Frame, app: &mut App, cache: &mut PreviewCache) {
    let cons: Vec<_> = layout_constraints(&app.config.panes);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
        .split(f.area());
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
            "preview" => {
                let text = match app.visible.get(app.selected) {
                    Some(&id) => match cache.get_or_render(
                        &app.arena.nodes[id].path,
                        app.config.preview_max_bytes,
                    ) {
                        PreviewKind::Text(t) => t,
                        PreviewKind::Meta(m) => m,
                        PreviewKind::Image(p) => format!("[image] {}", p.display()),
                    },
                    None => String::new(),
                };
                f.render_widget(
                    Paragraph::new(text)
                        .block(Block::default().borders(Borders::ALL).title("preview")),
                    area,
                );
            }
            _ => {}
        }
    }
}
