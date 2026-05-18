use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Build a styled line for one visible tree row.
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
) -> Line<'a> {
    let n = &arena.nodes[id];
    let indent = "  ".repeat(n.depth);
    let marker = if n.is_dir() {
        if n.expanded { "▾ " } else { "▸ " }
    } else {
        "  "
    };
    let name = n
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| n.path.display().to_string());
    let g = n.git.glyph();
    let git_style = match n.git {
        crate::tree::GitStatus::Modified => Style::default().fg(theme.git_modified),
        crate::tree::GitStatus::Added => Style::default().fg(theme.git_added),
        crate::tree::GitStatus::Untracked => Style::default().fg(theme.git_untracked),
        crate::tree::GitStatus::Conflicted => Style::default().fg(theme.git_conflict),
        _ => Style::default(),
    };
    let name_style = if n.is_dir() {
        Style::default().fg(theme.dir)
    } else {
        Style::default().fg(theme.file)
    };
    let mut line = Line::from(vec![
        Span::raw(format!("{indent}{marker}")),
        Span::styled(format!("{g} "), git_style),
        Span::styled(name, name_style),
    ]);
    if selected {
        line = line.style(Style::default().bg(theme.selected_bg).add_modifier(Modifier::BOLD));
    }
    line
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
        let line = tree_row(&a, child, true, &Theme::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("z.txt"));
    }
}
