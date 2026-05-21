use super::node::{Node, NodeId, NodeKind};
use std::fs;
use std::io;
use std::path::Path;

/// Quick probe: does `dir` contain at least one entry that survives the
/// hidden-files filter? Returns true on read failure to err on the safe
/// side (we don't want to flag a permission-denied dir as "empty").
fn dir_is_empty(dir: &Path, show_hidden: bool) -> bool {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return false,
    };
    !rd.filter_map(Result::ok).any(|e| {
        if show_hidden {
            return true;
        }
        e.file_name()
            .to_str()
            .map(|n| !n.starts_with('.'))
            .unwrap_or(true)
    })
}

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

    fn classify(p: &Path) -> (NodeKind, bool) {
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => (NodeKind::Symlink, false),
            Ok(m) if m.is_dir() => (NodeKind::Dir, false),
            Ok(m) => {
                #[cfg(unix)]
                let ex = {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let ex = false;
                (NodeKind::File, ex)
            }
            Err(_) => (NodeKind::File, false),
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
        let show_hidden = self.show_hidden;
        let mut child_ids = Vec::with_capacity(entries.len());
        for p in entries {
            let (kind, exec) = Self::classify(&p);
            let mut node = Node::new(p.clone(), kind, depth, Some(id));
            node.exec = exec;
            if matches!(kind, NodeKind::Dir) {
                node.empty = Some(dir_is_empty(&p, show_hidden));
            }
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

    #[test]
    fn empty_dir_is_flagged() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("hollow")).unwrap();
        fs::create_dir(d.path().join("solid")).unwrap();
        fs::write(d.path().join("solid/x"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap();
        let hollow = kids
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("hollow"))
            .unwrap();
        let solid = kids
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("solid"))
            .unwrap();
        assert_eq!(hollow.empty, Some(true));
        assert_eq!(solid.empty, Some(false));
    }

    #[test]
    fn dir_with_only_hidden_is_empty_unless_shown() {
        let d = tempdir().unwrap();
        let sub = d.path().join("dotonly");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join(".hidden"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let n = a.nodes[a.root]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("dotonly"))
            .unwrap();
        assert_eq!(n.empty, Some(true), "hidden-only dir reads as empty when show_hidden=false");
    }

    #[cfg(unix)]
    #[test]
    fn exec_bit_detected() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let xp = d.path().join("run.sh");
        std::fs::write(&xp, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&xp, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let pp = d.path().join("plain.txt");
        std::fs::write(&pp, "x").unwrap();
        std::fs::set_permissions(&pp, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap();
        let run = kids
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("run.sh"))
            .unwrap();
        let plain = kids
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("plain.txt"))
            .unwrap();
        assert!(run.exec);
        assert!(!plain.exec);
    }
}
