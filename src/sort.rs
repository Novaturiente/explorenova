use crate::tree::node::Node;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortMode {
    Name,
    Size,
    Mtime,
    Ext,
}

impl Default for SortMode {
    fn default() -> Self {
        SortMode::Name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SortState {
    pub mode: SortMode,
    pub reverse: bool,
    pub dir_first: bool,
}

impl Default for SortState {
    fn default() -> Self {
        SortState {
            mode: SortMode::Name,
            reverse: false,
            dir_first: true,
        }
    }
}

fn name_key(n: &Node) -> String {
    n.path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

fn ext_key(n: &Node) -> String {
    // "~" sorts after most lowercase chars => extensionless last.
    n.path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "~".into())
}

fn cmp_by(a: &Node, b: &Node, mode: SortMode) -> Ordering {
    match mode {
        SortMode::Name => name_key(a).cmp(&name_key(b)),
        SortMode::Size => a
            .size
            .cmp(&b.size)
            .then_with(|| name_key(a).cmp(&name_key(b))),
        SortMode::Mtime => a
            .mtime
            .cmp(&b.mtime)
            .then_with(|| name_key(a).cmp(&name_key(b))),
        SortMode::Ext => ext_key(a)
            .cmp(&ext_key(b))
            .then_with(|| name_key(a).cmp(&name_key(b))),
    }
}

/// Reorder `indices` in place per `sort`. `indices` is a slice of indices
/// into `arena_nodes` (typically a parent node's `children` vector).
pub fn sort_children(indices: &mut [usize], arena_nodes: &[Node], sort: &SortState) {
    indices.sort_by(|&ai, &bi| {
        let a = &arena_nodes[ai];
        let b = &arena_nodes[bi];
        let primary = if sort.dir_first {
            b.is_dir().cmp(&a.is_dir()) // dirs first (true > false)
        } else {
            Ordering::Equal
        };
        let secondary = cmp_by(a, b, sort.mode);
        let combined = primary.then(secondary);
        if sort.reverse {
            combined.reverse()
        } else {
            combined
        }
    });
}

pub fn render_menu(area: Rect, frame: &mut Frame, sort: &SortState) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Sort ")
        .borders(Borders::ALL);
    let mark = |m: SortMode| {
        if sort.mode == m {
            " (active)"
        } else {
            ""
        }
    };
    let on_off = |b: bool| if b { "on" } else { "off" };
    let lines = vec![
        Line::from(""),
        Line::from(format!("  1  Name    [n]{}", mark(SortMode::Name))),
        Line::from(format!("  2  Size    [s]{}", mark(SortMode::Size))),
        Line::from(format!("  3  Mtime   [m]{}", mark(SortMode::Mtime))),
        Line::from(format!("  4  Ext     [e]{}", mark(SortMode::Ext))),
        Line::from(""),
        Line::from(vec![
            Span::raw("  r  Reverse:   "),
            Span::styled(
                on_off(sort.reverse).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  d  Dir first: "),
            Span::styled(
                on_off(sort.dir_first).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from("  esc  close"),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Center a Rect of given (w, h) inside `area`. Public so help.rs reuses it.
pub fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::node::{Node, NodeKind};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn mk(name: &str, kind: NodeKind, size: u64, mtime_secs: u64) -> Node {
        let mut n = Node::new(PathBuf::from(name), kind, 0, None);
        n.size = size;
        n.mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs);
        n
    }

    #[test]
    fn name_asc_dir_first() {
        let nodes = vec![
            mk("b.txt", NodeKind::File, 0, 0),
            mk("aaa", NodeKind::Dir, 0, 0),
            mk("a.txt", NodeKind::File, 0, 0),
            mk("zzz", NodeKind::Dir, 0, 0),
        ];
        let mut idx = vec![0, 1, 2, 3];
        let s = SortState::default();
        sort_children(&mut idx, &nodes, &s);
        // Dirs first (aaa, zzz), then files (a.txt, b.txt).
        assert_eq!(idx, vec![1, 3, 2, 0]);
    }

    #[test]
    fn name_desc_no_dir_first() {
        let nodes = vec![
            mk("a", NodeKind::File, 0, 0),
            mk("b", NodeKind::Dir, 0, 0),
            mk("c", NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState {
            mode: SortMode::Name,
            reverse: true,
            dir_first: false,
        };
        sort_children(&mut idx, &nodes, &s);
        assert_eq!(idx, vec![2, 1, 0]);
    }

    #[test]
    fn size_ascending_with_name_tiebreak() {
        let nodes = vec![
            mk("c", NodeKind::File, 10, 0),
            mk("a", NodeKind::File, 10, 0),
            mk("b", NodeKind::File, 5, 0),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState {
            mode: SortMode::Size,
            reverse: false,
            dir_first: false,
        };
        sort_children(&mut idx, &nodes, &s);
        // 5 first, then 10s tied broken by name (a, c).
        assert_eq!(idx, vec![2, 1, 0]);
    }

    #[test]
    fn mtime_ascending() {
        let nodes = vec![
            mk("a", NodeKind::File, 0, 100),
            mk("b", NodeKind::File, 0, 50),
            mk("c", NodeKind::File, 0, 200),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState {
            mode: SortMode::Mtime,
            reverse: false,
            dir_first: false,
        };
        sort_children(&mut idx, &nodes, &s);
        assert_eq!(idx, vec![1, 0, 2]);
    }

    #[test]
    fn ext_groups_with_dotless_last() {
        let nodes = vec![
            mk("a.rs", NodeKind::File, 0, 0),
            mk("readme", NodeKind::File, 0, 0),
            mk("b.rs", NodeKind::File, 0, 0),
            mk("c.md", NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1, 2, 3];
        let s = SortState {
            mode: SortMode::Ext,
            reverse: false,
            dir_first: false,
        };
        sort_children(&mut idx, &nodes, &s);
        // .md first, then .rs (a then b), then no-ext (readme).
        assert_eq!(idx, vec![3, 0, 2, 1]);
    }
}
