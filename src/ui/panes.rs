use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Build a styled line for one visible tree row.
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
) -> Line<'a> {
    let n = &arena.nodes[id];
    let indent = "  ".repeat(n.depth);
    let marker = if n.is_dir() {
        if n.expanded {
            "▾ "
        } else {
            "▸ "
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
    let type_color = match kind {
        crate::icons::Kind::Dir => theme.dir,
        crate::icons::Kind::Symlink => theme.symlink,
        crate::icons::Kind::Exec => theme.executable,
        crate::icons::Kind::Media => theme.media,
        crate::icons::Kind::Archive => theme.archive,
        crate::icons::Kind::File => theme.file,
    };

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
    spans.push(Span::raw(indent));
    let marker_style = if selected {
        Style::default()
    } else {
        Style::default().fg(theme.dim)
    };
    spans.push(Span::styled(marker.to_string(), marker_style));
    if icons {
        let ic = crate::icons::icon_for(n.kind, n.expanded, &name);
        spans.push(Span::styled(
            format!("{ic} "),
            Style::default().fg(type_color),
        ));
    }
    let git_style = if git_clean && !selected {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(git_color)
    };
    spans.push(Span::styled(format!("{g} "), git_style));
    spans.push(Span::styled(name, Style::default().fg(type_color)));
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
        let line = tree_row(&a, child, true, &Theme::default(), true);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("z.txt"));
    }
}
