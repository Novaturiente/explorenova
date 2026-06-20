//! Reusable floating-overlay geometry + styled building blocks.
//!
//! All app modals (sort menu, help, open with, devices, input prompt,
//! confirm, op progress) route through these helpers so spacing,
//! borders, colors, and adaptive sizing stay consistent.

use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

/// Adaptive floating rect. Takes a *desired* content size (border-
/// inclusive); shrinks to leave margin inside `area`, never grows past
/// content_w/content_h. Falls back to a tiny rect on cramped terminals
/// instead of panicking.
///
/// Margins: 4 cols left/right, 2 rows top/bottom. Min size: 20 × 3
/// (or area dims, whichever is smaller).
pub fn adaptive_rect(content_w: u16, content_h: u16, area: Rect) -> Rect {
    const MARGIN_W: u16 = 4;
    const MARGIN_H: u16 = 2;
    const MIN_W: u16 = 20;
    const MIN_H: u16 = 3;
    let max_w = area.width.saturating_sub(MARGIN_W * 2).max(1);
    let max_h = area.height.saturating_sub(MARGIN_H * 2).max(1);
    let w = content_w.min(max_w).max(MIN_W.min(area.width));
    let h = content_h.min(max_h).max(MIN_H.min(area.height));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w, height: h }
}

/// Shrink a modal rect to its inner content area minus 1 col of left/
/// right padding. Use after `block.inner(rect)` so text has breathing
/// room from the border.
pub fn pad_inner(inner: Rect) -> Rect {
    Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

/// Consistent modal Block: rounded border + title, both in the theme
/// header accent so popups stand out from the dimmed tree behind them.
pub fn modal_block(title: &str, theme: &Theme) -> Block<'static> {
    modal_block_colored(title, theme.header)
}

/// Like [`modal_block`] but with an explicit border/title accent — used
/// by the confirm dialog to signal danger (red delete, peach trash).
pub fn modal_block_colored(title: &str, accent: Color) -> Block<'static> {
    let title_span = Span::styled(
        format!(" {title} "),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    );
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(title_span)
}

/// Selection-row style for lists inside modals.
pub fn selection_style(theme: &Theme) -> Style {
    Style::default()
        .bg(theme.selected_bg)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// Dim style for hints/footers/separators.
pub fn dim_style(theme: &Theme) -> Style {
    Style::default().fg(theme.dim)
}

/// Move `sel` within `0..len` with wrap-around. Returns the new index.
pub fn wrap_nav(sel: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (sel + 1) % len
    } else {
        (sel + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn wrap_nav_cycles() {
        assert_eq!(wrap_nav(0, 3, false), 2);
        assert_eq!(wrap_nav(2, 3, true), 0);
        assert_eq!(wrap_nav(1, 3, true), 2);
        assert_eq!(wrap_nav(0, 0, true), 0);
    }

    #[test]
    fn adaptive_respects_content_size() {
        let area = Rect { x: 0, y: 0, width: 120, height: 50 };
        let r = adaptive_rect(60, 20, area);
        assert_eq!(r.width, 60);
        assert_eq!(r.height, 20);
        assert_eq!(r.x, 30);
        assert_eq!(r.y, 15);
    }

    #[test]
    fn adaptive_shrinks_on_small_terminal() {
        let area = Rect { x: 0, y: 0, width: 40, height: 12 };
        let r = adaptive_rect(120, 30, area);
        // 40 - 4*2 = 32, 12 - 2*2 = 8
        assert_eq!(r.width, 32);
        assert_eq!(r.height, 8);
    }

    #[test]
    fn adaptive_handles_tiny_area() {
        let area = Rect { x: 0, y: 0, width: 5, height: 4 };
        let r = adaptive_rect(60, 20, area);
        assert!(r.width <= area.width);
        assert!(r.height <= area.height);
        assert!(r.x + r.width <= area.x + area.width);
        assert!(r.y + r.height <= area.y + area.height);
    }

    #[test]
    fn pad_inner_shrinks_left_right() {
        let r = Rect { x: 10, y: 5, width: 20, height: 8 };
        let p = pad_inner(r);
        assert_eq!(p.x, 11);
        assert_eq!(p.width, 18);
        assert_eq!(p.y, 5);
        assert_eq!(p.height, 8);
    }
}
