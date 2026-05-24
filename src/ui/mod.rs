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

/// Render a floating progress modal for an in-flight long op
/// (copy/move/extract). Bytes-based ops show a Gauge; extract
/// (indeterminate) shows a spinner + elapsed time.
fn render_op_modal(
    f: &mut Frame,
    body: ratatui::layout::Rect,
    op: &crate::ops::OpState,
    theme: &crate::ui::theme::Theme,
) {
    use crate::ui::modal;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Clear, Gauge, Paragraph};
    let r = modal::adaptive_rect(64, 7, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block(op.kind.verb(), theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Row 0: label + elapsed.
    let elapsed = op.started_at.elapsed().as_secs();
    let label_rect = ratatui::layout::Rect {
        height: 1,
        ..inner
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                op.label.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {elapsed}s elapsed"),
                modal::dim_style(theme),
            ),
        ])),
        label_rect,
    );

    // Row 1: blank.
    // Row 2: gauge or spinner.
    let bar_rect = ratatui::layout::Rect {
        y: inner.y + 2,
        height: 1,
        ..inner
    };
    if op.indeterminate || op.total_bytes == 0 {
        let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let i = (op.started_at.elapsed().as_millis() / 100) as usize;
        let frame = frames[i % frames.len()];
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{frame} "),
                    Style::default()
                        .fg(theme.symlink)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("working…", modal::dim_style(theme)),
            ])),
            bar_rect,
        );
    } else {
        let pct = (op.done_bytes as f64 / op.total_bytes as f64 * 100.0)
            .clamp(0.0, 100.0) as u16;
        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(theme.symlink)
                    .bg(theme.selected_bg),
            )
            .percent(pct)
            .label(format!(
                "{} / {}  ·  {}%",
                crate::status::human_bytes(op.done_bytes),
                crate::status::human_bytes(op.total_bytes),
                pct
            ));
        f.render_widget(gauge, bar_rect);
    }

    // Footer: dim hint.
    let footer_rect = ratatui::layout::Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            " op runs in background · tree refreshes when done",
            modal::dim_style(theme),
        )),
        footer_rect,
    );
}

/// Slice `lines` to a viewport of `visible` rows that keeps `sel` in
/// view. Anchors selection roughly in the middle when scrolling.
fn viewport_around<T: Clone>(lines: &[T], sel: usize, visible: usize) -> Vec<T> {
    if visible == 0 || lines.is_empty() {
        return Vec::new();
    }
    if lines.len() <= visible {
        return lines.to_vec();
    }
    let half = visible / 2;
    let max_start = lines.len() - visible;
    let start = sel.saturating_sub(half).min(max_start);
    lines[start..start + visible].to_vec()
}

pub fn render_frame(
    f: &mut Frame,
    app: &mut App,
    cache: &mut PreviewCache,
    picker: &mut Picker,
    img_cache: &mut ImageCache,
    keymap: &crate::keymap::Keymap,
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
                let title_text = if is_active { " tree ● " } else { " tree " };
                let title_style = if is_active {
                    ratatui::style::Style::default()
                        .fg(theme.mark)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    ratatui::style::Style::default()
                        .fg(theme.header)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                };
                let border_style = if is_active {
                    ratatui::style::Style::default().fg(theme.mark)
                } else {
                    ratatui::style::Style::default().fg(theme.dim)
                };
                let list = ratatui::widgets::List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(border_style)
                            .title(ratatui::text::Span::styled(title_text, title_style)),
                    )
                    .highlight_style(
                        ratatui::style::Style::default()
                            .bg(theme.selected_bg)
                            .add_modifier(ratatui::style::Modifier::BOLD),
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
                let title_text = if focused { " bookmarks ● " } else { " bookmarks " };
                let title_style = if focused {
                    ratatui::style::Style::default()
                        .fg(theme.mark)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    ratatui::style::Style::default()
                        .fg(theme.header)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                };
                let border_style = if focused {
                    ratatui::style::Style::default().fg(theme.mark)
                } else {
                    ratatui::style::Style::default().fg(theme.dim)
                };
                let list = ratatui::widgets::List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(border_style)
                            .title(ratatui::text::Span::styled(title_text, title_style)),
                    )
                    .highlight_style(
                        ratatui::style::Style::default()
                            .bg(theme.selected_bg)
                            .add_modifier(ratatui::style::Modifier::BOLD),
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
                        .border_style(ratatui::style::Style::default().fg(theme.dim))
                        .title(ratatui::text::Span::styled(
                            " preview ",
                            ratatui::style::Style::default()
                                .fg(theme.header)
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ));
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
                                .border_style(
                                    ratatui::style::Style::default().fg(theme.dim),
                                )
                                .title(ratatui::text::Span::styled(
                                    " preview ",
                                    ratatui::style::Style::default()
                                        .fg(theme.header)
                                        .add_modifier(ratatui::style::Modifier::BOLD),
                                )),
                        ),
                        area,
                    );
                }
            },
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
        render_op_modal(f, body, op, &theme);
        return;
    }

    // Modal overlay (input / confirm) centered over the body.
    // SortMenu and Help live on Mode (not Dialog) so they must be matched
    // BEFORE the generic Dialog::None catch-all swallows them.
    match (&app.mode, &app.dialog) {
        (crate::keymap::Mode::SortMenu, _) => {
            let r = crate::ui::modal::adaptive_rect(44, 13, body);
            crate::sort::render_menu(r, f, &app.sort, &theme);
            return;
        }
        (crate::keymap::Mode::Help, _) => {
            let entries = crate::help::build_entries(keymap);
            // Always request a large rect; adaptive_rect clamps it to
            // 80% of the terminal. Scrolling handles tall content.
            let r = crate::ui::modal::adaptive_rect(78, 40, body);
            crate::help::render(
                r,
                f,
                &entries,
                &theme,
                app.help_scroll,
                &app.help_filter,
            );
            return;
        }
        (_, crate::app::Dialog::None) => {}
        (_, crate::app::Dialog::AppPick { items, sel, filter }) => {
            use crate::ui::modal;
            use ratatui::style::{Modifier, Style};
            use ratatui::text::{Line, Span};
            // Filtered indices into `items`. Custom-command row is always
            // the last picker slot regardless of filter.
            let needle = filter.to_lowercase();
            let toks: Vec<&str> = needle.split_whitespace().collect();
            let filtered: Vec<usize> = if toks.is_empty() {
                (0..items.len()).collect()
            } else {
                items
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| {
                        let h = a.name.to_lowercase();
                        toks.iter().all(|t| h.contains(t))
                    })
                    .map(|(i, _)| i)
                    .collect()
            };
            let list_len = filtered.len() + 1;
            // Adaptive sizing: width tracks longest app name (capped),
            // height fits filter + sep + list (capped) + footer + borders.
            let max_name = items.iter().map(|a| a.name.len()).max().unwrap_or(20);
            let desired_w = (max_name as u16 + 10).clamp(40, 64);
            let visible_list = list_len.min(12) as u16;
            let desired_h = 1 /* filter */ + 1 /* sep */ + visible_list + 2 /* footer */ + 2 /* border */;
            let r = modal::adaptive_rect(desired_w, desired_h, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = modal::modal_block("Open with", &theme);
            let inner = modal::pad_inner(block.inner(r));
            f.render_widget(block, r);

            // Header: filter line + separator row.
            let header_rect = ratatui::layout::Rect {
                height: 2,
                ..inner
            };
            let filter_line = if filter.is_empty() {
                Line::from(vec![
                    Span::styled("/ ", Style::default().fg(theme.match_hl)),
                    Span::styled("type to filter…", modal::dim_style(&theme)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        "/ ",
                        Style::default()
                            .fg(theme.match_hl)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(filter.clone()),
                    Span::styled("▏", Style::default().fg(theme.match_hl)),
                ])
            };
            let sep_line = Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                modal::dim_style(&theme),
            ));
            f.render_widget(Paragraph::new(vec![filter_line, sep_line]), header_rect);

            // List body: scrollable, selection-highlighted rows.
            let footer_rows: u16 = 2;
            let list_h = inner
                .height
                .saturating_sub(2 + footer_rows)
                .max(1);
            let list_rect = ratatui::layout::Rect {
                y: inner.y + 2,
                height: list_h,
                ..inner
            };
            let mut lines: Vec<Line> = Vec::with_capacity(list_len);
            for (i, &orig) in filtered.iter().enumerate() {
                let style = if i == *sel {
                    modal::selection_style(&theme)
                } else {
                    Style::default()
                };
                let marker = if i == *sel { "▸ " } else { "  " };
                lines.push(Line::from(Span::styled(
                    format!("{marker}{}", items[orig].name),
                    style,
                )));
            }
            // Separator before Custom command...
            lines.push(Line::from(Span::styled(
                "  · · ·",
                modal::dim_style(&theme),
            )));
            let ci = filtered.len();
            let style = if *sel == ci {
                modal::selection_style(&theme)
            } else {
                Style::default().fg(theme.dim)
            };
            let cmarker = if *sel == ci { "▸ " } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("{cmarker}Custom command…"),
                style,
            )));
            let view = viewport_around(&lines, *sel, list_rect.height as usize);
            f.render_widget(Paragraph::new(view), list_rect);

            // Footer hints.
            let footer_rect = ratatui::layout::Rect {
                y: inner.y + inner.height.saturating_sub(footer_rows),
                height: footer_rows,
                ..inner
            };
            let hints = vec![
                Line::from(""),
                Line::from(Span::styled(
                    " enter open · ↑↓ pick · ⌫ del · esc close",
                    modal::dim_style(&theme),
                )),
            ];
            f.render_widget(Paragraph::new(hints), footer_rect);
        }
        (_, crate::app::Dialog::Devices { items, sel }) => {
            use crate::ui::modal;
            use ratatui::style::{Modifier, Style};
            use ratatui::text::{Line, Span};
            let body_rows = items.len().max(1) as u16;
            let visible_rows = body_rows.min(14);
            let desired_h = 1 /* header */ + 1 /* sep */ + visible_rows + 2 /* footer */ + 2 /* border */;
            let r = modal::adaptive_rect(78, desired_h, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = modal::modal_block("Devices", &theme);
            let inner = modal::pad_inner(block.inner(r));
            f.render_widget(block, r);

            // Header row: column titles.
            let header_rect = ratatui::layout::Rect {
                height: 2,
                ..inner
            };
            let cols = format!(
                "  {:<14} {:<8} {:<8} {:<14} {}",
                "DEVICE", "SIZE", "FS", "LABEL", "MOUNTPOINT"
            );
            let header_lines = vec![
                Line::from(Span::styled(
                    cols,
                    Style::default()
                        .fg(theme.header)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "─".repeat(inner.width as usize),
                    modal::dim_style(&theme),
                )),
            ];
            f.render_widget(Paragraph::new(header_lines), header_rect);

            // Body
            let footer_rows: u16 = 2;
            let body_h = inner.height.saturating_sub(2 + footer_rows).max(1);
            let body_rect = ratatui::layout::Rect {
                y: inner.y + 2,
                height: body_h,
                ..inner
            };
            let mut body_lines: Vec<Line> = Vec::new();
            if items.is_empty() {
                body_lines.push(Line::from(Span::styled(
                    "  no handleable devices",
                    modal::dim_style(&theme),
                )));
            } else {
                for (i, d) in items.iter().enumerate() {
                    let style = if i == *sel {
                        modal::selection_style(&theme)
                    } else {
                        Style::default()
                    };
                    let marker = if i == *sel { "▸ " } else { "  " };
                    let mounted_span = if d.is_mounted() {
                        Span::styled(
                            "● ",
                            Style::default()
                                .fg(theme.git_added)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled("○ ", modal::dim_style(&theme))
                    };
                    let label = d.label.as_deref().unwrap_or("-");
                    let size = d.size.as_deref().unwrap_or("?");
                    let fs = d.fstype.as_deref().unwrap_or("");
                    let mp = d
                        .mountpoint
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(not mounted)".to_string());
                    body_lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        mounted_span,
                        Span::styled(
                            format!(
                                "{:<14} {:<8} {:<8} {:<14} {}",
                                d.device, size, fs, label, mp
                            ),
                            style,
                        ),
                    ]));
                }
            }
            let view = viewport_around(&body_lines, *sel, body_h as usize);
            f.render_widget(Paragraph::new(view), body_rect);

            // Footer hints.
            let footer_rect = ratatui::layout::Rect {
                y: inner.y + inner.height.saturating_sub(footer_rows),
                height: footer_rows,
                ..inner
            };
            let footer = vec![
                Line::from(""),
                Line::from(Span::styled(
                    " enter cd/mount · u umount · r refresh · esc close",
                    modal::dim_style(&theme),
                )),
            ];
            f.render_widget(Paragraph::new(footer), footer_rect);
        }
        (crate::keymap::Mode::Search | crate::keymap::Mode::Input, _) => {
            use crate::ui::modal;
            use ratatui::style::{Modifier, Style};
            use ratatui::text::{Line, Span};
            let title = match app.mode {
                crate::keymap::Mode::Search => "Search",
                _ => match app.pending {
                    Some(crate::keymap::Action::PathInput) => "Go to path",
                    Some(crate::keymap::Action::Create) => "Create",
                    Some(crate::keymap::Action::Rename) => "Rename",
                    Some(crate::keymap::Action::OpenWith) => "Open with (command)",
                    Some(crate::keymap::Action::ShellOnce) => "Shell",
                    _ => "Input",
                },
            };
            // Input modal: prompt row + (optional) hint row → 3 inner rows.
            let r = modal::adaptive_rect(60, 5, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = modal::modal_block(title, &theme);
            let inner = modal::pad_inner(block.inner(r));
            f.render_widget(block, r);

            // Compute prompt window. Reserve last row for the hint line.
            let input_rect = ratatui::layout::Rect {
                height: 1,
                ..inner
            };
            let hint_rect = ratatui::layout::Rect {
                y: inner.y + inner.height.saturating_sub(1),
                height: 1,
                ..inner
            };
            let val = app.input.value();
            let w = input_rect.width.max(1) as usize;
            let nchars = val.chars().count();
            // Account for the 2-col "❯ " prefix in the rendered prompt.
            let avail = w.saturating_sub(2).max(1);
            let (shown, cursor_col): (String, u16) = if nchars < avail {
                (val.clone(), app.input.caret() as u16)
            } else {
                let caret = app.input.caret();
                let start = caret.saturating_sub(avail - 1);
                let s: String = val.chars().skip(start).take(avail - 1).collect();
                (s, (caret - start) as u16)
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "❯ ",
                        Style::default()
                            .fg(theme.match_hl)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(shown),
                ])),
                input_rect,
            );
            f.set_cursor_position((input_rect.x + 2 + cursor_col, input_rect.y));
            f.render_widget(
                Paragraph::new(Span::styled(
                    " enter accept · esc cancel",
                    modal::dim_style(&theme),
                )),
                hint_rect,
            );
        }
        (crate::keymap::Mode::Confirm, _) => {
            use crate::ui::modal;
            use ratatui::style::{Modifier, Style};
            use ratatui::text::{Line, Span};
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
            // 5 content rows (header, blank, name, blank, buttons) + 2 border + 1 slack
            let r = modal::adaptive_rect(56, 8, body);
            f.render_widget(ratatui::widgets::Clear, r);
            let block = modal::modal_block("Confirm delete", &theme);
            let inner = modal::pad_inner(block.inner(r));
            f.render_widget(block, r);
            let lines = vec![
                Line::from(Span::styled(
                    "Permanently delete:",
                    Style::default()
                        .fg(theme.git_conflict)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("  {name}")),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " y ",
                        Style::default()
                            .bg(theme.git_conflict)
                            .fg(ratatui::style::Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" delete    "),
                    Span::styled(
                        " n ",
                        Style::default()
                            .bg(theme.dim)
                            .fg(ratatui::style::Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" cancel"),
                ]),
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        (crate::keymap::Mode::Normal, _) => {}
    }
}
