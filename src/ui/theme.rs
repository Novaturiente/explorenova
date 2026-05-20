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
    pub executable: Color,
    pub symlink: Color,
    pub media: Color,
    pub archive: Color,
    pub header: Color,
    pub dim: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            dir: Color::Cyan,
            file: Color::Gray,
            selected_bg: Color::Rgb(45, 55, 72),
            git_modified: Color::Yellow,
            git_added: Color::Green,
            git_untracked: Color::Blue,
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
            executable: Color::Green,
            symlink: Color::Cyan,
            media: Color::Magenta,
            archive: Color::Red,
            header: Color::Rgb(130, 170, 255),
            dim: Color::DarkGray,
        }
    }
}
