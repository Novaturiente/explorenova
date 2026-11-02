use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Build a styled line for one visible tree row. `width` is the inner
/// width of the tree pane (i.e. `area.width - 2`); the git glyph is
/// padded out to that width so it lands at the right edge of the row.
#[allow(clippy::too_many_arguments)]
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
    marked: bool,
    cut: bool,
    width: u16,
    line_mode: crate::linemode::LineMode,
) -> Line<'a> {
    let n = &arena.nodes[id];
    let indent = "  ".repeat(n.depth);
    // Distinct glyph for known-empty dirs so the eye can skip them
    // without having to expand. `empty` is set during load_children;
    // unreadable / unprobed dirs fall through to the closed-arrow.
    let marker = if n.is_dir() {
        match (n.expanded, n.empty) {
            (true, _) => "▾ ",
            (false, Some(true)) => "▢ ",
            _ => "▸ ",
        }
    } else {
        "  "
    };
    // Sanitize BEFORE `.width()` is measured below so padding math lines
    // up with what's actually rendered — a raw control char would either
    // corrupt the row (a \r overwrites from col 0) or desync the padding
    // (an ESC sequence has zero display width but nonzero char count).
    let name = crate::ui::display_name(
        &n.path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| n.path.display().to_string()),
    );

    let kind = crate::icons::classify(n.kind, n.exec, &name);
    let mut type_color = match kind {
        crate::icons::Kind::Dir => theme.dir,
        crate::icons::Kind::Symlink => theme.symlink,
        crate::icons::Kind::Exec => theme.executable,
        crate::icons::Kind::Media => theme.media,
        crate::icons::Kind::Archive => theme.archive,
        crate::icons::Kind::File => theme.file,
    };
    if marked {
        type_color = theme.mark;
    }

    let g = n.git.glyph();
    let git_clean = matches!(n.git, crate::tree::GitStatus::Clean);
    let git_color = match n.git {
        crate::tree::GitStatus::Modified => theme.git_modified,
        crate::tree::GitStatus::Added => theme.git_added,
        crate::tree::GitStatus::Untracked => theme.git_untracked,
        crate::tree::GitStatus::Conflicted => theme.git_conflict,
        crate::tree::GitStatus::Deleted => theme.git_deleted,
        crate::tree::GitStatus::Renamed => theme.git_renamed,
        crate::tree::GitStatus::Ignored => theme.git_ignored,
        crate::tree::GitStatus::Clean => theme.dim,
    };

    let mut spans: Vec<Span> = Vec::new();
    // Track displayed width so we can right-align the git glyph.
    let mut used: usize = 0;
    // Fixed-width mark gutter. Pushing "* " only when marked shifted the
    // whole row two cells right of its unmarked siblings, breaking the
    // indent guides; reserve the two cells unconditionally instead.
    spans.push(Span::styled(
        if marked { "* " } else { "  " },
        Style::default().fg(theme.mark),
    ));
    used += 2;
    used += indent.chars().count();
    spans.push(Span::raw(indent));
    let marker_style = if selected {
        Style::default()
    } else {
        Style::default().fg(theme.dim)
    };
    spans.push(Span::styled(marker.to_string(), marker_style));
    used += marker.chars().count();
    if icons {
        let (ic, ic_color) = crate::icons::icon_for(n.kind, n.expanded, &name);
        // A marked row keeps the mark colour on its icon too.
        let ic_color = if marked { type_color } else { ic_color.unwrap_or(type_color) };
        spans.push(Span::styled(format!("{ic} "), Style::default().fg(ic_color)));
        // Most Nerd Font glyphs occupy 1 cell; plus the trailing space.
        used += 2;
    }
    let name_style = if cut {
        Style::default().fg(type_color).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(type_color)
    };

    // Right side: optional linemode cell + 2-cell git glyph, right-aligned.
    // The git column is reserved BEFORE the name is laid out — it is the
    // row's only status signal, so a name too long for the pane gets
    // truncated rather than pushing the glyph past the border, where the
    // pane's own frame clipped it away.
    let lm = line_mode.cell(n);
    let total = width as usize;
    let git_cells = 2usize;
    // One cell always separates the name from whatever follows it.
    let mut avail = total.saturating_sub(used + git_cells + 1);
    // Linemode is the droppable one: skip it when the name needs the room.
    let lm_cells = if lm.is_empty() || lm.width() + 1 >= avail {
        0
    } else {
        lm.width() + 1
    };
    avail -= lm_cells;
    let (shown, shown_cells) = truncate_cells(&name, avail);
    spans.push(Span::styled(shown, name_style));
    spans.push(Span::raw(" ".repeat(avail - shown_cells + 1)));
    if lm_cells > 0 {
        spans.push(Span::styled(format!("{lm} "), Style::default().fg(theme.dim)));
    }
    let git_style = if git_clean && !selected {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(git_color)
    };
    spans.push(Span::styled(format!("{g} "), git_style));
    Line::from(spans)
}

/// Truncate `s` to at most `max` display cells, marking the cut with `…`.
/// Returns the string plus its width so the caller can pad exactly —
/// `.len()` would be wrong for CJK/emoji, which occupy two cells each.
fn truncate_cells(s: &str, max: usize) -> (String, usize) {
    let w = s.width();
    if w <= max {
        return (s.to_string(), w);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    let budget = max - 1; // the ellipsis costs one cell
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > budget {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    (out, used + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn row_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn long_name_is_truncated_so_the_git_glyph_survives() {
        let d = tempdir().unwrap();
        let name = "a".repeat(200);
        std::fs::write(d.path().join(&name), "x").unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        a.nodes[child].git = crate::tree::GitStatus::Modified;
        let line = tree_row(&a, child, false, &Theme::default(), false, false, false, 30,
            crate::linemode::LineMode::None);
        let text = row_text(&line);
        assert_eq!(text.width(), 30, "row must fill exactly the pane width: {text:?}");
        assert!(text.ends_with("M "), "git glyph clipped off: {text:?}");
        assert!(text.contains('…'), "name should be ellipsised: {text:?}");
    }

    #[test]
    fn mark_does_not_shift_the_row() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("z.txt"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        let plain = row_text(&tree_row(&a, child, false, &Theme::default(), false, false,
            false, 40, crate::linemode::LineMode::None));
        let marked = row_text(&tree_row(&a, child, false, &Theme::default(), false, true,
            false, 40, crate::linemode::LineMode::None));
        assert_eq!(plain.find("z.txt"), marked.find("z.txt"),
            "marking must not move the name");
        assert!(marked.starts_with("* "));
    }

    #[test]
    fn truncate_cells_counts_wide_glyphs() {
        // 4 CJK chars = 8 cells; budget 5 keeps 2 chars (4 cells) + ellipsis.
        let (s, w) = truncate_cells("日本語版", 5);
        assert_eq!(w, 5);
        assert_eq!(s, "日本…");
        assert_eq!(truncate_cells("abc", 10), ("abc".to_string(), 3));
    }

    #[test]
    fn row_renders_name() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("z.txt"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        let line = tree_row(&a, child, true, &Theme::default(), true, false, false, 80, crate::linemode::LineMode::None);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("z.txt"));
    }

    #[test]
    fn cut_row_is_dimmed() {
        use ratatui::style::Modifier;
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("z.txt"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        let line = tree_row(&a, child, false, &Theme::default(), true, false, true, 80, crate::linemode::LineMode::None);
        // The name span carries the DIM modifier when cut.
        let name_span = line
            .spans
            .iter()
            .find(|s| s.content.contains("z.txt"))
            .expect("name span present");
        assert!(name_span.style.add_modifier.contains(Modifier::DIM));
    }
}
