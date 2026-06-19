use super::node::{Node, NodeId, NodeKind};
use crate::sort::{sort_children, SortState};
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

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
    pub sort: SortState,
}

impl Arena {
    pub fn new(root_path: &Path) -> io::Result<Self> {
        let kind = NodeKind::Dir;
        let root = Node::new(root_path.to_path_buf(), kind, 0, None);
        let mut a = Arena {
            nodes: vec![root],
            root: 0,
            show_hidden: false,
            sort: SortState::default(),
        };
        a.load_children(0)?;
        a.nodes[0].expanded = true;
        Ok(a)
    }

    /// Push a new sort order. Existing loaded children are re-sorted in
    /// place; future `load_children` uses the new order.
    pub fn set_sort(&mut self, sort: SortState) {
        self.sort = sort;
        self.resort_all();
    }

    /// Re-sort every already-loaded children list under `self.sort`.
    pub fn resort_all(&mut self) {
        let n = self.nodes.len();
        for id in 0..n {
            if let Some(mut ch) = self.nodes[id].children.take() {
                sort_children(&mut ch, &self.nodes, &self.sort);
                self.nodes[id].children = Some(ch);
            }
        }
    }

    fn classify(p: &Path) -> (NodeKind, bool, u64, SystemTime, u32) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => {
                let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                #[cfg(unix)]
                let mode = m.permissions().mode();
                #[cfg(not(unix))]
                let mode = 0;
                (NodeKind::Symlink, false, m.len(), mt, mode)
            }
            Ok(m) if m.is_dir() => {
                let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                #[cfg(unix)]
                let mode = m.permissions().mode();
                #[cfg(not(unix))]
                let mode = 0;
                (NodeKind::Dir, false, 0, mt, mode)
            }
            Ok(m) => {
                #[cfg(unix)]
                let ex = m.permissions().mode() & 0o111 != 0;
                #[cfg(not(unix))]
                let ex = false;
                #[cfg(unix)]
                let mode = m.permissions().mode();
                #[cfg(not(unix))]
                let mode = 0;
                let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                (NodeKind::File, ex, m.len(), mt, mode)
            }
            Err(_) => (NodeKind::File, false, 0, SystemTime::UNIX_EPOCH, 0),
        }
    }

    /// Populate `children` of `id` from disk. Idempotent.
    pub fn load_children(&mut self, id: NodeId) -> io::Result<()> {
        if self.nodes[id].children.is_some() {
            return Ok(());
        }
        let dir = self.nodes[id].path.clone();
        let depth = self.nodes[id].depth + 1;
        let entries: Vec<_> = fs::read_dir(&dir)?
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
        let mut child_ids = Vec::with_capacity(entries.len());
        for p in entries {
            let (kind, exec, size, mtime, mode) = Self::classify(&p);
            let mut node = Node::new(p.clone(), kind, depth, Some(id));
            node.exec = exec;
            node.size = size;
            node.mtime = mtime;
            node.mode = mode;
            self.nodes.push(node);
            child_ids.push(self.nodes.len() - 1);
        }
        sort_children(&mut child_ids, &self.nodes, &self.sort);
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

    /// Lazily fill the empty-dir marker for the given node ids. Only dirs
    /// with an unknown (`None`) marker are probed; idempotent and cheap.
    /// Callers pass the on-screen window so cost is O(viewport), not O(tree).
    pub fn probe_empty(&mut self, ids: &[NodeId]) {
        let show_hidden = self.show_hidden;
        for &id in ids {
            let n = &self.nodes[id];
            if n.is_dir() && n.empty.is_none() {
                let empty = dir_is_empty(&n.path, show_hidden);
                self.nodes[id].empty = Some(empty);
            }
        }
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
    fn size_and_mtime_populated_on_load() {
        let d = tempdir().unwrap();
        fs::write(d.path().join("hello.txt"), "12345").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap();
        let file = kids
            .iter()
            .map(|&i| &a.nodes[i])
            .find(|n| n.path.ends_with("hello.txt"))
            .unwrap();
        assert_eq!(file.size, 5);
        assert!(file.mtime > std::time::UNIX_EPOCH);
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
        let mut a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap().clone();
        a.probe_empty(&kids);
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
        let mut a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap().clone();
        a.probe_empty(&kids);
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

    #[test]
    #[cfg(unix)]
    fn mode_populated_on_load() {
        let d = tempdir().unwrap();
        let f = d.path().join("m.txt");
        fs::write(&f, "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let kids = a.nodes[a.root].children.as_ref().unwrap();
        let node = kids.iter().map(|&i| &a.nodes[i]).find(|n| n.path.ends_with("m.txt")).unwrap();
        assert_ne!(node.mode, 0, "mode bits should be captured at load");
    }

    #[test]
    fn empty_not_probed_at_load() {
        let d = fixture(); // has "sub" (non-empty) + files
        let a = Arena::new(d.path()).unwrap();
        let sub = a.nodes[a.root].children.as_ref().unwrap()
            .iter().copied().find(|&c| a.nodes[c].path.ends_with("sub")).unwrap();
        assert_eq!(a.nodes[sub].empty, None, "load must not eagerly probe emptiness");
    }

    #[test]
    fn probe_empty_fills_marker() {
        let d = fixture();
        let mut a = Arena::new(d.path()).unwrap();
        let sub = a.nodes[a.root].children.as_ref().unwrap()
            .iter().copied().find(|&c| a.nodes[c].path.ends_with("sub")).unwrap();
        a.probe_empty(&[sub]);
        assert_eq!(a.nodes[sub].empty, Some(false), "sub has inner.txt → not empty");
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
