//! Modal-overlay renderers (op progress, sort menu, help, input, confirm,
//! devices picker, app picker).
//!
//! [`render_overlay`] is the entry point — it inspects `(app.mode,
//! app.dialog)` and dispatches to the right per-overlay function.
//! Order matters: SortMenu and Help live on Mode (not Dialog) so they
//! must be matched before the Dialog catch-all.
//!
//! [`render_op_modal`] is intentionally separate from [`render_overlay`]
//! — it is rendered before other overlays and unconditionally takes
//! precedence so a long-running op is always visible.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Gauge, Paragraph};
use ratatui::Frame;

use crate::app::{App, Dialog};
use crate::keymap::{Action, Keymap, Mode};
use crate::ops::OpState;
use crate::ui::modal;
use crate::ui::theme::Theme;
use crate::ui::viewport_around;

/// Render the in-flight long-op modal (copy/move/extract). Byte-tracked
/// ops show a Gauge; indeterminate ops (extract) show a spinner.
pub fn render_op_modal(f: &mut Frame, body: Rect, op: &OpState, theme: &Theme) {
    let r = modal::adaptive_rect(64, 7, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block(op.kind.verb(), theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Row 0: label + elapsed.
    let elapsed = op.started_at.elapsed().as_secs();
    let label_rect = Rect {
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

    // Row 2: gauge or spinner.
    let bar_rect = Rect {
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
            .gauge_style(Style::default().fg(theme.symlink).bg(theme.selected_bg))
            .percent(pct)
            .label(format!(
                "{} / {}  ·  {}%",
                crate::status::human_bytes(op.done_bytes),
                crate::status::human_bytes(op.total_bytes),
                pct
            ));
        f.render_widget(gauge, bar_rect);
    }

    // Footer hint.
    let footer_rect = Rect {
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

/// Dispatch the (mode, dialog) pair to the correct overlay renderer.
/// SortMenu and Help precede the Dialog::None catch-all because they
/// live on Mode rather than Dialog.
pub fn render_overlay(
    f: &mut Frame,
    body: Rect,
    app: &App,
    theme: &Theme,
    keymap: &Keymap,
) {
    match (&app.mode, &app.dialog) {
        (Mode::SortMenu, _) => {
            let r = modal::adaptive_rect(44, 13, body);
            crate::sort::render_menu(r, f, &app.sort, theme);
        }
        (Mode::Help, _) => {
            let entries = crate::help::build_entries(keymap);
            // Always request a large rect; adaptive_rect clamps it to
            // 80% of the terminal. Scrolling handles tall content.
            let r = modal::adaptive_rect(78, 40, body);
            crate::help::render(r, f, &entries, theme, app.help_scroll, &app.help_filter);
        }
        (_, Dialog::None) => {}
        (_, Dialog::AppPick { items, sel, filter }) => {
            render_appick(f, body, theme, items, *sel, filter);
        }
        (_, Dialog::ScriptPick { items, sel, filter }) => {
            render_scriptpick(f, body, theme, items, *sel, filter);
        }
        (_, Dialog::Devices { items, sel }) => {
            render_devices(f, body, theme, items, *sel);
        }
        (Mode::Search | Mode::Input, _) => render_input(f, body, app, theme),
        (Mode::Confirm, _) => render_confirm(f, body, app, theme),
        (Mode::Normal, _) => {}
    }
}

fn render_appick(
    f: &mut Frame,
    body: Rect,
    theme: &Theme,
    items: &[crate::apps::AppEntry],
    sel: usize,
    filter: &str,
) {
    // Filtered indices into `items`. Custom-command row is always the
    // last picker slot regardless of filter.
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
    // Adaptive sizing: width tracks longest app name (capped), height
    // fits filter + sep + list (capped) + footer + borders.
    let max_name = items.iter().map(|a| a.name.len()).max().unwrap_or(20);
    let desired_w = (max_name as u16 + 10).clamp(40, 64);
    let visible_list = list_len.min(12) as u16;
    let desired_h = 1 /* filter */ + 1 /* sep */ + visible_list + 2 /* footer */ + 2 /* border */;
    let r = modal::adaptive_rect(desired_w, desired_h, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block("Open with", theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Header: filter line + separator row.
    let header_rect = Rect {
        height: 2,
        ..inner
    };
    let filter_line = if filter.is_empty() {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.match_hl)),
            Span::styled("type to filter…", modal::dim_style(theme)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "/ ",
                Style::default()
                    .fg(theme.match_hl)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(filter.to_string()),
            Span::styled("▏", Style::default().fg(theme.match_hl)),
        ])
    };
    let sep_line = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        modal::dim_style(theme),
    ));
    f.render_widget(Paragraph::new(vec![filter_line, sep_line]), header_rect);

    // List body: scrollable, selection-highlighted rows.
    let footer_rows: u16 = 2;
    let list_h = inner.height.saturating_sub(2 + footer_rows).max(1);
    let list_rect = Rect {
        y: inner.y + 2,
        height: list_h,
        ..inner
    };
    let mut lines: Vec<Line> = Vec::with_capacity(list_len);
    for (i, &orig) in filtered.iter().enumerate() {
        let style = if i == sel {
            modal::selection_style(theme)
        } else {
            Style::default()
        };
        let marker = if i == sel { "▸ " } else { "  " };
        lines.push(Line::from(Span::styled(
            format!("{marker}{}", items[orig].name),
            style,
        )));
    }
    // Separator before Custom command...
    lines.push(Line::from(Span::styled(
        "  · · ·",
        modal::dim_style(theme),
    )));
    let ci = filtered.len();
    let style = if sel == ci {
        modal::selection_style(theme)
    } else {
        Style::default().fg(theme.dim)
    };
    let cmarker = if sel == ci { "▸ " } else { "  " };
    lines.push(Line::from(Span::styled(
        format!("{cmarker}Custom command…"),
        style,
    )));
    let view = viewport_around(&lines, sel, list_rect.height as usize);
    f.render_widget(Paragraph::new(view), list_rect);

    // Footer hints.
    let footer_rect = Rect {
        y: inner.y + inner.height.saturating_sub(footer_rows),
        height: footer_rows,
        ..inner
    };
    let hints = vec![
        Line::from(""),
        Line::from(Span::styled(
            " enter open · ↑↓ pick · ⌫ del · esc close",
            modal::dim_style(theme),
        )),
    ];
    f.render_widget(Paragraph::new(hints), footer_rect);
}

fn render_scriptpick(
    f: &mut Frame,
    body: Rect,
    theme: &Theme,
    items: &[std::path::PathBuf],
    sel: usize,
    filter: &str,
) {
    let filtered = crate::dispatch::filter_script_indices(items, filter);
    let list_len = filtered.len();
    let names: Vec<String> = items
        .iter()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let max_name = names.iter().map(|s| s.len()).max().unwrap_or(20);
    let desired_w = (max_name as u16 + 10).clamp(40, 64);
    let visible_list = list_len.clamp(1, 12) as u16;
    let desired_h = 1 /* filter */ + 1 /* sep */ + visible_list + 2 /* footer */ + 2 /* border */;
    let r = modal::adaptive_rect(desired_w, desired_h, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block("Run script", theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Header: filter line + separator.
    let header_rect = Rect { height: 2, ..inner };
    let filter_line = if filter.is_empty() {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.match_hl)),
            Span::styled("type to filter…", modal::dim_style(theme)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "/ ",
                Style::default().fg(theme.match_hl).add_modifier(Modifier::BOLD),
            ),
            Span::raw(filter.to_string()),
            Span::styled("▏", Style::default().fg(theme.match_hl)),
        ])
    };
    let sep_line = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        modal::dim_style(theme),
    ));
    f.render_widget(Paragraph::new(vec![filter_line, sep_line]), header_rect);

    // List body.
    let footer_rows: u16 = 2;
    let list_h = inner.height.saturating_sub(2 + footer_rows).max(1);
    let list_rect = Rect { y: inner.y + 2, height: list_h, ..inner };
    let mut lines: Vec<Line> = Vec::with_capacity(list_len);
    for (i, &orig) in filtered.iter().enumerate() {
        let style = if i == sel {
            modal::selection_style(theme)
        } else {
            Style::default()
        };
        let marker = if i == sel { "▸ " } else { "  " };
        lines.push(Line::from(Span::styled(format!("{marker}{}", names[orig]), style)));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("  (no match)", modal::dim_style(theme))));
    }
    let view = viewport_around(&lines, sel, list_rect.height as usize);
    f.render_widget(Paragraph::new(view), list_rect);

    // Footer hints.
    let footer_rect = Rect {
        y: inner.y + inner.height.saturating_sub(footer_rows),
        height: footer_rows,
        ..inner
    };
    let hints = vec![
        Line::from(""),
        Line::from(Span::styled(
            " enter run · ↑↓ pick · ⌫ del · esc close",
            modal::dim_style(theme),
        )),
    ];
    f.render_widget(Paragraph::new(hints), footer_rect);
}

fn render_devices(
    f: &mut Frame,
    body: Rect,
    theme: &Theme,
    items: &[crate::devices::Device],
    sel: usize,
) {
    let body_rows = items.len().max(1) as u16;
    let visible_rows = body_rows.min(14);
    let desired_h = 1 /* header */ + 1 /* sep */ + visible_rows + 2 /* footer */ + 2 /* border */;
    let r = modal::adaptive_rect(78, desired_h, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block("Devices", theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Header row: column titles.
    let header_rect = Rect {
        height: 2,
        ..inner
    };
    let cols = format!(
        "      {:<14} {:<8} {:<8} {:<14} {}",
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
            modal::dim_style(theme),
        )),
    ];
    f.render_widget(Paragraph::new(header_lines), header_rect);

    // Body
    let footer_rows: u16 = 2;
    let body_h = inner.height.saturating_sub(2 + footer_rows).max(1);
    let body_rect = Rect {
        y: inner.y + 2,
        height: body_h,
        ..inner
    };
    let mut body_lines: Vec<Line> = Vec::new();
    if items.is_empty() {
        body_lines.push(Line::from(Span::styled(
            "  no handleable devices",
            modal::dim_style(theme),
        )));
    } else {
        for (i, d) in items.iter().enumerate() {
            let style = if i == sel {
                modal::selection_style(theme)
            } else {
                Style::default()
            };
            let marker = if i == sel { "▸ " } else { "  " };
            // SMB rows get a network glyph; block rows get blank padding
            // so the columns stay aligned (both are 2 cells wide).
            let kind_glyph = if d.is_smb() {
                "\u{f233} "
            } else {
                "  "
            };
            let mounted_span = if d.is_mounted() {
                Span::styled(
                    "● ",
                    Style::default()
                        .fg(theme.git_added)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("○ ", modal::dim_style(theme))
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
                Span::styled(kind_glyph, style),
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
    let view = viewport_around(&body_lines, sel, body_h as usize);
    f.render_widget(Paragraph::new(view), body_rect);

    // Footer hints.
    let footer_rect = Rect {
        y: inner.y + inner.height.saturating_sub(footer_rows),
        height: footer_rows,
        ..inner
    };
    let footer = vec![
        Line::from(""),
        Line::from(Span::styled(
            " enter cd/mount · a add · d delete · u umount · r refresh · esc close",
            modal::dim_style(theme),
        )),
    ];
    f.render_widget(Paragraph::new(footer), footer_rect);
}

fn render_input(f: &mut Frame, body: Rect, app: &App, theme: &Theme) {
    let title = match app.mode {
        Mode::Search => "Search",
        _ => match app.pending {
            Some(Action::PathInput) => "Go to path",
            Some(Action::Create) => "Create",
            Some(Action::Rename) => "Rename",
            Some(Action::OpenWith) => "Open with (command)",
            Some(Action::ShellOnce) => "Shell",
            Some(Action::AddSmb) => "Add SMB share (smb://user@host/share)",
            _ => "Input",
        },
    };
    // Input modal: prompt row + (optional) hint row → 3 inner rows.
    let r = modal::adaptive_rect(60, 5, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block(title, theme);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);

    // Compute prompt window. Reserve last row for the hint line.
    let input_rect = Rect {
        height: 1,
        ..inner
    };
    let hint_rect = Rect {
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
            modal::dim_style(theme),
        )),
        hint_rect,
    );
}

fn render_confirm(f: &mut Frame, body: Rect, app: &App, theme: &Theme) {
    // Show what will actually be deleted: a count when multiple files are
    // marked, otherwise the single target's name. Mirrors `App::targets`
    // (marks take precedence over the cursor) so the dialog never claims
    // one file while deleting many.
    let name = if app.marks.len() > 1 {
        format!("{} items", app.marks.len())
    } else {
        app.targets()
            .first()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    // The same confirm modal serves permanent-delete and (optional)
    // trash; the header (and border accent) reflect which one is pending.
    let trashing = matches!(app.pending, Some(crate::keymap::Action::Trash));
    let (title, header, accent) = if trashing {
        ("Confirm trash", "Move to trash:", theme.mark)
    } else {
        ("Confirm delete", "Permanently delete:", theme.git_conflict)
    };
    // 6 content rows (header, blank, name, size, blank, buttons) + 2 border + 1 slack
    let r = modal::adaptive_rect(56, 9, body);
    f.render_widget(Clear, r);
    let block = modal::modal_block_colored(title, accent);
    let inner = modal::pad_inner(block.inner(r));
    f.render_widget(block, r);
    // Dim subline: count + total byte weight of what's about to go.
    let bytes = crate::status::human_bytes(app.targets_bytes());
    let n = app.targets().len();
    let summary = if n > 1 {
        format!("  {n} items • {bytes}")
    } else {
        format!("  {bytes}")
    };
    let lines = vec![
        Line::from(Span::styled(
            header,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("  {name}")),
        Line::from(Span::styled(summary, modal::dim_style(theme))),
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
