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
