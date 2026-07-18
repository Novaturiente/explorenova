use super::node::{Node, NodeId, NodeKind};
use crate::sort::{sort_children, SortState};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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
    /// When `Some`, `flatten` shows only these node ids (matches + their
    /// ancestor dirs). Descends through ancestor dirs regardless of their
    /// `expanded` flag. `None` = normal expansion-driven flatten.
    pub filter: Option<std::collections::HashSet<NodeId>>,
    /// Nodes orphaned by `reload_dir` (removed from a children list but
    /// still occupying arena slots). Slots are never reused, so a watched
    /// churning dir grows the arena unboundedly — when this crosses
    /// `needs_compaction`'s threshold the caller does a full rebuild.
    pub orphaned: usize,
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
            filter: None,
            orphaned: 0,
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

    /// Returns (kind, exec, size, mtime, mode, link_to_dir). `link_to_dir`
    /// is true for a symlink whose target is a directory, so the tree can
    /// let the user descend into it while still showing it as a symlink.
    fn classify(p: &Path) -> (NodeKind, bool, u64, SystemTime, u32, bool) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => {
                let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                #[cfg(unix)]
                let mode = m.permissions().mode();
                #[cfg(not(unix))]
                let mode = 0;
                // Follow the link (metadata, not symlink_metadata) to learn
                // whether the target is a directory.
                let link_to_dir = fs::metadata(p).map(|t| t.is_dir()).unwrap_or(false);
                (NodeKind::Symlink, false, m.len(), mt, mode, link_to_dir)
            }
            Ok(m) if m.is_dir() => {
                let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                #[cfg(unix)]
                let mode = m.permissions().mode();
                #[cfg(not(unix))]
                let mode = 0;
                (NodeKind::Dir, false, 0, mt, mode, false)
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
                (NodeKind::File, ex, m.len(), mt, mode, false)
            }
            Err(_) => (NodeKind::File, false, 0, SystemTime::UNIX_EPOCH, 0, false),
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
            let (kind, exec, size, mtime, mode, link_to_dir) = Self::classify(&p);
            let mut node = Node::new(p.clone(), kind, depth, Some(id));
            node.exec = exec;
            node.size = size;
            node.mtime = mtime;
            node.link_to_dir = link_to_dir;
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

    /// DFS over expanded nodes -> visible NodeId list (root first). When a
    /// filter is active, shows only allowed ids and descends ancestor dirs
    /// regardless of their expanded flag.
    pub fn flatten(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        match &self.filter {
            Some(allowed) => self.visit_filtered(self.root, allowed, &mut out),
            None => self.visit(self.root, &mut out),
        }
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

    fn visit_filtered(
        &self,
        id: NodeId,
        allowed: &std::collections::HashSet<NodeId>,
        out: &mut Vec<NodeId>,
    ) {
        // Root is always shown; other nodes only if allowed.
        if id != self.root && !allowed.contains(&id) {
            return;
        }
        out.push(id);
        if let Some(ch) = &self.nodes[id].children {
            for &c in ch {
                self.visit_filtered(c, allowed, out);
            }
        }
    }

    /// Resolve `dir` to its node id, considering only nodes reachable
    /// from the root via `children`. This skips orphaned slots (left by
    /// `reload_dir` when an entry is removed) so a recreated dir of the
    /// same path never resolves to a dead node.
    fn id_for_path(&self, dir: &Path) -> Option<NodeId> {
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if self.nodes[id].path == dir {
                return Some(id);
            }
            if let Some(ch) = &self.nodes[id].children {
                stack.extend(ch.iter().copied());
            }
        }
        None
    }

    /// Re-read `dir`'s children from disk in place. Surviving paths keep
    /// their existing node id (and loaded subtree + expanded state); new
    /// paths get fresh nodes; removed paths are dropped from the children
    /// list (their slots orphan). Returns false if `dir` isn't in the
    /// arena (caller should fall back to a full rebuild). Idempotent.
    pub fn reload_dir(&mut self, dir: &Path) -> bool {
        let Some(id) = self.id_for_path(dir) else { return false; };
        // Only reload a dir we'd already loaded; an unloaded (collapsed,
        // never-expanded) dir has no children to refresh.
        if self.nodes[id].children.is_none() {
            return true;
        }
        let depth = self.nodes[id].depth + 1;
        let show_hidden = self.show_hidden;
        let entries: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    show_hidden
                        || !p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with('.'))
                            .unwrap_or(false)
                })
                .collect(),
            Err(_) => return true, // unreadable now (e.g. perms): leave as-is
        };
        // Index existing children by path so survivors keep their id.
        let existing: Vec<NodeId> =
            self.nodes[id].children.take().unwrap_or_default();
        let mut by_path: std::collections::HashMap<PathBuf, NodeId> =
            existing.iter().map(|&c| (self.nodes[c].path.clone(), c)).collect();
        let mut new_children: Vec<NodeId> = Vec::with_capacity(entries.len());
        for p in entries {
            if let Some(cid) = by_path.remove(&p) {
                // Survivor: refresh its own metadata, keep subtree/expanded
                // UNLESS the entry changed kind in place (dir swapped for a
                // file or vice versa) — the old subtree is meaningless then
                // and flatten() would render ghost rows for children that
                // no longer exist on disk.
                let (kind, exec, size, mtime, mode, link_to_dir) = Self::classify(&p);
                let n = &mut self.nodes[cid];
                let was_dir = n.is_dir();
                let is_dir_now = matches!(kind, NodeKind::Dir) || link_to_dir;
                let kind_changed = was_dir != is_dir_now;
                n.kind = kind;
                n.exec = exec;
                n.size = size;
                n.mtime = mtime;
                n.mode = mode;
                n.link_to_dir = link_to_dir;
                n.empty = None;
                if kind_changed {
                    n.children = None;
                    n.expanded = false;
                }
                new_children.push(cid);
            } else {
                let (kind, exec, size, mtime, mode, link_to_dir) = Self::classify(&p);
                let mut node = Node::new(p.clone(), kind, depth, Some(id));
                node.exec = exec;
                node.size = size;
                node.mtime = mtime;
                node.mode = mode;
                node.link_to_dir = link_to_dir;
                self.nodes.push(node);
                new_children.push(self.nodes.len() - 1);
            }
        }
        // Any ids left in `by_path` were removed on disk → orphaned,
        // along with their entire loaded subtrees.
        for (_, cid) in by_path {
            self.orphaned += self.subtree_count(cid);
        }
        sort_children(&mut new_children, &self.nodes, &self.sort);
        self.nodes[id].children = Some(new_children);
        true
    }

    fn subtree_count(&self, id: NodeId) -> usize {
        let mut n = 1;
        if let Some(ch) = &self.nodes[id].children {
            for &c in ch {
                n += self.subtree_count(c);
            }
        }
        n
    }

    /// True when enough dead slots accumulated that a full rebuild is
    /// worth the cost. Threshold is arbitrary but conservative: a few
    /// thousand orphans is ~hundreds of KB and slows every resort_all.
    pub fn needs_compaction(&self) -> bool {
        self.orphaned >= 4096
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
    fn symlink_to_dir_is_traversable_but_stays_symlink() {
        use std::os::unix::fs::symlink;
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("real")).unwrap();
        fs::write(d.path().join("real/inner.txt"), "x").unwrap();
        symlink(d.path().join("real"), d.path().join("link")).unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        let link = a.nodes[a.root].children.as_ref().unwrap().iter().copied()
            .find(|&c| a.nodes[c].path.ends_with("link")).unwrap();
        assert_eq!(a.nodes[link].kind, NodeKind::Symlink, "display stays a symlink");
        assert!(a.nodes[link].is_dir(), "but it is traversable");
        a.expand(link).unwrap();
        assert!(
            a.nodes[link].children.as_ref().unwrap().iter()
                .any(|&c| a.nodes[c].path.ends_with("inner.txt")),
            "expands into the link target's contents"
        );
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

    #[test]
    fn reload_dir_picks_up_new_and_removed_entries() {
        let d = tempdir().unwrap();
        fs::write(d.path().join("a.txt"), "x").unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        assert_eq!(a.nodes[a.root].children.as_ref().unwrap().len(), 1);

        // create a new file + remove the old one on disk
        fs::write(d.path().join("b.txt"), "y").unwrap();
        fs::remove_file(d.path().join("a.txt")).unwrap();

        assert!(a.reload_dir(d.path()));
        let names: Vec<String> = a.nodes[a.root].children.as_ref().unwrap()
            .iter().map(|&c| a.nodes[c].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"b.txt".to_string()), "new file picked up");
        assert!(!names.contains(&"a.txt".to_string()), "removed file dropped");
    }

    #[test]
    fn reload_dir_counts_orphans_including_subtrees() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/inner.txt"), "x").unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        let sub = a.nodes[a.root].children.as_ref().unwrap()
            .iter().copied().find(|&c| a.nodes[c].path.ends_with("sub")).unwrap();
        a.expand(sub).unwrap(); // load inner.txt so the subtree has 2 nodes
        assert_eq!(a.orphaned, 0);
        fs::remove_dir_all(d.path().join("sub")).unwrap();
        assert!(a.reload_dir(d.path()));
        assert_eq!(a.orphaned, 2, "dir + its loaded child both orphan");
        assert!(!a.needs_compaction(), "threshold far above 2");
    }

    #[test]
    fn reload_dir_preserves_expanded_subtree() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/inner.txt"), "x").unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        let sub = a.nodes[a.root].children.as_ref().unwrap()
            .iter().copied().find(|&c| a.nodes[c].path.ends_with("sub")).unwrap();
        a.expand(sub).unwrap();
        let sub_id_before = sub;
        assert!(a.nodes[sub_id_before].expanded);

        // add a sibling file in root, then reload root
        fs::write(d.path().join("z.txt"), "z").unwrap();
        assert!(a.reload_dir(d.path()));

        // "sub" keeps its id and stays expanded with its loaded child
        let sub_after = a.nodes[a.root].children.as_ref().unwrap()
            .iter().copied().find(|&c| a.nodes[c].path.ends_with("sub")).unwrap();
        assert_eq!(sub_after, sub_id_before, "surviving dir keeps its node id");
        assert!(a.nodes[sub_after].expanded, "expanded state preserved");
        assert!(a.nodes[sub_after].children.is_some(), "loaded subtree preserved");
    }

    #[test]
    fn reload_dir_returns_false_for_unknown_path() {
        let d = tempdir().unwrap();
        let a_root = Arena::new(d.path()).unwrap();
        let mut a = a_root;
        assert!(!a.reload_dir(Path::new("/nonexistent/elsewhere")));
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

    #[test]
    fn filtered_flatten_keeps_matches_and_ancestors() {
        let d = fixture(); // root: sub/ (inner.txt), a.txt, .hidden
        let mut a = Arena::new(d.path()).unwrap();
        // Build the allowed set: reveal sub/inner.txt and add it + ancestors.
        let target = d.path().join("sub/inner.txt");
        let id = a.reveal(&target).unwrap().unwrap();
        let mut allowed = std::collections::HashSet::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            allowed.insert(c);
            cur = a.nodes[c].parent;
        }
        a.filter = Some(allowed);
        let names: Vec<String> = a.flatten().iter()
            .map(|&i| a.nodes[i].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // root's own file_name + sub + inner.txt present; a.txt pruned.
        assert!(names.iter().any(|n| n == "sub"));
        assert!(names.iter().any(|n| n == "inner.txt"));
        assert!(!names.iter().any(|n| n == "a.txt"), "non-match pruned");
    }

    #[test]
    fn flatten_without_filter_unchanged() {
        let d = fixture();
        let a = Arena::new(d.path()).unwrap();
        assert!(a.filter.is_none());
        assert!(a.flatten().len() >= 2); // root + children
    }

    #[test]
    fn reload_dir_ignores_orphaned_same_path_node() {
        let d = tempdir().unwrap();
        let subp = d.path().join("sub");
        fs::create_dir(&subp).unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        // expand sub so it becomes a loaded (children=Some) node
        let sub = a.nodes[a.root].children.as_ref().unwrap().iter().copied()
            .find(|&c| a.nodes[c].path == subp).unwrap();
        a.expand(sub).unwrap();
        // delete sub on disk, reload root → old sub node orphaned
        std::fs::remove_dir_all(&subp).unwrap();
        assert!(a.reload_dir(d.path()));
        // recreate sub, reload root → fresh sub node at a higher index
        fs::create_dir(&subp).unwrap();
        assert!(a.reload_dir(d.path()));
        let live = a.nodes[a.root].children.as_ref().unwrap().iter().copied()
            .find(|&c| a.nodes[c].path == subp).unwrap();
        a.expand(live).unwrap(); // load the live node's children
        // id_for_path must resolve to the LIVE node, not the orphan
        assert_eq!(a.id_for_path(&subp), Some(live));
        // and reload_dir on it picks up a new file
        fs::write(subp.join("new.txt"), "1").unwrap();
        assert!(a.reload_dir(&subp));
        let has_new = a.nodes[live].children.as_ref().unwrap().iter()
            .any(|&c| a.nodes[c].path.ends_with("new.txt"));
        assert!(has_new, "live recreated dir must refresh, not the orphan");
    }

    #[test]
    fn reload_clears_children_when_dir_becomes_file() {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.txt"), "x").unwrap();
        let mut a = Arena::new(d.path()).unwrap();
        let sub = a.flatten().iter().copied()
            .find(|&id| a.nodes[id].path.ends_with("sub")).unwrap();
        a.expand(sub).unwrap();
        // Replace dir with same-named file.
        std::fs::remove_dir_all(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub"), "now a file").unwrap();
        assert!(a.reload_dir(d.path()));
        let sub = a.flatten().iter().copied()
            .find(|&id| a.nodes[id].path.ends_with("sub")).unwrap();
        assert!(!a.nodes[sub].is_dir());
        assert!(!a.nodes[sub].expanded, "file must not stay expanded");
        assert!(a.nodes[sub].children.is_none(), "stale children must be dropped");
        // No ghost rows.
        assert!(!a.flatten().iter().any(|&id| a.nodes[id].path.ends_with("inner.txt")));
    }
}
