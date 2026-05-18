use super::node::{Node, NodeId, NodeKind};
use std::fs;
use std::io;
use std::path::Path;

pub struct Arena {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub show_hidden: bool,
}

impl Arena {
    pub fn new(root_path: &Path) -> io::Result<Self> {
        let kind = NodeKind::Dir;
        let root = Node::new(root_path.to_path_buf(), kind, 0, None);
        let mut a = Arena { nodes: vec![root], root: 0, show_hidden: false };
        a.load_children(0)?;
        a.nodes[0].expanded = true;
        Ok(a)
    }

    fn classify(p: &Path) -> NodeKind {
        let meta = fs::symlink_metadata(p);
        match meta {
            Ok(m) if m.file_type().is_symlink() => NodeKind::Symlink,
            Ok(m) if m.is_dir() => NodeKind::Dir,
            _ => NodeKind::File,
        }
    }

    /// Populate `children` of `id` from disk. Idempotent.
    pub fn load_children(&mut self, id: NodeId) -> io::Result<()> {
        if self.nodes[id].children.is_some() {
            return Ok(());
        }
        let dir = self.nodes[id].path.clone();
        let depth = self.nodes[id].depth + 1;
        let mut entries: Vec<_> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                self.show_hidden
                    || !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with('.'))
                        .unwrap_or(false)
            })
            .collect();
        entries.sort_by(|a, b| {
            let ad = a.is_dir();
            let bd = b.is_dir();
            bd.cmp(&ad).then(a.file_name().cmp(&b.file_name()))
        });
        let mut child_ids = Vec::with_capacity(entries.len());
        for p in entries {
            let kind = Self::classify(&p);
            let node = Node::new(p, kind, depth, Some(id));
            self.nodes.push(node);
            child_ids.push(self.nodes.len() - 1);
        }
        self.nodes[id].children = Some(child_ids);
        Ok(())
    }

    pub fn expand(&mut self, id: NodeId) -> io::Result<()> {
        if self.nodes[id].is_dir() {
            self.load_children(id)?;
            self.nodes[id].expanded = true;
        }
        Ok(())
    }

    pub fn collapse(&mut self, id: NodeId) {
        self.nodes[id].expanded = false;
    }

    pub fn toggle(&mut self, id: NodeId) -> io::Result<()> {
        if self.nodes[id].expanded {
            self.collapse(id);
        } else {
            self.expand(id)?;
        }
        Ok(())
    }

    /// DFS over expanded nodes -> visible NodeId list (root first).
    pub fn flatten(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.visit(self.root, &mut out);
        out
    }

    fn visit(&self, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        let n = &self.nodes[id];
        if n.expanded {
            if let Some(ch) = &n.children {
                for &c in ch {
                    self.visit(c, out);
                }
            }
        }
    }

    /// Expand every ancestor of `target` path so it becomes visible. Returns its id.
    pub fn reveal(&mut self, target: &Path) -> io::Result<Option<NodeId>> {
        let root_path = self.nodes[self.root].path.clone();
        let rel = match target.strip_prefix(&root_path) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let mut cur = self.root;
        let mut acc = root_path;
        for comp in rel.components() {
            self.expand(cur)?;
            acc = acc.join(comp);
            let next = self.nodes[cur]
                .children
                .as_ref()
                .and_then(|ch| ch.iter().copied().find(|&c| self.nodes[c].path == acc));
            match next {
                Some(n) => cur = n,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn fixture() -> tempfile::TempDir {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/inner.txt"), "x").unwrap();
        fs::write(d.path().join("a.txt"), "x").unwrap();
        fs::write(d.path().join(".hidden"), "x").unwrap();
        d
    }

    #[test]
    fn root_loads_children_hidden_excluded() {
        let d = fixture();
        let a = Arena::new(d.path()).unwrap();
        let names: Vec<_> = a.nodes[a.root]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|&c| a.nodes[c].path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"sub".to_string()));
        assert!(names.contains(&"a.txt".to_string()));
        assert!(!names.contains(&".hidden".to_string()));
        // dirs sort before files
        assert_eq!(names[0], "sub");
    }

    #[test]
    fn flatten_grows_when_dir_expanded() {
        let d = fixture();
        let mut a = Arena::new(d.path()).unwrap();
        let before = a.flatten().len();
        let sub = a.nodes[a.root]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .copied()
            .find(|&c| a.nodes[c].is_dir())
            .unwrap();
        a.expand(sub).unwrap();
        assert!(a.flatten().len() > before);
    }

    #[test]
    fn toggle_collapses_and_reexpands() {
        let d = fixture();
        let mut a = Arena::new(d.path()).unwrap();
        a.toggle(a.root).unwrap();
        assert_eq!(a.flatten().len(), 1); // only root when collapsed
        a.toggle(a.root).unwrap();
        assert!(a.flatten().len() > 1);
    }

    #[test]
    fn reveal_expands_ancestors() {
        let d = fixture();
        let mut a = Arena::new(d.path()).unwrap();
        let target = d.path().join("sub/inner.txt");
        let id = a.reveal(&target).unwrap().unwrap();
        assert_eq!(a.nodes[id].path, target);
        assert!(a.flatten().contains(&id));
    }
}
