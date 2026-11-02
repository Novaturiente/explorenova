use crate::keymap::{Action, Keymap};
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

pub struct HelpCategory {
    pub name: &'static str,
    pub rows: Vec<(String, String)>, // (label, keys)
}

/// (Action, category, human label). Order within a category preserved in render.
const ACTION_META: &[(Action, &str, &str)] = &[
    // Navigation
    (Action::Down, "Navigation", "Down"),
    (Action::Up, "Navigation", "Up"),
    (Action::Top, "Navigation", "Top"),
    (Action::Bottom, "Navigation", "Bottom"),
    (Action::HalfDown, "Navigation", "Half page down"),
    (Action::HalfUp, "Navigation", "Half page up"),
    (Action::Open, "Navigation", "Expand / open"),
    (Action::Collapse, "Navigation", "Collapse"),
    (Action::Parent, "Navigation", "Up to parent dir"),
    (Action::Toggle, "Navigation", "Toggle / open default"),
    (Action::OpenDefault, "Navigation", "Open default"),
    (Action::OpenWith, "Navigation", "Open with..."),
    (Action::JumpHome, "Navigation", "Jump home"),
    (Action::ZoxideJump, "Navigation", "Zoxide jump"),
    (Action::PathInput, "Navigation", "Path input"),
    // Panes / Tabs
    (Action::FocusNext, "Panes / Tabs", "Cycle focus / swap tabs"),
    (Action::SidebarNextSection, "Panes / Tabs", "Sidebar: jump to next section"),
    (Action::ToggleBookmarksPane, "Panes / Tabs", "Toggle bookmarks pane"),
    (Action::TogglePreviewPane, "Panes / Tabs", "Toggle preview pane"),
    (Action::ToggleFooter, "Panes / Tabs", "Toggle footer (processes / metadata / clipboard)"),
    (Action::PreviewScrollDown, "Panes / Tabs", "Scroll preview down"),
    (Action::PreviewScrollUp, "Panes / Tabs", "Scroll preview up"),
    (Action::ResizeShrink, "Panes / Tabs", "Shrink pane"),
    (Action::ResizeGrow, "Panes / Tabs", "Grow pane"),
    (Action::ReorderLeft, "Panes / Tabs", "Move pane left"),
    (Action::ReorderRight, "Panes / Tabs", "Move pane right"),
    (Action::NewTab, "Panes / Tabs", "New tab (split)"),
    (Action::CloseTab, "Panes / Tabs", "Close tab"),
    (Action::CycleLinemode, "Panes / Tabs", "Cycle line info"),
    // Files
    (Action::Create, "Files", "Create file/dir"),
    (Action::Rename, "Files", "Rename"),
    (Action::BulkRename, "Files", "Bulk rename (editor)"),
    (Action::Copy, "Files", "Copy"),
    (Action::Cut, "Files", "Cut"),
    (Action::Paste, "Files", "Paste"),
    (Action::Trash, "Files", "Move to trash"),
    (Action::PermanentDelete, "Files", "Permanent delete"),
    (Action::OpenTrash, "Files", "Open trash"),
    (Action::SysClipCopy, "Files", "Copy path to system clipboard"),
    // Selection
    (Action::ToggleMark, "Selection", "Toggle mark"),
    (Action::SelectAllVisible, "Selection", "Select all visible"),
    (Action::ToggleVisualMode, "Selection", "Visual mode"),
    // Search / Bookmarks
    (Action::Search, "Search / Bookmarks", "Search (Tab: names / contents)"),
    (Action::SearchNext, "Search / Bookmarks", "Next match"),
    (Action::SearchPrev, "Search / Bookmarks", "Prev match"),
    (Action::TogglePin, "Search / Bookmarks", "Toggle pin"),
    (Action::Filter, "Search / Bookmarks", "Filter tree"),
    (Action::ContentSearch, "Search / Bookmarks", "Content search (rg)"),
    // Shell / Misc
    (Action::ShellOnce, "Shell / Misc", "Shell one-shot ({paths}/{file}/{dir})"),
    (Action::ScriptPick, "Shell / Misc", "Run script from scripts dir"),
    (Action::ShellDrop, "Shell / Misc", "Drop to shell"),
    (Action::ShowDevices, "Shell / Misc", "Devices modal"),
    (Action::ToggleHidden, "Shell / Misc", "Toggle hidden"),
    (Action::Refresh, "Shell / Misc", "Refresh"),
    (Action::SortMenu, "Shell / Misc", "Sort menu"),
    (Action::Help, "Shell / Misc", "Help (this overlay)"),
    (Action::Quit, "Shell / Misc", "Quit"),
];

/// Sort-menu keys. They live in `Keymap::resolve`'s `Mode::SortMenu` arm
/// rather than in `defaults()`, so `keys_for` finds nothing for them and
/// they were missing from the overlay entirely. Keep in sync with keymap.rs.
const SORT_ROWS: &[(&str, &str)] = &[
    ("Sort by name", "1 / n"),
    ("Sort by size", "2 / s"),
    ("Sort by mtime", "3 / m"),
    ("Sort by extension", "4 / e"),
    ("Toggle reverse order", "r"),
    ("Toggle dirs first", "d"),
];

/// Printable name for a chord token. `" "` is a real binding (ToggleMark)
/// but renders as a bold blank, so it needs a word; every other token is
/// already a visible glyph or a name like `ctrl-d`.
fn key_label(chord: &str) -> &str {
    if chord.trim().is_empty() {
        "Space"
    } else {
        chord
    }
}

pub fn build_entries(km: &Keymap) -> Vec<HelpCategory> {
    let mut out: Vec<HelpCategory> = Vec::new();
    for &(action, cat, label) in ACTION_META {
        let keys = km
            .keys_for(action)
            .iter()
            .map(|k| key_label(k))
            .collect::<Vec<_>>()
            .join(" / ");
        if keys.is_empty() {
            continue;
        }
        let row = (label.to_string(), keys);
        if let Some(last) = out.last_mut() {
            if last.name == cat {
                last.rows.push(row);
                continue;
            }
        }
        out.push(HelpCategory {
            name: cat,
            rows: vec![row],
        });
    }
    out.push(HelpCategory {
        name: "Sort menu (after s)",
        rows: SORT_ROWS
            .iter()
            .map(|(l, k)| (l.to_string(), k.to_string()))
            .collect(),
    });
    out
}

fn row_matches(needle: &[&str], category: &str, label: &str, keys: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = format!("{category} {label} {keys}").to_lowercase();
    needle.iter().all(|t| hay.contains(t))
}

/// Build the full list of styled lines for the help overlay. Splitting
/// this from `render` lets callers measure total rows for scroll
/// clamping. `filter` does a case-insensitive multi-token substring
/// match against `category + label + keys`; empty filter shows all.
pub fn build_lines(
    entries: &[HelpCategory],
    theme: &Theme,
    inner_w: usize,
    filter: &str,
) -> Vec<Line<'static>> {
    let needle_owned: String = filter.to_lowercase();
    let toks: Vec<&str> = needle_owned.split_whitespace().collect();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut shown_any = false;
    for cat in entries.iter() {
        let matching: Vec<&(String, String)> = cat
            .rows
            .iter()
            .filter(|(l, k)| row_matches(&toks, cat.name, l, k))
            .collect();
        if matching.is_empty() {
            continue;
        }
        if shown_any {
            lines.push(Line::from(""));
        }
        shown_any = true;
        lines.push(Line::from(Span::styled(
            cat.name.to_string(),
            Style::default()
                .fg(theme.header)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for (label, keys) in matching {
            // Fit "  label  ···  keys" into inner_w. If too tight,
            // drop the dotted leader and just use a single-space gap.
            let prefix_len = 2 + label.len() + 2; // "  label  "
            let suffix_len = 2 + keys.len();       // "  keys"
            if prefix_len + suffix_len + 2 <= inner_w {
                let leader = inner_w - prefix_len - suffix_len;
                lines.push(Line::from(vec![
                    Span::raw(format!("  {label}  ")),
                    Span::styled(
                        "·".repeat(leader),
                        Style::default().fg(theme.dim),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        keys.clone(),
                        Style::default()
                            .fg(theme.match_hl)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            } else {
                // Tight modal: skip the leader, leave one-space gap.
                let used = 2 + label.len() + 1 + keys.len();
                let pad = inner_w.saturating_sub(used);
                lines.push(Line::from(vec![
                    Span::raw(format!("  {label} ")),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(
                        keys.clone(),
                        Style::default()
                            .fg(theme.match_hl)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }
    }
    lines
}

pub fn render(
    area: Rect,
    frame: &mut Frame,
    entries: &[HelpCategory],
    theme: &Theme,
    scroll: &mut usize,
    filter: &str,
) {
    use crate::ui::modal;
    frame.render_widget(Clear, area);
    let block = modal::modal_block("explorenova keys", theme);
    let inner = modal::pad_inner(block.inner(area));
    frame.render_widget(block, area);

    // Layout: [filter row][separator row][...body...][footer row]
    let header_h: u16 = 2;
    let footer_h: u16 = 1;
    let body_h = inner
        .height
        .saturating_sub(header_h + footer_h)
        .max(1) as usize;
    // Clamped: a fixed offset on a short terminal put the body and footer
    // below the modal's own border, over the panes behind it.
    let header_rect = modal::sub_rect(inner, 0, header_h);
    let body_rect = modal::sub_rect(inner, header_h, body_h as u16);
    let footer_rect = modal::sub_rect(inner, inner.height.saturating_sub(footer_h), footer_h);

    // Filter row + separator.
    let filter_line = if filter.is_empty() {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.match_hl)),
            Span::styled("type to filter…", Style::default().fg(theme.dim)),
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
    let sep = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(theme.dim),
    ));
    frame.render_widget(Paragraph::new(vec![filter_line, sep]), header_rect);

    // Body (filtered + scrolled).
    let all = build_lines(entries, theme, inner.width as usize, filter);
    // Clamp the CALLER's value, not a local copy. `End` sets help_scroll to
    // a huge sentinel and the old code clamped only for display, so the
    // stored number stayed near usize::MAX and `Up` spent the rest of the
    // session decrementing from about 4.6 quintillion.
    *scroll = (*scroll).min(all.len().saturating_sub(body_h));
    let scroll = *scroll;
    if all.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  no matches",
                Style::default().fg(theme.dim),
            )),
            body_rect,
        );
    } else {
        let max_scroll = all.len().saturating_sub(body_h);
        let s = scroll.min(max_scroll);
        let view: Vec<Line> = all.iter().skip(s).take(body_h).cloned().collect();
        frame.render_widget(Paragraph::new(view), body_rect);
    }

    // Footer.
    let max_scroll = all.len().saturating_sub(body_h);
    let pos_hint = if max_scroll > 0 {
        let s = scroll.min(max_scroll);
        format!(" {}/{} ·", s + 1, max_scroll + 1)
    } else {
        String::new()
    };
    let hint = format!(
        "{pos_hint} ↑↓ scroll · type to filter · ⌫ trim · esc close"
    );
    frame.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(theme.dim))),
        footer_rect,
    );
}

#[cfg(test)]
mod tests {

    #[test]
    fn end_then_up_scrolls_back_one_row() {
        // `End` sets help_scroll to a huge sentinel. render used to clamp a
        // local copy for display only, so the stored value stayed near
        // usize::MAX and every `Up` afterwards decremented from about 4.6
        // quintillion — the overlay looked frozen at the bottom forever.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let km = crate::keymap::Keymap::build(&Default::default());
        let entries = build_entries(&km);
        let theme = crate::ui::theme::Theme::default();
        let mut term = Terminal::new(TestBackend::new(78, 30)).unwrap();

        let mut scroll = usize::MAX / 2;
        term.draw(|f| render(f.area(), f, &entries, &theme, &mut scroll, ""))
            .unwrap();
        let at_end = scroll;
        assert!(at_end < 10_000, "End must land on a real row, got {at_end}");

        scroll = scroll.saturating_sub(1);
        term.draw(|f| render(f.area(), f, &entries, &theme, &mut scroll, ""))
            .unwrap();
        assert_eq!(scroll, at_end - 1, "Up must move exactly one row");
    }

    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn build_includes_at_least_one_row_per_category() {
        let km = Keymap::build(&BTreeMap::new());
        let cats = build_entries(&km);
        let names: Vec<_> = cats.iter().map(|c| c.name).collect();
        assert!(names.contains(&"Navigation"));
        assert!(names.contains(&"Files"));
        assert!(names.contains(&"Panes / Tabs"));
        assert!(names.contains(&"Shell / Misc"));
        for c in &cats {
            assert!(!c.rows.is_empty(), "{} empty", c.name);
        }
    }

    #[test]
    fn user_remap_reflected_in_keys() {
        let mut overrides = BTreeMap::new();
        overrides.insert("quit".to_string(), vec!["Q".to_string()]);
        let km = Keymap::build(&overrides);
        let cats = build_entries(&km);
        let quit_row = cats
            .iter()
            .flat_map(|c| &c.rows)
            .find(|(l, _)| l == "Quit")
            .unwrap();
        assert_eq!(quit_row.1, "Q");
    }

    #[test]
    fn help_action_bound_to_question_mark() {
        let km = Keymap::build(&BTreeMap::new());
        let keys = km.keys_for(Action::Help);
        assert_eq!(keys, vec!["?"]);
    }

    #[test]
    fn row_matches_substring() {
        // Note: caller lowercases tokens before calling row_matches;
        // these tests pass already-lowercased tokens.
        assert!(row_matches(&["quit"], "Shell / Misc", "Quit", "q"));
        assert!(row_matches(&["tab"], "Panes / Tabs", "New tab (split)", "t"));
        // Multi-token: every token must match the haystack.
        assert!(row_matches(&["ctrl", "page"], "nav", "Half page down", "ctrl-d"));
        assert!(!row_matches(&["xyz"], "nav", "Quit", "q"));
    }

    #[test]
    fn every_default_bound_action_is_documented() {
        let documented: std::collections::HashSet<crate::keymap::Action> =
            ACTION_META.iter().map(|(a, _, _)| *a).collect();
        for (action, _chords) in crate::keymap::Keymap::defaults() {
            // Cancel is bound to "esc" everywhere implicitly (dialog escape
            // hatch) and is already surfaced via the footer hint text, not
            // as a dedicated row.
            if action == crate::keymap::Action::Cancel {
                continue;
            }
            assert!(documented.contains(&action), "undocumented action: {action:?}");
        }
    }

    #[test]
    fn space_chord_renders_as_a_word() {
        let km = Keymap::build(&BTreeMap::new());
        let cats = build_entries(&km);
        let row = cats
            .iter()
            .flat_map(|c| &c.rows)
            .find(|(l, _)| l == "Toggle mark")
            .expect("Toggle mark documented");
        assert_eq!(row.1, "Space", "a bare space renders as a blank");
    }

    #[test]
    fn sort_menu_keys_are_documented() {
        let km = Keymap::build(&BTreeMap::new());
        let cats = build_entries(&km);
        let sort = cats
            .iter()
            .find(|c| c.name.starts_with("Sort menu"))
            .expect("sort section present");
        assert_eq!(sort.rows.len(), 6);
        // The keys must match what Keymap::resolve actually accepts.
        for (_, keys) in &sort.rows {
            for k in keys.split(" / ") {
                assert!(
                    km.resolve(crate::keymap::Mode::SortMenu, k).is_some(),
                    "help lists an unbound sort key: {k:?}"
                );
            }
        }
    }

    #[test]
    fn build_lines_filter_narrows_rows() {
        let km = Keymap::build(&BTreeMap::new());
        let entries = build_entries(&km);
        let theme = crate::ui::theme::Theme::default();
        let all = build_lines(&entries, &theme, 80, "");
        let only_quit = build_lines(&entries, &theme, 80, "quit");
        assert!(all.len() > only_quit.len());
        assert!(!only_quit.is_empty());
    }
}
