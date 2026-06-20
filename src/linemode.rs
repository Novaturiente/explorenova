//! Optional right-aligned metadata column in the tree (size / mtime /
//! permissions), cycled with the `L` key and persisted in state.

use crate::tree::node::{Node, NodeKind};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineMode {
    #[default]
    None,
    Size,
    Mtime,
    Perms,
}

impl LineMode {
    pub fn next(self) -> LineMode {
        match self {
            LineMode::None => LineMode::Size,
            LineMode::Size => LineMode::Mtime,
            LineMode::Mtime => LineMode::Perms,
            LineMode::Perms => LineMode::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineMode::None => "none",
            LineMode::Size => "size",
            LineMode::Mtime => "mtime",
            LineMode::Perms => "perms",
        }
    }

    /// The right-aligned cell text for `n`. Empty for `None` and for the
    /// size of a directory (we don't recurse to size dirs).
    pub fn cell(self, n: &Node) -> String {
        match self {
            LineMode::None => String::new(),
            LineMode::Size => {
                if n.is_dir() {
                    String::new()
                } else {
                    crate::status::human_bytes(n.size)
                }
            }
            LineMode::Mtime => rel_time(n.mtime),
            LineMode::Perms => perms(n.kind, n.mode),
        }
    }
}

/// Humanize "time since mtime" as a compact relative string. No chrono:
/// just bucket the elapsed duration. Future timestamps clamp to "0s".
fn rel_time(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 86_400 * 7 {
        format!("{}d", secs / 86_400)
    } else if secs < 86_400 * 365 {
        format!("{}w", secs / (86_400 * 7))
    } else {
        format!("{}y", secs / (86_400 * 365))
    }
}

/// Unix `rwxr-xr-x`-style string from mode bits. Leading type char:
/// `d` dir, `l` symlink, `-` file. mode==0 (non-unix/unreadable) → "?".
fn perms(kind: NodeKind, mode: u32) -> String {
    if mode == 0 {
        return "?".to_string();
    }
    let t = match kind {
        NodeKind::Dir => 'd',
        NodeKind::Symlink => 'l',
        NodeKind::File => '-',
    };
    let bit = |shift: u32, ch: char| if mode & (1 << shift) != 0 { ch } else { '-' };
    format!(
        "{t}{}{}{}{}{}{}{}{}{}",
        bit(8, 'r'), bit(7, 'w'), bit(6, 'x'),
        bit(5, 'r'), bit(4, 'w'), bit(3, 'x'),
        bit(2, 'r'), bit(1, 'w'), bit(0, 'x'),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn file(size: u64, mode: u32) -> Node {
        let mut n = Node::new(PathBuf::from("/x/f"), NodeKind::File, 0, None);
        n.size = size;
        n.mode = mode;
        n
    }

    #[test]
    fn cycle_wraps() {
        assert_eq!(LineMode::None.next(), LineMode::Size);
        assert_eq!(LineMode::Perms.next(), LineMode::None);
    }

    #[test]
    fn none_cell_is_empty() {
        assert_eq!(LineMode::None.cell(&file(10, 0o644)), "");
    }

    #[test]
    fn size_cell_human() {
        // human_bytes(1024) is "1.0K" / "1 KiB"-ish — just assert non-empty
        // and that a directory yields empty.
        assert!(!LineMode::Size.cell(&file(2048, 0)).is_empty());
        let mut d = Node::new(PathBuf::from("/x/d"), NodeKind::Dir, 0, None);
        d.size = 0;
        assert_eq!(LineMode::Size.cell(&d), "");
    }

    #[test]
    fn perms_formats_rwx() {
        assert_eq!(perms(NodeKind::File, 0o755), "-rwxr-xr-x");
        assert_eq!(perms(NodeKind::Dir, 0o750), "drwxr-x---");
        assert_eq!(perms(NodeKind::File, 0), "?");
    }

    #[test]
    fn rel_time_buckets() {
        let now = SystemTime::now();
        assert_eq!(rel_time(now), "0s");
        assert_eq!(rel_time(now - Duration::from_secs(120)), "2m");
        assert_eq!(rel_time(now - Duration::from_secs(3 * 3600)), "3h");
    }
}
