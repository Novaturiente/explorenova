//! Per-pane renderers (tree, bookmarks, preview) and the top header.
//!
//! These are the inside of `render_frame`'s per-pane loop, lifted into
//! standalone functions so each pane's rendering can be reasoned about
//! independently. The split-mode shenanigans (active slot, tab swap)
//! stay in [`render_pane_tree`] because they are tree-pane specific.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, BorderType, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::preview::{PreviewCache, PreviewKind};
use crate::ui::theme::Theme;
use crate::ui::{render_image, short_bookmark_label, ImageCache};

/// Render the top header bar showing the current arena root path.
/// Left-truncates with an ellipsis when the path exceeds the area width.
pub fn render_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let root = &app.arena.nodes[app.arena.root].path;
    // A gvfs SMB mount has an unreadable dir name
    // (smb-share:server=...,share=...); show "share on server" instead.
    let root_path = crate::smb::gvfs_display_label(root)
        .unwrap_or_else(|| root.display().to_string());
    let w = area.width.max(1) as usize;
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
    let items: Vec<ListItem> = visible_ref
        .iter()
        .enumerate()
        .map(|(idx, &id)| {
            let path = &arena_ref.nodes[id].path;
            ListItem::new(crate::ui::panes::tree_row(
                arena_ref,
                id,
                idx == selected_ref,
                theme,
                app.config.icons,
                marks_ref.contains(path),
                cut_paths.is_some_and(|ps| ps.iter().any(|p| p == path)),
                inner_w,
            ))
        })
        .collect();
    let is_active = split && app_here;
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
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(Span::styled(title_text, title_style)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );
    let mut st = ListState::default();
    st.select(Some(selected_ref));
    f.render_stateful_widget(list, area, &mut st);
}

/// Render the bookmarks pane. Highlights the selected bookmark only
/// when the bookmarks pane is the focused one.
pub fn render_pane_bookmarks(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.focus == crate::app::Pane::Bookmarks;
    let items: Vec<ListItem> = app
        .state
        .bookmarks
        .iter()
        .map(|b| ListItem::new(short_bookmark_label(b)))
        .collect();
    let title_text = if focused {
        " bookmarks ● "
    } else {
        " bookmarks "
    };
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
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(Span::styled(title_text, title_style)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );
    let mut st = ListState::default();
    if focused && !app.state.bookmarks.is_empty() {
        let i = app.bookmark_sel.min(app.state.bookmarks.len() - 1);
        st.select(Some(i));
    }
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

    const PREVIEW_DEBOUNCE_MS: u128 = 150;
    let path = app.arena.nodes[id].path.clone();

    // Settle gate: only do the (potentially expensive) get_or_render
    // call once the cursor has been on this row for >= 150ms. Cached
    // entries short-circuit so revisited paths are instant.
    let elapsed = app.cursor_moved_at.elapsed().as_millis() >= PREVIEW_DEBOUNCE_MS;
    let cached = cache.contains(&path);
    if !(elapsed || cached) {
        f.render_widget(Paragraph::new("(loading…)"), inner);
        return;
    }

    let kind = cache.get_or_render(&path, app.config.preview_max_bytes);
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
                f.render_widget(Paragraph::new("(loading…)"), inner);
            }
        }
        PreviewKind::Text(t) => f.render_widget(Paragraph::new(t), inner),
        PreviewKind::Meta(t) => f.render_widget(Paragraph::new(t), inner),
    }
}
