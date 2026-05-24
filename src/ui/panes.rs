use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Build a styled line for one visible tree row. `width` is the inner
/// width of the tree pane (i.e. `area.width - 2`); the git glyph is
/// padded out to that width so it lands at the right edge of the row.
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
    marked: bool,
    width: u16,
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
    spans.push(Span::styled(name.clone(), Style::default().fg(type_color)));
    used += name_chars;

    // Pad to right-align git glyph (1 cell + 1 trailing space = 2 cells).
    let total = width as usize;
    let git_cells = 2usize;
    if used + git_cells < total {
        let pad = total - used - git_cells;
        spans.push(Span::raw(" ".repeat(pad)));
    } else {
        spans.push(Span::raw(" "));
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
        let line = tree_row(&a, child, true, &Theme::default(), true, false, 80);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("z.txt"));
    }
}
