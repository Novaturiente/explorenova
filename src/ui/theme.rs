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
    pub mark: Color,
    pub executable: Color,
    pub symlink: Color,
    pub media: Color,
    pub archive: Color,
    pub header: Color,
    pub dim: Color,
}

impl Default for Theme {
    fn default() -> Self {
        // Palette tuned for contrast on a dark background. Saturated
        // primaries; mark and git_modified use distinct hues so they
        // don't visually collide on a row that is both marked and dirty.
        Self {
            dir: Color::Rgb(80, 200, 250),         // bright sky
            file: Color::Rgb(225, 225, 230),       // near-white
            selected_bg: Color::Rgb(60, 80, 130),  // higher contrast
            git_modified: Color::Rgb(255, 195, 60), // amber
            git_added: Color::Rgb(80, 220, 110),   // vivid green
            git_untracked: Color::Rgb(130, 175, 255), // sky blue
            git_conflict: Color::Rgb(255, 90, 90), // bright red
            match_hl: Color::Rgb(230, 130, 230),   // pink
            mark: Color::Rgb(255, 230, 100),       // saturated yellow
            executable: Color::Rgb(130, 240, 130), // mint green
            symlink: Color::Rgb(140, 240, 255),    // cyan-bright
            media: Color::Rgb(230, 130, 230),      // pink
            archive: Color::Rgb(255, 140, 80),     // orange
            header: Color::Rgb(140, 180, 255),
            dim: Color::Rgb(120, 120, 140),
        }
    }
}
