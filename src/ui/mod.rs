pub mod modal;
pub mod panes;
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
    pub key: Option<(std::path::PathBuf, std::time::SystemTime)>,
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
    // Split-pane mode: hide preview, render two equal tree panes
    // side by side (bookmarks pane still respects its visible flag).
    // Re-derives the constraint list from scratch so a user-toggled
    // preview pane doesn't leak in.
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
    {
        let root_path =
            app.arena.nodes[app.arena.root].path.display().to_string();
        let w = header_area.width.max(1) as usize;
        let chars: Vec<char> = root_path.chars().collect();
        let shown = if chars.len() > w {
            let start = chars.len() - (w - 1);
            let tail: String = chars[start..].iter().collect();
            format!("…{tail}")
        } else {
            root_path
        };
        f.render_widget(
            Paragraph::new(shown).style(
                ratatui::style::Style::default()
                    .fg(crate::ui::theme::Theme::default().header)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            header_area,
        );
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
        .split(body);
    let theme = crate::ui::theme::Theme::default();

    for (i, (name, _)) in cons.iter().enumerate() {
        let area: Rect = chunks[i];
        match name.as_str() {
            "tree" | "tree_other" => {
                // Slot index: "tree" = 0 (left), "tree_other" = 1 (right).
                // In single mode "tree" is the only slot and always
                // hosts App fields; "tree_other" is unused.
                // In split mode the App fields render in the slot named
                // by `active_slot`; the inactive `tabs[0]` data renders
                // in the other slot. This keeps screen positions fixed
                // across Tab swaps — only the highlight moves.
                let slot: u8 = if name == "tree" { 0 } else { 1 };
                let app_here = !split || app.active_slot == slot;
                let inner_w = area.width.saturating_sub(2);
                let (arena_ref, visible_ref, selected_ref, marks_ref):
                    (&crate::tree::Arena, &Vec<usize>, usize, &std::collections::BTreeSet<std::path::PathBuf>) =
                    if app_here {
                        (&app.arena, &app.visible, app.selected, &app.marks)
                    } else {
                        let snap = match app.tabs.first() {
                            Some(s) => s,
                            None => continue,
                        };
                        (&snap.arena, &snap.visible, snap.selected, &snap.marks)
                    };
                let items: Vec<ratatui::widgets::ListItem> = visible_ref
                    .iter()
                    .enumerate()
                    .map(|(idx, &id)| {
                        ratatui::widgets::ListItem::new(crate::ui::panes::tree_row(
                            arena_ref,
                            id,
                            idx == selected_ref,
                            &theme,
                            app.config.icons,
                            marks_ref.contains(&arena_ref.nodes[id].path),
                            inner_w,
                        ))
                    })
                    .collect();
                // Active pane (in split mode) gets the • marker and a
                // cyan border so the focused tree is visible at a glance.
                let is_active = split && app_here;
                let title = if is_active { "tree •".to_string() } else { "tree".to_string() };
                let border_style = if is_active {
                    ratatui::style::Style::default().fg(theme.mark)
                } else {
                    ratatui::style::Style::default()
                };
                let list = ratatui::widgets::List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(border_style)
                            .title(title),
                    )
                    .highlight_style(
                        ratatui::style::Style::default().bg(theme.selected_bg),
                    );
                let mut st = ratatui::widgets::ListState::default();
                st.select(Some(selected_ref));
                f.render_stateful_widget(list, area, &mut st);
            }
            "bookmarks" => {
                let focused = app.focus == crate::app::Pane::Bookmarks;
                let items: Vec<ratatui::widgets::ListItem> = app
                    .state
                    .bookmarks
                    .iter()
                    .map(|b| {
                        ratatui::widgets::ListItem::new(short_bookmark_label(b))
                    })
                    .collect();
                let title = if focused {
                    "bookmarks •"
                } else {
                    "bookmarks"
                };
                let list = ratatui::widgets::List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .title(title),
                    )
                    .highlight_style(
                        ratatui::style::Style::default().bg(theme.selected_bg),
                    );
                let mut st = ratatui::widgets::ListState::default();
                if focused && !app.state.bookmarks.is_empty() {
                    let i = app
                        .bookmark_sel
                        .min(app.state.bookmarks.len() - 1);
                    st.select(Some(i));
                }
                f.render_stateful_widget(list, area, &mut st);
            }
            "preview" => match app.visible.get(app.selected) {
                Some(&id) => {
                    const PREVIEW_DEBOUNCE_MS: u128 = 150;
                    let path = app.arena.nodes[id].path.clone();
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .title("preview");
                    let inner = block.inner(area);
                    f.render_widget(block, area);

                    // Settle gate: only do the (potentially expensive)
                    // get_or_render call once the cursor has been on
                    // this row for >= 150ms. Cached entries short-
                    // circuit so revisited paths are instant.
                    let elapsed = app.cursor_moved_at.elapsed().as_millis()
                        >= PREVIEW_DEBOUNCE_MS;
                    let cached = cache.contains(&path);
                    if !(elapsed || cached) {
                        f.render_widget(Paragraph::new("(loading…)"), inner);
                    } else {
                        let kind = cache
                            .get_or_render(&path, app.config.preview_max_bytes);
                        match kind {
                            PreviewKind::Image(p) => {
                                let already_decoded = img_cache
                                    .key
                                    .as_ref()
                                    .map(|(k, _)| k == &p)
                                    .unwrap_or(false);
                                if already_decoded || elapsed {
                                    render_image(f, inner, picker, img_cache, &p);
                                } else {
                                    f.render_widget(
                                        Paragraph::new("(loading…)"),
                                        inner,
                                    );
                                }
                            }
                            PreviewKind::Text(t) => {
                                f.render_widget(Paragraph::new(t), inner);
                            }
                            PreviewKind::Meta(t) => {
                                f.render_widget(Paragraph::new(t), inner);
                            }
                        }
                    }
                }
                None => {
                    f.render_widget(
                        Paragraph::new(String::new()).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(ratatui::widgets::BorderType::Rounded)
                                .title("preview"),
                        ),
                        area,
                    );
                }
            },
            _ => {}
        }
    }

    // Status row: transient messages only.
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let mut spans: Vec<Span> = Vec::new();
    if app.visual_mode {
        spans.push(Span::styled(
            "-- VISUAL --  ",
            Style::default()
                .fg(theme.mark)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(format!(
        "{}/{}  ",
        app.selected + 1,
        app.visible.len().max(1),
    )));
    if !app.marks.is_empty() {
        spans.push(Span::raw(format!("{} selected  ", app.marks.len())));
    }
    spans.push(Span::raw(app.status.clone()));
    f.render_widget(Paragraph::new(Line::from(spans)), prompt_area);

    // Modal overlay (input / confirm) centered over the body.
    match (&app.mode, &app.dialog) {
        (_, crate::app::Dialog::None) => {}
        (_, crate::app::Dialog::AppPick { items, sel }) => {
            let rows = items.len() + 1;
            let r = crate::ui::modal::centered_rect(
                50,
                (rows.min(14) as u16) + 1,
                body,
            );
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title("Open with");
            let inner = block.inner(r);
            f.render_widget(block, r);
            let mut lines: Vec<ratatui::text::Line> = Vec::new();
            for (i, a) in items.iter().enumerate() {
                let marker = if i == *sel { "> " } else { "  " };
                lines.push(ratatui::text::Line::from(format!(
                    "{marker}{}",
                    a.name
                )));
            }
            lines.push(ratatui::text::Line::from("  ──────────────"));
            let ci = items.len();
            let cmarker = if *sel == ci { "> " } else { "  " };
            lines.push(ratatui::text::Line::from(format!(
                "{cmarker}Custom command…"
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        (_, crate::app::Dialog::Devices { items, sel }) => {
            use ratatui::style::Style;
            use ratatui::text::{Line, Span};
            let rows = items.len().max(1);
            let r = crate::ui::modal::centered_rect(
                70,
                (rows.min(14) as u16) + 2,
                body,
            );
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title("Devices");
            let inner = block.inner(r);
            f.render_widget(block, r);
            let mut lines: Vec<Line> = Vec::new();
            if items.is_empty() {
                lines.push(Line::from("  no handleable devices"));
            } else {
                for (i, d) in items.iter().enumerate() {
                    let marker = if i == *sel { "> " } else { "  " };
                    let mounted = if d.is_mounted() { "●" } else { "○" };
                    let label = d.label.as_deref().unwrap_or("-");
                    let size = d.size.as_deref().unwrap_or("?");
                    let fs = d.fstype.as_deref().unwrap_or("");
                    let mp = d
                        .mountpoint
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(not mounted)".to_string());
                    lines.push(Line::from(format!(
                        "{marker}{mounted} {:<14} {:<8} {:<8} {:<14} {}",
                        d.device, size, fs, label, mp
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter cd/mount   u umount   r refresh   Esc close",
                Style::default().fg(theme.dim),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
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
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title(title);
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
                (s, (caret - start) as u16)
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
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title("Delete");
            let inner = block.inner(r);
            f.render_widget(block, r);
            f.render_widget(
                Paragraph::new(format!("{name}\n[y] yes   [n] no")),
                inner,
            );
        }
        (crate::keymap::Mode::Normal, _) => {}
        (crate::keymap::Mode::SortMenu, _) => {
            let r = crate::sort::centered_rect(40, 12, body);
            crate::sort::render_menu(r, f, &app.sort);
        }
        (crate::keymap::Mode::Help, _) => {
            // Wired in Task 4. Render is a no-op until help::render exists.
        }
    }
}
