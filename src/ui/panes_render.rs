//! Per-pane renderers (tree, bookmarks, preview) and the top header.
//!
//! These are the inside of `render_frame`'s per-pane loop, lifted into
//! standalone functions so each pane's rendering can be reasoned about
//! independently. The split-mode shenanigans (active slot, tab swap)
//! stay in [`render_pane_tree`] because they are tree-pane specific.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::preview::{PreviewCache, PreviewKind};
use crate::ui::theme::Theme;
use crate::ui::modal;
use crate::ui::{render_image, ImageCache};

/// Render dim, centered placeholder text inside `area` — used for the
/// empty states of the tree and bookmarks panes (e.g. an empty directory
/// or no saved bookmarks) so they never show a blank void.
fn render_centered_hint(f: &mut Frame, area: Rect, theme: &Theme, lines: &[&str]) {
    if area.height == 0 {
        return;
    }
    let h = (lines.len() as u16).min(area.height);
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect { y, height: h, ..area };
    let para = Paragraph::new(
        lines
            .iter()
            .map(|l| Line::from(Span::styled(*l, Style::default().fg(theme.dim))))
            .collect::<Vec<_>>(),
    )
    .alignment(Alignment::Center);
    f.render_widget(para, rect);
}

/// Render the top header bar showing the current arena root path.
/// Left-truncates with an ellipsis when the path exceeds the area width.
pub fn render_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let root = &app.arena.nodes[app.arena.root].path;
    // A gvfs SMB mount has an unreadable dir name
    // (smb-share:server=...,share=...); show "share on server" instead.
    let root_path = crate::ui::display_name(
        &crate::smb::gvfs_display_label(root).unwrap_or_else(|| root.display().to_string()),
    );
    let w = area.width.max(1) as usize;
    // Truncate by display cells so wide (CJK/emoji) path components
    // don't overflow the header line.
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let shown = if root_path.width() > w {
        let budget = w.saturating_sub(1); // room for the ellipsis
        let mut tail_rev: Vec<char> = Vec::new();
        let mut used = 0usize;
        for c in root_path.chars().rev() {
            let cw = c.width().unwrap_or(0);
            if used + cw > budget {
                break;
            }
            tail_rev.push(c);
            used += cw;
        }
        let tail: String = tail_rev.into_iter().rev().collect();
        format!("…{tail}")
    } else {
        root_path
    };
    f.render_widget(
        Paragraph::new(shown).style(
            Style::default()
                .fg(theme.header)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

/// Render the tree pane. In split mode both `tree` and `tree_other`
/// slots are valid; only the one matching `app.active_slot` reads from
/// the live `App` fields, the other reads from `app.tabs[0]`. This
/// keeps screen positions fixed across Tab swaps — only the focus
/// marker moves.
pub fn render_pane_tree(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    slot: u8,
    split: bool,
) {
    let app_here = !split || app.active_slot == slot;
    let inner_w = area.width.saturating_sub(2);
    let (arena_ref, visible_ref, selected_ref, marks_ref): (
        &crate::tree::Arena,
        &Vec<usize>,
        usize,
        &std::collections::BTreeSet<std::path::PathBuf>,
    ) = if app_here {
        (&app.arena, &app.visible, app.selected, &app.marks)
    } else {
        let Some(snap) = app.tabs.first() else { return };
        (&snap.arena, &snap.visible, snap.selected, &snap.marks)
    };
    // Cut items (dd) render dimmed. Clipboard is shared across tabs, so
    // read app.clipboard regardless of which slot we're drawing.
    let cut_paths: Option<&[std::path::PathBuf]> = match &app.clipboard {
        Some((paths, true)) => Some(paths),
        _ => None,
    };
    let win_h = area.height.saturating_sub(2) as usize;
    let start = crate::ui::viewport_start(visible_ref.len(), selected_ref, win_h);
    let end = (start + win_h).min(visible_ref.len());
    let items: Vec<ListItem> = visible_ref[start..end]
        .iter()
        .enumerate()
        .map(|(off, &id)| {
            let path = &arena_ref.nodes[id].path;
            ListItem::new(crate::ui::panes::tree_row(
                arena_ref,
                id,
                start + off == selected_ref,
                theme,
                app.config.icons,
                marks_ref.contains(path),
                cut_paths.is_some_and(|ps| ps.iter().any(|p| p == path)),
                inner_w,
                app.line_mode,
            ))
        })
        .collect();
    let is_active = (split && app_here) || (!split && app.focus == crate::app::Pane::Tree);
    let title_text = if is_active { " tree ● " } else { " tree " };
    let title_style = if is_active {
        Style::default().fg(theme.mark).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.header)
            .add_modifier(Modifier::BOLD)
    };
    let border_style = if is_active {
        Style::default().fg(theme.mark)
    } else {
        Style::default().fg(theme.dim)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(title_text, title_style));
    // Empty directory: render the framed pane + a centered hint instead of
    // a blank box (an underflowing `selected - start` would also panic here).
    if visible_ref.is_empty() {
        let inner = block.inner(area);
        f.render_widget(block, area);
        render_centered_hint(f, inner, theme, &["(empty)"]);
        return;
    }
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );
    let mut st = ListState::default();
    st.select(Some(selected_ref - start));
    f.render_stateful_widget(list, area, &mut st);
}

/// Icon + color span pair for a sidebar row's path (dir/file/etc, same
/// classification the tree pane uses).
fn path_icon_spans(path: &std::path::Path, label: &str, theme: &Theme) -> (String, ratatui::style::Color) {
    let kind = if path.is_dir() {
        crate::tree::NodeKind::Dir
    } else {
        crate::tree::NodeKind::File
    };
    let color = match crate::icons::classify(kind, false, label) {
        crate::icons::Kind::Dir => theme.dir,
        crate::icons::Kind::Symlink => theme.symlink,
        crate::icons::Kind::Exec => theme.executable,
        crate::icons::Kind::Media => theme.media,
        crate::icons::Kind::Archive => theme.archive,
        crate::icons::Kind::File => theme.file,
    };
    (crate::icons::icon_for(kind, false, label).to_string(), color)
}

fn sidebar_row_item(row: &crate::sidebar::Row, app: &App, theme: &Theme) -> ListItem<'static> {
    let label = row.label.clone();
    if !app.config.icons {
        return ListItem::new(label);
    }
    match (&row.path, &row.device) {
        (_, Some(d)) => {
            let kind_glyph = if d.is_smb() { "\u{f233}" } else { "\u{f0a0}" };
            let (mark, mark_color) = if d.is_mounted() {
                ("● ", theme.git_added)
            } else {
                ("○ ", theme.dim)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{kind_glyph} "), Style::default().fg(theme.symlink)),
                Span::styled(mark, Style::default().fg(mark_color)),
                Span::styled(label, Style::default().fg(theme.file)),
            ]))
        }
        (Some(p), None) => {
            let (ic, color) = path_icon_spans(p, &label, theme);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{ic} "), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(color)),
            ]))
        }
        (None, None) => ListItem::new(label),
    }
}

/// Render the sidebar pane: three stacked, non-editable-header groups
/// (Bookmarks = auto well-known dirs, Pinned = user's own, Disks =
/// mounted devices/shares) sharing one flat selection index
/// (`app.bookmark_sel`), built by [`crate::sidebar::build`].
pub fn render_pane_bookmarks(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.focus == crate::app::Pane::Bookmarks;
    let rows = crate::sidebar::build(app);

    let header_style = Style::default().fg(theme.header).add_modifier(Modifier::BOLD | Modifier::ITALIC);
    let mut items: Vec<ListItem> = Vec::new();
    let mut select_at: Option<usize> = None;
    let mut sel_idx = 0usize;
    for section in [
        crate::sidebar::Section::Bookmarks,
        crate::sidebar::Section::Pinned,
        crate::sidebar::Section::Disks,
    ] {
        items.push(ListItem::new(Span::styled(
            format!(" {} ", section.title()),
            header_style,
        )));
        let mut any = false;
        for row in rows.iter().filter(|r| r.section == section) {
            any = true;
            if focused && sel_idx == app.bookmark_sel {
                select_at = Some(items.len());
            }
            sel_idx += 1;
            items.push(sidebar_row_item(row, app, theme));
        }
        if !any {
            items.push(ListItem::new(Span::styled("  (none)", modal::dim_style(theme))));
        }
    }

    let title_text = if focused { " sidebar ● " } else { " sidebar " };
    let title_style = if focused {
        Style::default().fg(theme.mark).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.header)
            .add_modifier(Modifier::BOLD)
    };
    let border_style = if focused {
        Style::default().fg(theme.mark)
    } else {
        Style::default().fg(theme.dim)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(title_text, title_style));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );
    let mut st = ListState::default();
    st.select(select_at);
    f.render_stateful_widget(list, area, &mut st);
}

/// Render the preview pane. Honours a 150ms settle gate so cursor
/// movement does not thrash the (potentially expensive) renderer; cache
/// hits skip the gate.
pub fn render_pane_preview(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    cache: &mut PreviewCache,
    picker: &mut Picker,
    img_cache: &mut ImageCache,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.dim))
        .title(Span::styled(
            " preview ",
            Style::default()
                .fg(theme.header)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(&id) = app.visible.get(app.selected) else {
        return;
    };

    let path = app.arena.nodes[id].path.clone();

    // Settle gate: only do the (potentially expensive) get_or_render call
    // once the cursor has rested on this row for the type's settle delay.
    // Heavy types (archive/pdf/video/xlsx/avif) wait longer so scrolling
    // past them never triggers their blocking generation. Cached entries
    // short-circuit so revisited paths are instant.
    let debounce_ms = crate::preview::settle_ms(
        &path,
        app.config.preview.settle_cheap_ms,
        app.config.preview.settle_heavy_ms,
    ) as u128;
    let elapsed = app.cursor_moved_at.elapsed().as_millis() >= debounce_ms;
    let cached = cache.contains(&path);
    let spinner_line = || {
        Line::from(vec![
            Span::styled(
                format!("{} ", crate::ui::modal::spinner_char(app.cursor_moved_at.elapsed().as_millis())),
                Style::default().fg(theme.symlink).add_modifier(Modifier::BOLD),
            ),
            Span::styled("loading…", crate::ui::modal::dim_style(theme)),
        ])
    };

    if !(elapsed || cached) {
        f.render_widget(Paragraph::new(spinner_line()), inner);
        return;
    }

    // Fire-and-forget: queues a render if needed (no-op if already cached
    // or in flight), then `get()` picks up whatever's there right now.
    // Both are pure map lookups — no filesystem access on this thread.
    cache.request(&path, app.config.preview_max_bytes);
    let Some(kind) = cache.get(&path) else {
        f.render_widget(Paragraph::new(spinner_line()), inner);
        return;
    };
    match &*kind {
        PreviewKind::Image(p, img) => {
            let already_decoded = img_cache
                .key
                .as_ref()
                .map(|(k, cached_img)| k == p && std::sync::Arc::ptr_eq(cached_img, img))
                .unwrap_or(false);
            if already_decoded || elapsed {
                render_image(f, inner, picker, img_cache, p, img);
            } else {
                f.render_widget(Paragraph::new("(loading…)"), inner);
            }
        }
        PreviewKind::Text(t) => {
            // ctrl-j/ctrl-k scroll; clamp so the last line stays on screen.
            let max = t.lines.len().saturating_sub(1);
            let off = app.preview_scroll.min(max) as u16;
            // Borrow the styled spans instead of deep-cloning the Text —
            // Paragraph needs an owned value, but a borrowed view only
            // reallocates the container Vecs, not every span's String.
            f.render_widget(
                Paragraph::new(crate::preview::borrow_text(t)).scroll((off, 0)),
                inner,
            )
        }
        PreviewKind::Meta(t) => f.render_widget(Paragraph::new(t.as_str()), inner),
    }
}
