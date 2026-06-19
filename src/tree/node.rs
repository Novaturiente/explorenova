use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Clean,
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

impl GitStatus {
    /// Severity for directory aggregation (higher = surfaced).
    pub fn rank(self) -> u8 {
        match self {
            GitStatus::Clean => 0,
            GitStatus::Ignored => 1,
            GitStatus::Untracked => 2,
            GitStatus::Added => 3,
            GitStatus::Renamed => 4,
            GitStatus::Modified => 5,
            GitStatus::Deleted => 6,
            GitStatus::Conflicted => 7,
        }
    }

    pub fn glyph(self) -> char {
        match self {
            GitStatus::Clean => ' ',
            GitStatus::Modified => 'M',
            GitStatus::Added => 'A',
            GitStatus::Deleted => 'D',
            GitStatus::Renamed => 'R',
            GitStatus::Untracked => '?',
            GitStatus::Ignored => '!',
            GitStatus::Conflicted => 'U',
        }
    }
}

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub kind: NodeKind,
    pub expanded: bool,
    pub children: Option<Vec<NodeId>>,
    pub git: GitStatus,
    pub depth: usize,
    pub parent: Option<NodeId>,
    pub exec: bool,
    /// Cached emptiness for directories. `Some(true)` = has no entries
    /// after the hidden-files filter; `Some(false)` = has at least one;
    /// `None` = not yet probed (e.g. files, or dirs we couldn't read).
    pub empty: Option<bool>,
    pub size: u64,
    pub mtime: SystemTime,
    /// Unix mode bits captured at load (0 on non-unix or unreadable).
    pub mode: u32,
    /// True when this is a symlink whose target is a directory — lets the
    /// tree treat it as traversable while `kind` stays `Symlink` for the
    /// icon/permissions display.
    pub link_to_dir: bool,
}

impl Node {
    pub fn new(path: PathBuf, kind: NodeKind, depth: usize, parent: Option<NodeId>) -> Self {
        Self {
            path,
            kind,
            expanded: false,
            children: None,
            git: GitStatus::Clean,
            depth,
            parent,
            exec: false,
            empty: None,
            size: 0,
            mtime: SystemTime::UNIX_EPOCH,
            mode: 0,
            link_to_dir: false,
        }
    }

    /// Treated as a directory for navigation: a real dir, or a symlink
    /// pointing at one (so the user can descend into linked folders).
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir) || self.link_to_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicted_outranks_modified() {
        assert!(GitStatus::Conflicted.rank() > GitStatus::Modified.rank());
    }

    #[test]
    fn clean_glyph_is_space() {
        assert_eq!(GitStatus::Clean.glyph(), ' ');
        assert_eq!(GitStatus::Modified.glyph(), 'M');
    }

    #[test]
    fn new_node_unexpanded_and_unloaded() {
        let n = Node::new(PathBuf::from("/a"), NodeKind::Dir, 0, None);
        assert!(!n.expanded);
        assert!(n.children.is_none());
        assert!(n.is_dir());
    }
}
