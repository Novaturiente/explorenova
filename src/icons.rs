//! Filetype classification + Nerd Font glyphs for the tree.

use crate::tree::NodeKind;

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

fn ext(name: &str) -> String {
    name.rsplit_once('.')
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

/// Nerd Font glyph for a node.
pub fn icon_for(kind: NodeKind, expanded: bool, name: &str) -> &'static str {
    match kind {
        NodeKind::Dir => {
            if expanded {
                "\u{f07c}"
            } else {
                "\u{f07b}"
            }
        }
        NodeKind::Symlink => "\u{f481}",
        _ => match ext(name).as_str() {
            "rs" => "\u{e7a8}",
            "py" => "\u{e606}",
            "js" | "mjs" | "cjs" => "\u{e74e}",
            "ts" => "\u{e628}",
            "tsx" | "jsx" => "\u{e7ba}",
            "go" => "\u{e627}",
            "c" | "h" => "\u{e61e}",
            "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
            "rb" => "\u{e21e}",
            "java" => "\u{e738}",
            "kt" | "kts" => "\u{e634}",
            "swift" => "\u{e755}",
            "php" => "\u{e73d}",
            "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
            "lua" => "\u{e620}",
            "vim" => "\u{e62b}",
            "el" => "\u{e632}",
            "ex" | "exs" => "\u{e62d}",
            "hs" => "\u{e777}",
            "ml" | "mli" => "\u{e67a}",
            "nim" => "\u{e677}",
            "zig" => "\u{e6a9}",
            "dart" => "\u{e798}",
            "md" | "markdown" => "\u{f48a}",
            "txt" | "text" => "\u{f15c}",
            "json" => "\u{e60b}",
            "toml" => "\u{e6b2}",
            "yaml" | "yml" => "\u{e615}",
            "lock" => "\u{f023}",
            "xml" => "\u{f72d}",
            "html" | "htm" => "\u{f13b}",
            "css" => "\u{e749}",
            "scss" | "sass" => "\u{e749}",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => "\u{f1c5}",
            "svg" => "\u{f1c5}",
            "pdf" => "\u{f1c1}",
            "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" => {
                "\u{f410}"
            }
            "mp3" | "flac" | "wav" => "\u{f001}",
            "mp4" | "mkv" | "mov" => "\u{f03d}",
            "csv" => "\u{f1c3}",
            "sql" => "\u{f1c0}",
            "db" | "sqlite" => "\u{f1c0}",
            "" => "\u{f016}",
            _ => "\u{f15b}",
        },
    }
}

#[cfg(test)]
mod tests {
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
        assert!(!c.is_empty() && !o.is_empty());
    }

    #[test]
    fn symlink_has_glyph() {
        assert_eq!(icon_for(NodeKind::Symlink, false, "l"), "\u{f481}");
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
