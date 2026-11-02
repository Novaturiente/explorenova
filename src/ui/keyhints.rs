//! Bottom key-hint bar — a superfile-style row of "key label" chips for
//! the current mode's most-used actions. Reads real bindings via
//! `Keymap::keys_for` so a remapped key shows correctly, not a stale
//! default.

use crate::app::App;
use crate::keymap::{Action, Keymap, Mode};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

const CORE_HINTS: &[(Action, &str)] = &[
    (Action::Down, "move"),
    (Action::Open, "open"),
    (Action::ToggleMark, "mark"),
    (Action::Copy, "yank"),
    (Action::Cut, "cut"),
    (Action::Paste, "paste"),
    (Action::Trash, "trash"),
    (Action::Search, "search"),
    (Action::Help, "help"),
    (Action::Quit, "quit"),
];

/// Render the key-hint row. Only shown in Normal mode — every other mode
/// (search/input/confirm/sort/help/pickers) already renders its own hint
/// text inside its overlay, so a second row here would just be noise.
pub fn render(area: Rect, f: &mut Frame, app: &App, keymap: &Keymap, theme: &Theme) {
    if app.mode != Mode::Normal {
        return;
    }
    let key_style = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.dim);
    let sep_style = Style::default().fg(theme.dim);

    let mut spans: Vec<Span> = Vec::new();
    let mut width = 0usize;
    for (i, (action, label)) in CORE_HINTS.iter().enumerate() {
        let key = keymap
            .keys_for(*action)
            .into_iter()
            .next()
            .unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        let chip_w = key.width() + 1 + label.width();
        let sep_w = if i == 0 { 0 } else { 2 };
        if width + sep_w + chip_w > area.width as usize {
            break;
        }
        if i > 0 {
            spans.push(Span::styled("  ", sep_style));
            width += sep_w;
        }
        spans.push(Span::styled(key, key_style));
        spans.push(Span::styled(format!(" {label}"), label_style));
        width += chip_w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
