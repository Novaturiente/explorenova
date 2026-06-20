use ratatui::style::Color;
use std::collections::BTreeMap;
use std::str::FromStr;

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
        // Catppuccin Mocha. Roles mapped to distinct accents so colours that
        // can share a row (mark vs git_modified) and fg vs bg (file vs
        // selected_bg, file vs dim) never collide. Hex from the official
        // Catppuccin Mocha palette.
        Self {
            dir: Color::Rgb(137, 180, 250),         // Blue   #89b4fa
            file: Color::Rgb(205, 214, 244),        // Text   #cdd6f4
            selected_bg: Color::Rgb(69, 71, 90),    // Surface1 #45475a
            git_modified: Color::Rgb(249, 226, 175), // Yellow #f9e2af
            git_added: Color::Rgb(166, 227, 161),   // Green  #a6e3a1
            git_untracked: Color::Rgb(137, 220, 235), // Sky    #89dceb
            git_conflict: Color::Rgb(243, 139, 168), // Red    #f38ba8
            match_hl: Color::Rgb(245, 194, 231),    // Pink   #f5c2e7
            mark: Color::Rgb(250, 179, 135),        // Peach  #fab387
            executable: Color::Rgb(148, 226, 213),  // Teal   #94e2d5
            symlink: Color::Rgb(116, 199, 236),     // Sapphire #74c7ec
            media: Color::Rgb(203, 166, 247),       // Mauve  #cba6f7
            archive: Color::Rgb(235, 160, 172),     // Maroon #eba0ac
            header: Color::Rgb(180, 190, 254),      // Lavender #b4befe
            dim: Color::Rgb(108, 112, 134),         // Overlay0 #6c7086
        }
    }
}

impl Theme {
    /// ANSI-named palette: colors follow the user's terminal theme.
    /// ponytail: 15 semantic roles mapped onto 16 ANSI colors — a few
    /// near-collisions are unavoidable (header/selected_bg share Blue,
    /// but one is fg and one is bg). Opt-in via `[theme] preset = "terminal"`.
    pub fn terminal() -> Self {
        Self {
            dir: Color::Cyan,
            file: Color::White,
            selected_bg: Color::Blue,
            git_modified: Color::Yellow,
            git_added: Color::Green,
            git_untracked: Color::LightBlue,
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
            mark: Color::LightYellow,
            executable: Color::LightGreen,
            symlink: Color::LightCyan,
            media: Color::LightMagenta,
            archive: Color::LightRed,
            header: Color::Blue,
            dim: Color::DarkGray,
        }
    }

    /// Set the field named `name` to `color`. Returns false if `name` is
    /// not a known theme field.
    pub fn set_field(&mut self, name: &str, color: Color) -> bool {
        match name {
            "dir" => self.dir = color,
            "file" => self.file = color,
            "selected_bg" => self.selected_bg = color,
            "git_modified" => self.git_modified = color,
            "git_added" => self.git_added = color,
            "git_untracked" => self.git_untracked = color,
            "git_conflict" => self.git_conflict = color,
            "match_hl" => self.match_hl = color,
            "mark" => self.mark = color,
            "executable" => self.executable = color,
            "symlink" => self.symlink = color,
            "media" => self.media = color,
            "archive" => self.archive = color,
            "header" => self.header = color,
            "dim" => self.dim = color,
            _ => return false,
        }
        true
    }

    /// Build a Theme from the raw `[theme]` config table. Tolerant: a bad
    /// color value or unknown key produces a warning and is skipped, never
    /// discarding the rest. `preset` selects the base palette.
    pub fn from_config(raw: &BTreeMap<String, String>) -> (Theme, Vec<String>) {
        let mut warnings = Vec::new();
        let mut theme = match raw.get("preset").map(String::as_str) {
            None | Some("default") => Theme::default(),
            Some("terminal") => Theme::terminal(),
            Some(other) => {
                warnings.push(format!("theme.preset: unknown preset '{other}', using default"));
                Theme::default()
            }
        };
        for (key, val) in raw {
            if key == "preset" {
                continue;
            }
            let color = match Color::from_str(val) {
                Ok(Color::Reset) => {
                    warnings.push(format!("theme.{key}: 'reset' not allowed, keeping default"));
                    continue;
                }
                Ok(c) => c,
                Err(_) => {
                    warnings.push(format!("theme.{key}: invalid color '{val}', keeping default"));
                    continue;
                }
            };
            if !theme.set_field(key, color) {
                warnings.push(format!("theme: unknown key '{key}'"));
            }
        }
        (theme, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn empty_config_is_rgb_default() {
        let (t, w) = Theme::from_config(&BTreeMap::new());
        assert_eq!(t.dir, Theme::default().dir);
        assert!(w.is_empty());
    }

    #[test]
    fn terminal_preset_uses_ansi_names() {
        let (t, w) = Theme::from_config(&map(&[("preset", "terminal")]));
        assert_eq!(t.dir, Color::Cyan);
        assert!(w.is_empty());
    }

    #[test]
    fn per_field_override_applies_over_preset() {
        let (t, w) = Theme::from_config(&map(&[("dir", "#102030")]));
        assert_eq!(t.dir, Color::Rgb(16, 32, 48));
        // unspecified field keeps the RGB default
        assert_eq!(t.file, Theme::default().file);
        assert!(w.is_empty());
    }

    #[test]
    fn bad_color_warns_and_keeps_default() {
        let (t, w) = Theme::from_config(&map(&[("dir", "blu")]));
        assert_eq!(t.dir, Theme::default().dir);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("dir"));
    }

    #[test]
    fn unknown_key_warns() {
        let (_t, w) = Theme::from_config(&map(&[("dirr", "blue")]));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("dirr"));
    }

    #[test]
    fn reset_is_rejected() {
        let (t, w) = Theme::from_config(&map(&[("selected_bg", "reset")]));
        assert_eq!(t.selected_bg, Theme::default().selected_bg);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn unknown_preset_warns_and_defaults() {
        let (t, w) = Theme::from_config(&map(&[("preset", "dracula")]));
        assert_eq!(t.dir, Theme::default().dir);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("dracula"));
    }
}
