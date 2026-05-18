use std::path::PathBuf;

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
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir)
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
