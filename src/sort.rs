use crate::tree::node::Node;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortMode {
    #[default]
    Name,
    Size,
    Mtime,
    Ext,
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

/// Natural order: digit runs compare numerically, so `file2` < `file10`.
/// Numerically-equal runs ("07" vs "7") fall through to the next segment;
/// the raw-name tiebreak in `cmp_name_keys` keeps the total order
/// deterministic — without it, `A` vs `a` order would follow arbitrary
/// `read_dir` order and could flip between reloads.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ac = a.chars().peekable();
    let mut bc = b.chars().peekable();
    loop {
        match (ac.peek().copied(), bc.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(&c) = ac.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    na.push(c);
                    ac.next();
                }
                let mut nb = String::new();
                while let Some(&c) = bc.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    nb.push(c);
                    bc.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.cmp(&y);
                if ord != Ordering::Equal {
                    return ord;
                }
                ac.next();
                bc.next();
            }
        }
    }
}

/// Precomputed per-entry sort key. `name`/`ext` are lowercased once here
/// instead of being recomputed (and re-allocated) on every comparison
/// `sort_by`'s O(n log n) comparisons would otherwise make — for name_key
/// and ext_key that was two extra String allocations per comparison.
struct Key {
    idx: usize,
    is_dir: bool,
    name: String,
    raw: std::ffi::OsString,
    ext: String,
    size: u64,
    mtime: std::time::SystemTime,
}

fn cmp_name_keys(a: &Key, b: &Key) -> Ordering {
    natural_cmp(&a.name, &b.name).then_with(|| a.raw.cmp(&b.raw))
}

fn cmp_keys(a: &Key, b: &Key, mode: SortMode) -> Ordering {
    match mode {
        SortMode::Name => cmp_name_keys(a, b),
        // Size and mtime lead with the biggest / newest (what you look for
        // when sorting by them); the name tiebreak stays A-Z.
        SortMode::Size => b.size.cmp(&a.size).then_with(|| cmp_name_keys(a, b)),
        SortMode::Mtime => b.mtime.cmp(&a.mtime).then_with(|| cmp_name_keys(a, b)),
        SortMode::Ext => a.ext.cmp(&b.ext).then_with(|| cmp_name_keys(a, b)),
    }
}

/// Reorder `indices` in place per `sort`. `indices` is a slice of indices
/// into `arena_nodes` (typically a parent node's `children` vector).
pub fn sort_children(indices: &mut [usize], arena_nodes: &[Node], sort: &SortState) {
    let mut keyed: Vec<Key> = indices
        .iter()
        .map(|&i| {
            let n = &arena_nodes[i];
            Key {
                idx: i,
                is_dir: n.is_dir(),
                name: name_key(n),
                raw: n.path.file_name().unwrap_or_default().to_owned(),
                ext: ext_key(n),
                size: n.size,
                mtime: n.mtime,
            }
        })
        .collect();
    keyed.sort_by(|a, b| {
        let primary = if sort.dir_first {
            b.is_dir.cmp(&a.is_dir) // dirs first (true > false)
        } else {
            Ordering::Equal
        };
        // Reverse only flips the sort field, not the dir/file grouping:
        // with dir_first on, reversing keeps dirs above files and reverses
        // order *within* each group (the intuitive behavior).
        let secondary = cmp_keys(a, b, sort.mode);
        let secondary = if sort.reverse { secondary.reverse() } else { secondary };
        primary.then(secondary)
    });
    for (slot, k) in indices.iter_mut().zip(keyed) {
        *slot = k.idx;
    }
}

pub fn render_menu(area: Rect, frame: &mut Frame, sort: &SortState, theme: &crate::ui::theme::Theme) {
    use crate::ui::modal;
    frame.render_widget(Clear, area);
    let block = modal::modal_block("Sort", theme);
    let inner = modal::pad_inner(block.inner(area));
    frame.render_widget(block, area);

    let key = |k: &str| {
        Span::styled(
            format!(" {k} "),
            Style::default()
                .fg(theme.match_hl)
                .add_modifier(Modifier::BOLD),
        )
    };
    let active_marker = |m: SortMode| -> Span<'static> {
        if sort.mode == m {
            Span::styled(
                "  ● active".to_string(),
                Style::default()
                    .fg(theme.git_added)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(String::new())
        }
    };
    let on_off = |b: bool| {
        if b {
            Span::styled(
                "on".to_string(),
                Style::default()
                    .fg(theme.git_added)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("off".to_string(), modal::dim_style(theme))
        }
    };
    let row = |k: &str, label: &str, m: SortMode| -> Line<'static> {
        Line::from(vec![
            key(k),
            Span::raw(format!(" {label:<10}")),
            active_marker(m),
        ])
    };
    let toggle_row = |k: &str, label: &str, val: Span<'static>| -> Line<'static> {
        Line::from(vec![
            key(k),
            Span::raw(format!(" {label:<10}")),
            val,
        ])
    };

    let lines = vec![
        Line::from(""),
        row("1·n", "Name", SortMode::Name),
        row("2·s", "Size", SortMode::Size),
        row("3·m", "Mtime", SortMode::Mtime),
        row("4·e", "Ext", SortMode::Ext),
        Line::from(""),
        toggle_row("r", "Reverse", on_off(sort.reverse)),
        toggle_row("d", "Dir first", on_off(sort.dir_first)),
        Line::from(""),
        Line::from(Span::styled(
            " esc to close".to_string(),
            modal::dim_style(theme),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
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
    fn reverse_with_dir_first_keeps_dirs_above_files() {
        let nodes = vec![
            mk("a.txt", NodeKind::File, 0, 0),
            mk("aaa", NodeKind::Dir, 0, 0),
            mk("b.txt", NodeKind::File, 0, 0),
            mk("zzz", NodeKind::Dir, 0, 0),
        ];
        let mut idx = vec![0, 1, 2, 3];
        let s = SortState { mode: SortMode::Name, reverse: true, dir_first: true };
        sort_children(&mut idx, &nodes, &s);
        // Dirs still first (reversed within group: zzz, aaa), then files
        // (reversed: b.txt, a.txt) — reverse must NOT interleave files first.
        assert_eq!(idx, vec![3, 1, 2, 0]);
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
    fn size_largest_first_with_name_tiebreak() {
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
        // 10s first, tie broken by name (a, c), then 5.
        assert_eq!(idx, vec![1, 0, 2]);
    }

    #[test]
    fn mtime_newest_first() {
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
        assert_eq!(idx, vec![2, 0, 1]); // c (200), a (100), b (50)
    }

    #[test]
    fn natural_sort_orders_digit_runs_numerically() {
        let nodes = vec![
            mk("file10.txt", NodeKind::File, 0, 0),
            mk("file2.txt", NodeKind::File, 0, 0),
            mk("file1.txt", NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState { mode: SortMode::Name, reverse: false, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        assert_eq!(idx, vec![2, 1, 0]); // file1, file2, file10
    }

    #[test]
    fn case_ties_are_deterministic() {
        let nodes = vec![
            mk("a.txt", NodeKind::File, 0, 0),
            mk("A.txt", NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1];
        let s = SortState { mode: SortMode::Name, reverse: false, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        let first = idx.clone();
        let mut idx2 = vec![1, 0]; // opposite starting order
        sort_children(&mut idx2, &nodes, &s);
        assert_eq!(first, idx2); // same order regardless of read_dir order
    }

    #[test]
    fn natural_cmp_edges() {
        use super::natural_cmp;
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("file2", "file10"), Less);
        assert_eq!(natural_cmp("file07", "file7"), Equal); // raw tiebreak decides
        assert_eq!(natural_cmp("a2b3", "a2b10"), Less);
        assert_eq!(natural_cmp("abc", "abd"), Less);
        assert_eq!(natural_cmp("", "a"), Less);
    }

    #[test]
    fn cached_key_sort_matches_reference_order() {
        // names covering: case, digits, extensionless, unicode. Expected
        // order captured from the pre-refactor comparator (recomputing
        // keys per comparison) — the cached-key version must match it
        // exactly; semantics are frozen, only the allocation pattern
        // changes.
        let names = ["File10", "file2", "a.TXT", "b.txt", "Makefile", "ä", "07", "7"];
        let nodes: Vec<Node> = names.iter().map(|n| mk(n, NodeKind::File, 0, 0)).collect();
        let mut idx: Vec<usize> = (0..nodes.len()).collect();
        let s = SortState::default();
        sort_children(&mut idx, &nodes, &s);
        let order: Vec<&str> = idx.iter().map(|&i| names[i]).collect();
        assert_eq!(
            order,
            vec!["07", "7", "a.TXT", "b.txt", "file2", "File10", "Makefile", "ä"]
        );
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
