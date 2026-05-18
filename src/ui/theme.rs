use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub dir: Color,
    pub file: Color,
    pub selected_bg: Color,
    pub git_modified: Color,
    pub git_added: Color,
    pub git_untracked: Color,
    pub git_conflict: Color,
    pub match_hl: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            dir: Color::Cyan,
            file: Color::Gray,
            selected_bg: Color::DarkGray,
            git_modified: Color::Yellow,
            git_added: Color::Green,
            git_untracked: Color::Blue,
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
        }
    }
}
