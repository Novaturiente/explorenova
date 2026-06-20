use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

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
    let name = n
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| n.path.display().to_string());

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
        _ => theme.dim,
    };

    let mut spans: Vec<Span> = Vec::new();
    // Track displayed width so we can right-align the git glyph.
    let mut used: usize = 0;
    if marked {
        spans.push(Span::styled("* ", Style::default().fg(theme.mark)));
        used += 2;
    }
    spans.push(Span::raw(indent.clone()));
    used += indent.chars().count();
    let marker_style = if selected {
        Style::default()
    } else {
        Style::default().fg(theme.dim)
    };
    spans.push(Span::styled(marker.to_string(), marker_style));
    used += marker.chars().count();
    if icons {
        let ic = crate::icons::icon_for(n.kind, n.expanded, &name);
        spans.push(Span::styled(
            format!("{ic} "),
            Style::default().fg(type_color),
        ));
        // Most Nerd Font glyphs occupy 1 cell; plus the trailing space.
        used += 2;
    }
    let name_chars = name.chars().count();
    let name_style = if cut {
        Style::default().fg(type_color).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(type_color)
    };
    spans.push(Span::styled(name.clone(), name_style));
    used += name_chars;

    // Right side: optional linemode cell + 2-cell git glyph, right-aligned.
    let lm = line_mode.cell(n);
    let lm_cells = if lm.is_empty() { 0 } else { lm.chars().count() + 1 };
    let total = width as usize;
    let git_cells = 2usize;
    if used + git_cells + lm_cells < total {
        let pad = total - used - git_cells - lm_cells;
        spans.push(Span::raw(" ".repeat(pad)));
    } else {
        spans.push(Span::raw(" "));
    }
    if !lm.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
