//! Filetype classification + Nerd Font glyphs for the tree.

use crate::tree::NodeKind;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Symlink,
    Exec,
    Media,
    Archive,
    File,
}

const MEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "mp4", "mkv", "mov",
    "mp3", "flac", "wav",
];
const ARCHIVE: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar"];

/// Extension, lowercased, following the same rule as `Path::extension` and
/// therefore as `sort::ext_key`: a leading dot belongs to the NAME, so
/// `.gitignore` has no extension. Splitting on the last dot alone made the
/// two disagree — a file called `.mp4` picked up a media icon here while
/// sorting with the extensionless entries there.
fn ext(name: &str) -> String {
    let stem = name.strip_prefix('.').unwrap_or(name);
    stem.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Classify a node for colour + icon (single source of truth).
pub fn classify(kind: NodeKind, exec: bool, name: &str) -> Kind {
    match kind {
        NodeKind::Dir => Kind::Dir,
        NodeKind::Symlink => Kind::Symlink,
        _ => {
            if exec {
                return Kind::Exec;
            }
            let e = ext(name);
            if MEDIA.contains(&e.as_str()) {
                Kind::Media
            } else if ARCHIVE.contains(&e.as_str()) {
                Kind::Archive
            } else {
                Kind::File
            }
        }
    }
}

// Catppuccin Mocha accents for per-filetype icon colours.
// shortcut: fixed Mocha colours ignore `[theme] preset`, so they are pastel on
// the light preset; route them through Theme if light themes need them.
const RED: Color = Color::Rgb(243, 139, 168);
const MAROON: Color = Color::Rgb(235, 160, 172);
const PEACH: Color = Color::Rgb(250, 179, 135);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const GREEN: Color = Color::Rgb(166, 227, 161);
const TEAL: Color = Color::Rgb(148, 226, 213);
const SKY: Color = Color::Rgb(137, 220, 235);
const SAPPHIRE: Color = Color::Rgb(116, 199, 236);
const BLUE: Color = Color::Rgb(137, 180, 250);
const LAVENDER: Color = Color::Rgb(180, 190, 254);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const PINK: Color = Color::Rgb(245, 194, 231);
const FLAMINGO: Color = Color::Rgb(242, 205, 205);
const SUBTEXT: Color = Color::Rgb(166, 173, 200);

/// Nerd Font glyph for a node, plus its own colour when the file type has
/// one. `None` means "use the row's type colour" (dirs, symlinks, unknown
/// files), so theme presets still drive those.
pub fn icon_for(kind: NodeKind, expanded: bool, name: &str) -> (&'static str, Option<Color>) {
    let lower = name.to_ascii_lowercase();
    match kind {
        NodeKind::Dir => {
            let special = match lower.as_str() {
                ".git" => Some(("\u{e5fb}", PEACH)),
                ".github" => Some(("\u{e5fd}", LAVENDER)),
                ".config" => Some(("\u{e5fc}", SUBTEXT)),
                "node_modules" => Some(("\u{e5fa}", RED)),
                _ => None,
            };
            match special {
                Some((g, c)) => (g, Some(c)),
                None if expanded => ("\u{f07c}", None),
                None => ("\u{f07b}", None),
            }
        }
        NodeKind::Symlink => ("\u{f481}", None),
        _ => {
            // Well-known file names win over their extension.
            let by_name = match lower.as_str() {
                "cargo.toml" | "cargo.lock" => Some(("\u{e7a8}", PEACH)),
                "package.json" | "package-lock.json" => Some(("\u{e71e}", RED)),
                "makefile" | "justfile" | "cmakelists.txt" => Some(("\u{e673}", SUBTEXT)),
                "dockerfile" | ".dockerignore" | "compose.yml" | "compose.yaml"
                | "docker-compose.yml" | "docker-compose.yaml" => Some(("\u{f308}", SAPPHIRE)),
                ".gitignore" | ".gitattributes" | ".gitmodules" => Some(("\u{f1d3}", PEACH)),
                "pkgbuild" => Some(("\u{f303}", BLUE)),
                "license" | "license.md" | "license.txt" | "copying" => Some(("\u{f02d}", YELLOW)),
                "readme" | "readme.md" | "readme.txt" => Some(("\u{f405}", LAVENDER)),
                ".env" | ".envrc" => Some(("\u{f084}", YELLOW)),
                _ => None,
            };
            if let Some((g, c)) = by_name {
                return (g, Some(c));
            }
            let (g, c) = match ext(name).as_str() {
                "rs" => ("\u{e7a8}", Some(PEACH)),
                "py" => ("\u{e606}", Some(YELLOW)),
                "js" | "mjs" | "cjs" => ("\u{e74e}", Some(YELLOW)),
                "ts" | "mts" | "cts" => ("\u{e628}", Some(BLUE)),
                "tsx" | "jsx" => ("\u{e7ba}", Some(SAPPHIRE)),
                "go" => ("\u{e627}", Some(SAPPHIRE)),
                "c" | "h" => ("\u{e61e}", Some(BLUE)),
                "cpp" | "cc" | "cxx" | "hpp" => ("\u{e61d}", Some(BLUE)),
                "rb" => ("\u{e21e}", Some(RED)),
                "java" | "jar" => ("\u{e738}", Some(PEACH)),
                "kt" | "kts" => ("\u{e634}", Some(MAUVE)),
                "swift" => ("\u{e755}", Some(PEACH)),
                "php" => ("\u{e73d}", Some(LAVENDER)),
                "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", Some(GREEN)),
                "lua" => ("\u{e620}", Some(BLUE)),
                "vim" => ("\u{e62b}", Some(GREEN)),
                "el" => ("\u{e632}", Some(MAUVE)),
                "ex" | "exs" => ("\u{e62d}", Some(MAUVE)),
                "hs" => ("\u{e777}", Some(MAUVE)),
                "ml" | "mli" => ("\u{e67a}", Some(PEACH)),
                "nim" => ("\u{e677}", Some(YELLOW)),
                "zig" => ("\u{e6a9}", Some(PEACH)),
                "dart" => ("\u{e798}", Some(SAPPHIRE)),
                "nix" => ("\u{f313}", Some(SAPPHIRE)),
                "md" | "markdown" => ("\u{f48a}", Some(LAVENDER)),
                "txt" | "text" | "log" => ("\u{f15c}", Some(SUBTEXT)),
                "json" | "jsonc" => ("\u{e60b}", Some(YELLOW)),
                "toml" => ("\u{e6b2}", Some(PEACH)),
                "yaml" | "yml" => ("\u{e615}", Some(MAUVE)),
                "conf" | "ini" | "cfg" | "service" => ("\u{e615}", Some(SUBTEXT)),
                "lock" => ("\u{f023}", Some(SUBTEXT)),
                "xml" => ("\u{f72d}", Some(PEACH)),
                "html" | "htm" => ("\u{f13b}", Some(PEACH)),
                "css" | "scss" | "sass" => ("\u{e749}", Some(BLUE)),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => {
                    ("\u{f1c5}", Some(MAUVE))
                }
                "pdf" => ("\u{f1c1}", Some(RED)),
                "doc" | "docx" | "odt" => ("\u{f1c2}", Some(BLUE)),
                "xls" | "xlsx" | "ods" | "csv" => ("\u{f1c3}", Some(GREEN)),
                "ppt" | "pptx" | "odp" => ("\u{f1c4}", Some(PEACH)),
                "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => {
                    ("\u{f410}", Some(MAROON))
                }
                "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" => ("\u{f001}", Some(PINK)),
                "mp4" | "mkv" | "mov" | "webm" | "avi" => ("\u{f03d}", Some(FLAMINGO)),
                "ttf" | "otf" | "woff" | "woff2" => ("\u{f031}", Some(SUBTEXT)),
                "sql" | "db" | "sqlite" => ("\u{f1c0}", Some(TEAL)),
                "diff" | "patch" => ("\u{f440}", Some(GREEN)),
                "desktop" => ("\u{f108}", Some(SKY)),
                "" => ("\u{f016}", None),
                _ => ("\u{f15b}", None),
            };
            (g, c)
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn dotfiles_have_no_extension_just_like_path_extension() {
        // `sort::ext_key` uses `Path::extension`, which treats the leading
        // dot as part of the name. These two must agree or a row sorts one
        // way and paints another.
        for name in [".gitignore", ".bashrc", ".mp4"] {
            assert_eq!(ext(name), "", "{name} must be extensionless");
            assert_eq!(std::path::Path::new(name).extension(), None);
        }
        assert_eq!(ext(".config.toml"), "toml");
        assert_eq!(ext("clip.MP4"), "mp4");
        assert_eq!(ext("archive.tar.gz"), "gz");
    }

    use super::*;

    #[test]
    fn classify_categories() {
        assert_eq!(classify(NodeKind::Dir, false, "src"), Kind::Dir);
        assert_eq!(classify(NodeKind::Symlink, false, "lnk"), Kind::Symlink);
        assert_eq!(classify(NodeKind::File, true, "run"), Kind::Exec);
        assert_eq!(classify(NodeKind::File, false, "a.PNG"), Kind::Media);
        assert_eq!(classify(NodeKind::File, false, "a.zip"), Kind::Archive);
        assert_eq!(classify(NodeKind::File, false, "a.rs"), Kind::File);
    }

    #[test]
    fn dir_icon_changes_with_expanded() {
        let c = icon_for(NodeKind::Dir, false, "x");
        let o = icon_for(NodeKind::Dir, true, "x");
        assert_ne!(c, o);
        assert!(!c.0.is_empty() && !o.0.is_empty());
    }

    #[test]
    fn symlink_has_glyph() {
        assert_eq!(icon_for(NodeKind::Symlink, false, "l").0, "\u{f481}");
    }

    #[test]
    fn special_names_beat_extension_and_carry_colour() {
        let cargo = icon_for(NodeKind::File, false, "Cargo.toml");
        assert_ne!(cargo, icon_for(NodeKind::File, false, "x.toml"));
        assert!(cargo.1.is_some());
        let git = icon_for(NodeKind::Dir, false, ".git");
        assert_ne!(git, icon_for(NodeKind::Dir, false, "src"));
        // Plain dirs and unknown files fall back to the theme's row colour.
        assert_eq!(icon_for(NodeKind::Dir, false, "src").1, None);
        assert_eq!(icon_for(NodeKind::File, false, "x.unknownext").1, None);
    }

    #[test]
    fn known_ext_differs_from_default_and_is_case_insensitive() {
        let def = icon_for(NodeKind::File, false, "x.unknownext");
        let rs = icon_for(NodeKind::File, false, "main.rs");
        assert_ne!(rs, def);
        assert_eq!(
            icon_for(NodeKind::File, false, "M.RS"),
            icon_for(NodeKind::File, false, "m.rs")
        );
    }
}