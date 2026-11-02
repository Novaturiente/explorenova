//! Superfile-style footer: processes | metadata | clipboard, display only.

use crate::app::App;
use crate::ui::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph};
use ratatui::Frame;

/// Footer height including borders.
pub const HEIGHT: u16 = 10;

/// Is the footer drawn this frame? Hidden by `F`, and on short terminals
/// where it would squeeze the tree to nothing.
pub fn shown(app: &App, body_height: u16) -> bool {
    !app.state.hide_footer && body_height >= HEIGHT + 10
}

pub fn render(f: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(1, 3), Constraint::Ratio(1, 3)])
        .split(area);
    render_processes(f, cols[0], app, theme);
    render_metadata(f, cols[1], app, theme);
    render_clipboard(f, cols[2], app, theme);
}

fn panel(title: String, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.dim))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(theme.header).add_modifier(Modifier::BOLD),
        ))
}

fn dim(theme: &Theme) -> Style {
    Style::default().fg(theme.dim)
}

fn render_processes(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = panel("processes".into(), theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut y = inner.y;
    if let Some(op) = &app.op {
        let spin = crate::ui::modal::spinner_char(op.started_at.elapsed().as_millis());
        let head = Line::from(vec![
            Span::styled(format!("{spin} "), Style::default().fg(theme.accent)),
            Span::styled(
                crate::ui::display_name(&op.label), // already reads "Copying 3 items"
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}s · esc cancels", op.started_at.elapsed().as_secs()), dim(theme)),
        ]);
        f.render_widget(Paragraph::new(head), Rect { y, height: 1, ..inner });
        y += 1;
        if y < inner.y + inner.height {
            let bar = Rect { y, height: 1, ..inner };
            match op.progress() {
                (Some(pct), text) => f.render_widget(
                    Gauge::default()
                        .gauge_style(Style::default().fg(theme.accent).bg(theme.selected_bg))
                        .percent(pct)
                        .label(text),
                    bar,
                ),
                (None, text) => f.render_widget(Paragraph::new(Span::styled(text, dim(theme))), bar),
            }
            y += 1;
        }
    }
    for rec in app.op_history.iter().rev() {
        let (mark, color, msg) = match &rec.result {
            Ok(s) if s.starts_with("cancelled") => ("■", theme.dim, s.as_str()),
            Ok(s) => ("✓", theme.git_added, s.as_str()),
            Err(e) => ("✗", theme.git_conflict, e.as_str()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), Style::default().fg(color)),
            Span::raw(crate::ui::display_name(msg)),
        ]));
    }
    if app.op.is_none() && lines.is_empty() {
        lines.push(Line::from(Span::styled("no operations yet", dim(theme))));
    }
    let rest = Rect { y, height: (inner.y + inner.height).saturating_sub(y), ..inner };
    f.render_widget(Paragraph::new(lines), rest);
}

fn render_metadata(f: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let block = panel("metadata".into(), theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(&id) = app.visible.get(app.selected) else { return };
    let path = app.arena.nodes[id].path.clone();
    let is_dir = app.arena.nodes[id].is_dir();
    // Same rest delay as heavy previews, so scrolling past folders doesn't
    // queue a size walk for each one.
    if app.meta.wants_extra(&path, is_dir)
        && app.cursor_moved_at.elapsed().as_millis() as u64 >= app.config.preview.settle_heavy_ms
    {
        app.meta.request(&path);
    }
    let mut rows = app.meta.basic(&path).clone();
    if let Some(extra) = app.meta.extra(&path) {
        rows.extend(extra.iter().cloned());
    } else if app.meta.is_loading(&path) {
        let spin = crate::ui::modal::spinner_char(app.cursor_moved_at.elapsed().as_millis());
        rows.push(("…".into(), format!("{spin} computing")));
    }
    let key_w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0).min(14);
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<key_w$}  "), dim(theme)),
                Span::raw(v),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_clipboard(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let (title, lines) = match &app.clipboard {
        Some((paths, is_cut)) => {
            let title = format!("clipboard ({}, {})", if *is_cut { "cut" } else { "copy" }, paths.len());
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let lines = paths
                .iter()
                .map(|p| {
                    let shown = match home.as_ref().and_then(|h| p.strip_prefix(h).ok()) {
                        Some(rel) => format!("~/{}", rel.display()),
                        None => p.display().to_string(),
                    };
                    Line::from(Span::raw(crate::ui::display_name(&shown)))
                })
                .collect();
            (title, lines)
        }
        None => (
            "clipboard".to_string(),
            vec![Line::from(Span::styled("empty · yy copy · dd cut", dim(theme)))],
        ),
    };
    let block = panel(title, theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}
