# Preview & Refresh Responsiveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the intermittent UI freezes during scrolling and file actions by removing the synchronous, full-tree work from the main thread: incremental single-directory refresh, a path-aware/self-write-aware fs-watcher, off-thread preview generation, and per-frame cost cuts.

**Architecture:** The event loop in `src/main.rs` owns all worker channels and drains them each iteration (the existing pattern for git status and copy/move ops). This plan extends that pattern: the fs-watcher carries event *paths* so only changed directories are reloaded; preview generation moves to a worker thread that delivers results over a channel; and two per-frame costs (a re-`stat`, a full-list build) are removed. Each task is independently testable and committs on its own.

**Tech Stack:** Rust, ratatui/crossterm (TUI), notify 6 (fs events), git2 (status), std threads + `mpsc` channels.

## Global Constraints

- No new dependencies. Crate name in integration tests is `explorenova`.
- Vendored trees `src/explorenova/` and `src/cargo-home/` are NOT part of the build — never edit them; ignore them in greps.
- `cargo test` must stay green; `cargo clippy` must stay warning-free on the real source (the project treats clippy-clean as a requirement).
- `Arena.nodes` is an append-only `Vec<Node>`; `NodeId` is an index into it. Existing node ids must never be reused for a different path while still referenced — reloading a directory reuses a surviving child's existing id (preserving its subtree) or pushes a new node; removed entries are dropped from the parent's `children` list and their node slots are simply orphaned (unreferenced, invisible to `flatten`).
- Status messages stay lowercase terse phrases set via `app.set_status(...)`.
- Preserve current behavior exactly except for the performance characteristics: same tree contents, same cursor-restore semantics, same git annotations, same preview output.

---

### Task 1: Cache file mode on `Node` (stop re-`stat`-ing the cursor every frame)

**Why:** `status::format_perms` calls `std::fs::symlink_metadata` on the cursor's path on every frame (status.rs:84), even though `Arena::classify` already stat'd the file at load time. On a slow/network path this adds a blocking stat to every redraw.

**Files:**
- Modify: `src/tree/node.rs` (add `mode` field)
- Modify: `src/tree/arena.rs` (`classify` returns mode; `load_children` stores it)
- Modify: `src/status.rs` (`format_perms` reads `n.mode`)
- Test: `src/tree/arena.rs` inline tests

**Interfaces:**
- Produces: `Node.mode: u32` — the unix mode bits captured at load (0 on non-unix or unreadable).
- `Arena::classify(p) -> (NodeKind, bool, u64, SystemTime, u32)` (mode appended).

- [ ] **Step 1: Write a failing test that mode is populated on load**

In `src/tree/arena.rs` tests module:

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib mode_populated_on_load`
Expected: FAIL — `Node` has no `mode` field (won't compile).

- [ ] **Step 3: Add the field to `Node`**

In `src/tree/node.rs`, add to the struct (after `mtime`):

```rust
    pub mtime: SystemTime,
    /// Unix mode bits captured at load (0 on non-unix or unreadable).
    pub mode: u32,
```

In `Node::new`, add `mode: 0,` to the initializer.

- [ ] **Step 4: Capture mode in `classify` and store it in `load_children`**

In `src/tree/arena.rs`, change `classify` to return mode. Replace its body so each arm yields a 5-tuple; on unix capture `m.mode()`, else 0:

```rust
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
```

In `load_children`, update the destructuring and assignment:

```rust
            let (kind, exec, size, mtime, mode) = Self::classify(&p);
            let mut node = Node::new(p.clone(), kind, depth, Some(id));
            node.exec = exec;
            node.size = size;
            node.mtime = mtime;
            node.mode = mode;
```

- [ ] **Step 5: Read mode from the node in `format_perms`**

In `src/status.rs`, replace the unix `format_perms` body so it uses `n.mode` instead of re-stat'ing:

```rust
#[cfg(unix)]
fn format_perms(n: &crate::tree::node::Node) -> String {
    let mode = n.mode;
    let kind = match n.kind {
        crate::tree::node::NodeKind::Dir => 'd',
        crate::tree::node::NodeKind::Symlink => 'l',
        _ => '-',
    };
    let bits = |s: u32| {
        let r = if (mode >> s) & 0o4 != 0 { 'r' } else { '-' };
        let w = if (mode >> s) & 0o2 != 0 { 'w' } else { '-' };
        let x = if (mode >> s) & 0o1 != 0 { 'x' } else { '-' };
        format!("{r}{w}{x}")
    };
    format!("{kind}{}{}{}", bits(6), bits(3), bits(0))
}
```

(`NodeKind` is already public; add `use crate::tree::node::NodeKind;` at the top of status.rs if the path form is too long, or keep fully-qualified as shown. Remove the now-unused `MetadataExt` import.)

- [ ] **Step 6: Run tests + clippy**

Run: `cargo test --lib && cargo clippy 2>&1 | grep -vE 'cargo-home|explorenova/src' | grep 'warning:'`
Expected: tests PASS; no clippy warnings (no unused imports).

- [ ] **Step 7: Commit**

```bash
git add src/tree/node.rs src/tree/arena.rs src/status.rs
git commit -m "perf(status): cache file mode on Node, stop re-stat per frame"
```

---

### Task 2: Viewport-slice the tree render (stop building a row per node every frame)

**Why:** `render_pane_tree` builds a styled `ListItem` for *every* visible node each frame (panes_render.rs:79), then lets ratatui clip. On a large expanded tree that allocates/styles thousands of rows per keystroke. `viewport_around` (ui/mod.rs) already exists but is unused here.

**Files:**
- Modify: `src/ui/panes_render.rs` (`render_pane_tree` — slice to the visible window, offset the ListState)
- Test: `src/ui/mod.rs` inline tests (viewport offset helper)

**Interfaces:**
- Consumes: `crate::ui::viewport_around` (already `pub(crate)`).
- Produces: a new `pub(crate) fn viewport_start(len: usize, sel: usize, visible: usize) -> usize` in `src/ui/mod.rs` — the index of the first visible row, so the renderer can translate `selected` into a slice-relative index. `viewport_around` and `viewport_start` must agree (same `start` math).

- [ ] **Step 1: Write a failing test for `viewport_start`**

In `src/ui/mod.rs` tests module (find the existing `#[cfg(test)] mod tests`; if none, add one):

```rust
    #[test]
    fn viewport_start_centers_and_clamps() {
        // fits on screen → start 0
        assert_eq!(viewport_start(5, 3, 10), 0);
        // sel near top → start 0
        assert_eq!(viewport_start(100, 1, 10), 0);
        // sel in middle → centered (sel - visible/2)
        assert_eq!(viewport_start(100, 50, 10), 45);
        // sel near bottom → clamped so window ends at len
        assert_eq!(viewport_start(100, 99, 10), 90);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib viewport_start_centers_and_clamps`
Expected: FAIL — `viewport_start` not defined.

- [ ] **Step 3: Add `viewport_start`, refactor `viewport_around` to use it**

In `src/ui/mod.rs`, add next to `viewport_around`:

```rust
/// First visible row index for a window of `visible` rows centered on
/// `sel` within `len` items. Matches `viewport_around`'s slice start.
pub(crate) fn viewport_start(len: usize, sel: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        return 0;
    }
    let half = visible / 2;
    let max_start = len - visible;
    sel.saturating_sub(half).min(max_start)
}
```

Then change `viewport_around`'s body to reuse it (keeps them in lock-step):

```rust
pub(crate) fn viewport_around<T: Clone>(lines: &[T], sel: usize, visible: usize) -> Vec<T> {
    if visible == 0 || lines.is_empty() {
        return Vec::new();
    }
    if lines.len() <= visible {
        return lines.to_vec();
    }
    let start = viewport_start(lines.len(), sel, visible);
    lines[start..start + visible].to_vec()
}
```

- [ ] **Step 4: Slice the tree render to the window**

In `src/ui/panes_render.rs` `render_pane_tree`, the inner height is `area.height - 2` (border). Replace the `let items: Vec<ListItem> = visible_ref.iter().enumerate().map(...)` block so it only builds rows for the window, and offset the ListState. Concretely, compute the window first:

```rust
    let win_h = area.height.saturating_sub(2) as usize;
    let start = crate::ui::viewport_start(visible_ref.len(), selected_ref, win_h);
    let end = (start + win_h).min(visible_ref.len());
    let items: Vec<ListItem> = visible_ref[start..end]
        .iter()
        .enumerate()
        .map(|(off, &id)| {
            let path = &arena_ref.nodes[id].path;
            ListItem::new(crate::ui::panes::tree_row(
                arena_ref,
                id,
                start + off == selected_ref,
                theme,
                app.config.icons,
                marks_ref.contains(path),
                cut_paths.is_some_and(|ps| ps.iter().any(|p| p == path)),
                inner_w,
            ))
        })
        .collect();
```

Then change the `ListState` selection from the absolute index to the slice-relative one (find `st.select(Some(selected_ref));` near the end of the function):

```rust
    let mut st = ListState::default();
    st.select(Some(selected_ref - start));
    f.render_stateful_widget(list, area, &mut st);
```

(`cut_paths` is defined just above the map; keep it. The `idx == selected_ref` highlight is now `start + off == selected_ref`.)

- [ ] **Step 5: Build + run the suite (visual glue has no unit test; rely on compile + existing tests)**

Run: `cargo build 2>&1 | grep -E 'warning|error'; cargo test --lib`
Expected: empty build output; tests PASS.

- [ ] **Step 6: Manual smoke (note in report, not a blocker)**

Note in the report: launch on a large directory, hold `j`/`k` and `gg`/`G` — the cursor highlight tracks correctly and scrolling is smooth. (No automated assert for the render glue; the offset math is covered by Task 2's unit test.)

- [ ] **Step 7: Commit**

```bash
git add src/ui/mod.rs src/ui/panes_render.rs
git commit -m "perf(ui): build tree rows only for the visible viewport"
```

---

### Task 3: Defer `dir_is_empty` to a viewport-bounded probe (cut the extra read_dir per sub-dir)

**Why:** `load_children` calls `dir_is_empty` for *every* sub-directory (arena.rs:120), doing an extra `read_dir` per child dir on every load/rebuild. Across an expanded tree that multiplies syscalls. The result only feeds the `▢` empty-dir glyph, which is needed only for *visible* unexpanded dirs.

**Files:**
- Modify: `src/tree/arena.rs` (drop the eager probe; expose `dir_is_empty` + a `probe_empty` method)
- Modify: `src/main.rs` (probe the viewport's dir nodes before draw)
- Test: `src/tree/arena.rs` inline tests

**Interfaces:**
- Produces: `Arena::probe_empty(&mut self, ids: &[NodeId])` — for each id that is a dir with `empty == None`, set `empty = Some(dir_is_empty(path, show_hidden))`. Cheap and idempotent.
- `dir_is_empty` becomes `pub(crate)` (used from the same crate's loop helper if needed) — or keep private and only call via `probe_empty`. Prefer keeping it private; expose only `probe_empty`.

- [ ] **Step 1: Write failing tests — load no longer probes; probe_empty fills it**

In `src/tree/arena.rs` tests:

```rust
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib empty_not_probed_at_load probe_empty_fills_marker`
Expected: FAIL — load still sets `empty`; `probe_empty` not defined.

- [ ] **Step 3: Drop the eager probe and add `probe_empty`**

In `src/tree/arena.rs` `load_children`, remove these lines:

```rust
            if matches!(kind, NodeKind::Dir) {
                node.empty = Some(dir_is_empty(&p, show_hidden));
            }
```

(`node.empty` then stays `None` until probed.) The `show_hidden` local in `load_children` may now be unused — remove it if clippy flags it.

Add the method (inside `impl Arena`):

```rust
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
```

- [ ] **Step 4: Probe the viewport before draw in the loop**

In `src/main.rs`, just before `term.draw(...)` (after `app.refresh_disk_free();`), add a bounded probe of the visible window. Use the terminal height as a generous upper bound on visible rows:

```rust
        // Lazily probe empty-dir markers only for the on-screen window,
        // so the marker survives without an O(tree) read_dir storm.
        if let Ok(sz) = term.size() {
            let win = sz.height as usize;
            let start = app.selected.saturating_sub(win);
            let end = (app.selected + win).min(app.visible.len());
            let ids: Vec<usize> = app.visible[start..end].to_vec();
            app.arena.probe_empty(&ids);
        }
```

(Window of `2*height` around the cursor covers the tree pane regardless of layout; each dir is probed at most once then cached.)

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test --lib && cargo clippy 2>&1 | grep -vE 'cargo-home|explorenova/src' | grep 'warning:'`
Expected: tests PASS; no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/tree/arena.rs src/main.rs
git commit -m "perf(tree): probe empty-dir marker lazily for the viewport only"
```

---

### Task 4: Incremental single-directory refresh + path-aware, self-write-aware watcher

**Why (root causes #1, #2, #3):** `refresh_in_place` rebuilds the *entire* expanded tree on the main thread; the watcher discards which path changed (main.rs:217-219), forcing that full rebuild; and every file action triggers it twice (explicit + the app's own write echoing back through the watcher). This is the primary freeze.

**Files:**
- Modify: `src/tree/arena.rs` (add `reload_dir`)
- Modify: `src/dispatch.rs` (add `refresh_dirs`; route action refreshes through it)
- Modify: `src/main.rs` (carry event paths; collect changed dirs; suppress self-writes)
- Test: `src/tree/arena.rs` inline tests (reload_dir)

**Interfaces:**
- Produces: `Arena::reload_dir(&mut self, dir: &Path) -> bool` — re-reads `dir`'s children from disk, reusing the existing child node id for any surviving path (preserving its loaded subtree + `expanded` flag), pushing new nodes for new paths, dropping removed paths from the children list. Returns `true` if `dir` was found in the arena and reloaded, `false` otherwise (caller falls back to a full `refresh_in_place`). Re-sorts the new children list.
- Produces: `dispatch::refresh_dirs(app: &mut App, dirs: &[PathBuf], git_tx: &Sender<()>)` — reload each dir present in the arena, re-flatten once, restore the cursor by path, trigger git. Falls back to `refresh_in_place` for any dir not found (e.g. the changed dir is the root's parent).
- Produces: `App.self_write_until: Option<Instant>` and `App.self_write_dirs: HashSet<PathBuf>` (or a small struct) — set when the app performs a mutation, used by the loop to ignore the watcher echo for those dirs for ~400ms.

- [ ] **Step 1: Write failing tests for `reload_dir`**

In `src/tree/arena.rs` tests:

```rust
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib reload_dir`
Expected: FAIL — `reload_dir` not defined.

- [ ] **Step 3: Implement `reload_dir`**

In `src/tree/arena.rs` (`impl Arena`), add. It mirrors `load_children` but diffs against existing children:

```rust
    /// Find the node id whose path equals `dir`, if loaded into the arena.
    fn id_for_path(&self, dir: &Path) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.path == dir)
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
                // Survivor: refresh its own metadata, keep subtree/expanded.
                let (kind, exec, size, mtime, mode) = Self::classify(&p);
                let n = &mut self.nodes[cid];
                n.kind = kind;
                n.exec = exec;
                n.size = size;
                n.mtime = mtime;
                n.mode = mode;
                new_children.push(cid);
            } else {
                let (kind, exec, size, mtime, mode) = Self::classify(&p);
                let mut node = Node::new(p.clone(), kind, depth, Some(id));
                node.exec = exec;
                node.size = size;
                node.mtime = mtime;
                node.mode = mode;
                self.nodes.push(node);
                new_children.push(self.nodes.len() - 1);
            }
        }
        // Any ids left in `by_path` were removed on disk → orphaned.
        sort_children(&mut new_children, &self.nodes, &self.sort);
        self.nodes[id].children = Some(new_children);
        true
    }
```

(Add `use std::path::PathBuf;` to arena.rs imports if not present — it currently imports only `Path`.)

- [ ] **Step 4: Run reload_dir tests to verify they pass**

Run: `cargo test --lib reload_dir`
Expected: PASS (all three).

- [ ] **Step 5: Add `refresh_dirs` in dispatch.rs**

In `src/dispatch.rs`, add next to `refresh_in_place`:

```rust
/// Incrementally refresh only the directories whose contents changed.
/// Reloads each in place (preserving expansion/subtrees elsewhere),
/// re-flattens once, restores the cursor by path, and re-triggers git.
/// Falls back to a full `refresh_in_place` if any changed dir isn't in
/// the arena (e.g. a change above the current root).
pub fn refresh_dirs(app: &mut App, dirs: &[std::path::PathBuf], git_tx: &mpsc::Sender<()>) {
    let sel_path = app
        .visible
        .get(app.selected)
        .map(|&id| app.arena.nodes[id].path.clone());
    let mut all_known = true;
    for d in dirs {
        if !app.arena.reload_dir(d) {
            all_known = false;
            break;
        }
    }
    if !all_known {
        refresh_in_place(app, git_tx);
        return;
    }
    app.visible = app.arena.flatten();
    app.selected = 0;
    if let Some(mut p) = sel_path {
        loop {
            if app.navigate_to(&p) {
                break;
            }
            match p.parent() {
                Some(parent) if parent != p.as_path() => p = parent.to_path_buf(),
                _ => break,
            }
        }
    }
    if !app.visible.is_empty() && app.selected >= app.visible.len() {
        app.selected = app.visible.len() - 1;
    }
    let _ = git_tx.send(());
}
```

- [ ] **Step 6: Add self-write tracking to App**

In `src/app.rs`, add fields to the `App` struct (near `cursor_moved_at`):

```rust
    /// Directories the app just mutated, with an expiry. The fs-watcher
    /// echo for these is ignored until the deadline so an action doesn't
    /// trigger a second (redundant) refresh.
    pub self_write_dirs: std::collections::HashSet<std::path::PathBuf>,
    pub self_write_until: Option<std::time::Instant>,
```

Initialize in `App::new`: `self_write_dirs: std::collections::HashSet::new(), self_write_until: None,`.

Add a helper method on `impl App`:

```rust
    /// Mark `dirs` as just-written-by-us so the watcher echo is ignored
    /// for a short window.
    pub fn mark_self_write(&mut self, dirs: impl IntoIterator<Item = std::path::PathBuf>) {
        self.self_write_dirs.extend(dirs);
        self.self_write_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
    }
```

- [ ] **Step 7: Carry event paths in the watcher channel and collect changed dirs**

In `src/main.rs`, change the watcher channel to carry the changed *paths*. The notify closure currently sends `ev`; send its paths instead:

```rust
    let (fs_tx, fs_rx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(ev) = res {
            let ev: notify::Event = ev;
            if !ev.paths.is_empty() {
                let _ = fs_tx.send(ev.paths);
            }
        }
    })?;
```

Replace the drain+rebuild block (main.rs ~216-231) with one that collects the *parent directories* of changed paths, drops self-write echoes, and refreshes incrementally:

```rust
        let mut changed_dirs: HashSet<PathBuf> = HashSet::new();
        while let Ok(paths) = fs_rx.try_recv() {
            for p in paths {
                // The dir whose listing changed is the parent of the
                // changed entry (a file create/delete/modify), or the
                // path itself if it is a watched directory.
                let dir = if watched.contains(&p) {
                    p.clone()
                } else {
                    p.parent().map(Path::to_path_buf).unwrap_or(p)
                };
                changed_dirs.insert(dir);
            }
        }
        // Drop our own just-written dirs (their refresh already happened).
        if let Some(until) = app.self_write_until {
            if Instant::now() < until {
                for d in &app.self_write_dirs {
                    changed_dirs.remove(d);
                }
            } else {
                app.self_write_until = None;
                app.self_write_dirs.clear();
            }
        }
        if !changed_dirs.is_empty() && fs_dirty_since.is_none() {
            fs_dirty_since = Some(Instant::now());
        }
        if let Some(t0) = fs_dirty_since {
            if t0.elapsed() >= Duration::from_millis(150) {
                let dirs: Vec<PathBuf> = changed_dirs.iter().cloned().collect();
                if dirs.is_empty() {
                    // only self-writes seen; nothing external to refresh
                } else {
                    refresh_dirs(&mut app, &dirs, &git_tx);
                }
                fs_dirty_since = None;
            }
        }
```

**Accumulation across the debounce window:** the 150ms window can span several loop iterations, so the changed dirs must persist between them. Declare `let mut pending_dirs: HashSet<PathBuf> = HashSet::new();` *above* the loop (next to `let mut fs_dirty_since`). In the block above, insert into `pending_dirs` (not a per-iteration local), and when the debounce fires, drain it:

```rust
        while let Ok(paths) = fs_rx.try_recv() {
            for p in paths {
                let dir = if watched.contains(&p) {
                    p.clone()
                } else {
                    p.parent().map(Path::to_path_buf).unwrap_or(p)
                };
                pending_dirs.insert(dir);
            }
        }
        if let Some(until) = app.self_write_until {
            if Instant::now() < until {
                for d in &app.self_write_dirs {
                    pending_dirs.remove(d);
                }
            } else {
                app.self_write_until = None;
                app.self_write_dirs.clear();
            }
        }
        if !pending_dirs.is_empty() && fs_dirty_since.is_none() {
            fs_dirty_since = Some(Instant::now());
        }
        if let Some(t0) = fs_dirty_since {
            if t0.elapsed() >= Duration::from_millis(150) {
                let dirs: Vec<PathBuf> = pending_dirs.drain().collect();
                if !dirs.is_empty() {
                    refresh_dirs(&mut app, &dirs, &git_tx);
                }
                fs_dirty_since = None;
            }
        }
```

(This replaces the two illustrative snippets above — use this version verbatim.)

Import `refresh_dirs` at the top: `use explorenova::dispatch::{handle_key, refresh_in_place, refresh_dirs};`. Ensure `notify::Event` is namable (it is via `notify::Event`).

- [ ] **Step 8: Mark self-writes on the explicit action refreshes**

The explicit `refresh_in_place(app, git_tx)` calls after mutations (dispatch.rs:586 create, :593 rename, :974 trash, and the op-finish at main.rs:242) should mark the touched dirs so the watcher echo is dropped. The simplest correct touch: in each of those call sites, before/after the refresh, call `app.mark_self_write([cwd_or_parent])`. For `dispatch::paste`/trash/create/rename the relevant dir is the current arena root or the destination dir. Concretely, in `confirm_yes` (delete), `trash_targets`, `commit_create`, `commit_rename`, and the paste worker-finish, call:

```rust
    app.mark_self_write([app.arena.nodes[app.arena.root].path.clone()]);
```

immediately before their `refresh_in_place(...)` / at op-finish. (Marking the current root covers the common case where the action targets the visible directory; if the destination differs, mark that too — for paste, mark `dst_dir`.)

- [ ] **Step 9: Build + run the full suite**

Run: `cargo build 2>&1 | grep -E 'warning|error'; cargo test`
Expected: empty build output; all tests PASS (the existing dispatch/app tests still green; new reload_dir tests green).

- [ ] **Step 10: Manual smoke (note in report)**

Note: with a build dir or `~/Downloads` expanded and churning, scrolling no longer hitches every ~150ms; a single copy/move/delete refreshes once (not twice). Cursor stays put after a refresh.

- [ ] **Step 11: Commit**

```bash
git add src/tree/arena.rs src/dispatch.rs src/app.rs src/main.rs
git commit -m "perf(refresh): incremental per-dir reload + path-aware, self-write-aware watcher"
```

---

### Task 5: Generate previews off the main thread

**Why (root cause #4):** Once the settle gate fires, `PreviewCache::get_or_render` runs synchronously in `term.draw` — `bsdtar`, `pdftoppm`/`ffmpegthumbnailer`/`avifdec`, syntect highlighting, xlsx parse, and the directory `count_entries` `read_dir` all block the UI thread. Resting on a heavy file freezes the TUI for the full duration.

**Files:**
- Modify: `src/preview/mod.rs` (`PreviewCache` gains a worker + request/result channels + in-flight set)
- Modify: `src/ui/panes_render.rs` (`render_pane_preview` requests async, shows placeholder until ready)
- Modify: `src/main.rs` (drain completed previews each iteration so a ready preview redraws)
- Test: `src/preview/mod.rs` inline tests

**Interfaces:**
- Produces on `PreviewCache`:
  - `request(&mut self, path: &Path, max_bytes: u64) -> Option<PreviewKind>` — returns the cached `PreviewKind` if present; otherwise spawns/queues an off-thread `render`, marks the key in-flight, and returns `None`. Never blocks on I/O.
  - `drain(&mut self) -> bool` — move any completed worker results into the cache; returns `true` if at least one arrived (so the loop knows to redraw).
- The worker is a single background thread owning a `Receiver<(PathBuf, u64)>` of requests and a `Sender<((PathBuf, SystemTime), PreviewKind)>` of results. `PreviewCache` keeps the request `Sender`, the result `Receiver`, the `map`, and an `in_flight: HashSet<(PathBuf, SystemTime)>`.

- [ ] **Step 1: Write failing tests for async request/drain**

In `src/preview/mod.rs` tests:

```rust
    #[test]
    fn request_is_async_then_drains_into_cache() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        let mut c = PreviewCache::new();
        // First request: not cached → None, work dispatched off-thread.
        assert!(c.request(&f, 1024).is_none());
        // Wait for the worker, draining until the result lands.
        let mut got = false;
        for _ in 0..200 {
            if c.drain() { got = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "worker should deliver a result");
        // Now cached → Some, synchronously.
        assert!(matches!(c.request(&f, 1024), Some(PreviewKind::Text(_))));
    }
```

(Replace `PreviewCache::default()` usage in the existing `cache_returns_same_value_twice` test with the new sync path: it should now request, drain until ready, then request again — or keep a small synchronous helper. Update that test to use `request` + a drain loop like above.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib request_is_async_then_drains_into_cache`
Expected: FAIL — `PreviewCache::new`/`request`/`drain` not defined.

- [ ] **Step 3: Rebuild `PreviewCache` around a worker thread**

In `src/preview/mod.rs`, replace the `PreviewCache` struct + impl. Keep `render`, `PreviewKind`, and `contains` semantics:

```rust
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};

pub struct PreviewCache {
    map: HashMap<(PathBuf, SystemTime), PreviewKind>,
    in_flight: HashSet<(PathBuf, SystemTime)>,
    req_tx: Sender<(PathBuf, SystemTime, u64)>,
    res_rx: Receiver<((PathBuf, SystemTime), PreviewKind)>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewCache {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(PathBuf, SystemTime, u64)>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((path, mtime, max_bytes)) = req_rx.recv() {
                let kind = render(&path, max_bytes);
                let _ = res_tx.send(((path, mtime), kind));
            }
        });
        Self { map: HashMap::new(), in_flight: HashSet::new(), req_tx, res_rx }
    }

    fn key(path: &Path) -> (PathBuf, SystemTime) {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        (path.to_path_buf(), mtime)
    }

    /// Cached preview if present; otherwise dispatch the render off-thread
    /// and return None. Never blocks on preview I/O.
    pub fn request(&mut self, path: &Path, max_bytes: u64) -> Option<PreviewKind> {
        let key = Self::key(path);
        if let Some(v) = self.map.get(&key) {
            return Some(v.clone());
        }
        if self.in_flight.insert(key.clone()) {
            let _ = self.req_tx.send((key.0, key.1, max_bytes));
        }
        None
    }

    /// Move completed worker results into the cache. Returns true if any
    /// arrived (caller should redraw).
    pub fn drain(&mut self) -> bool {
        let mut got = false;
        while let Ok((key, kind)) = self.res_rx.try_recv() {
            self.in_flight.remove(&key);
            self.map.insert(key, kind);
            got = true;
        }
        got
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(&Self::key(path))
    }
}
```

(Note: `key` does one `metadata` stat — cheap, on the main thread, but it is a stat per request. Acceptable: it's one stat, not the heavy render. If a later profile shows it matters, the cursor's `Node.mtime` from Task 1 can be passed in instead.)

- [ ] **Step 4: Make the render path request instead of block**

In `src/ui/panes_render.rs` `render_pane_preview`, replace the `get_or_render` call and the match so an un-ready preview shows the placeholder:

```rust
    let Some(kind) = cache.request(&path, app.config.preview_max_bytes) else {
        f.render_widget(Paragraph::new("(loading…)"), inner);
        return;
    };
    match kind {
        PreviewKind::Image(p) => {
            let already_decoded = img_cache
                .key
                .as_ref()
                .map(|(k, _)| k == &p)
                .unwrap_or(false);
            if already_decoded || elapsed {
                render_image(f, inner, picker, img_cache, &p);
            } else {
                f.render_widget(Paragraph::new("(loading…)"), inner);
            }
        }
        PreviewKind::Text(t) => f.render_widget(Paragraph::new(t), inner),
        PreviewKind::Meta(t) => f.render_widget(Paragraph::new(t), inner),
    }
```

(The settle gate above this — `let debounce_ms = crate::preview::settle_ms(&path) ...; if !(elapsed || cached) { ... return; }` — stays: it avoids even *requesting* a render until the cursor settles. Keep `let cached = cache.contains(&path);` as the gate input.)

- [ ] **Step 5: Drain completed previews in the loop**

In `src/main.rs`, in the channel-draining section at the top of the loop (near the git/fs drains), add:

```rust
        // Pull in any preview that finished rendering off-thread; a fresh
        // one means the preview pane has new content to show.
        let preview_ready = preview_cache.drain();
```

Then ensure the loop redraws promptly when one arrives. The poll already wakes on the settle timer; add `preview_ready` to shorten the next poll so the just-arrived preview paints without waiting for a key. In the `poll_ms` computation, force a short tick when a preview just arrived:

```rust
            if app.op.is_some() { base.min(80) } else if preview_ready { 0 } else { base }
```

(A `0` poll returns immediately, the loop redraws with the now-cached preview, then settles back. Because `drain` already happened this iteration, the cached value renders on *this* frame; the `0` is belt-and-suspenders for the in-flight case where the result lands mid-frame.)

- [ ] **Step 6: Run tests + clippy**

Run: `cargo test --lib preview && cargo test && cargo clippy 2>&1 | grep -vE 'cargo-home|explorenova/src' | grep 'warning:'`
Expected: PASS; no warnings.

- [ ] **Step 7: Manual smoke (note in report)**

Note: rest on a large archive or PDF — the pane shows "(loading…)" and the rest of the UI (scrolling, keys) stays responsive while it renders, then the preview appears.

- [ ] **Step 8: Commit**

```bash
git add src/preview/mod.rs src/ui/panes_render.rs src/main.rs
git commit -m "perf(preview): generate previews on a worker thread, never block the UI"
```

---

### Task 6: Scope and coalesce the git status scan

**Why (root cause #5):** `scan_status` walks the entire repo working tree (`recurse_untracked_dirs(true)`, git.rs:24) from the *startup* root, re-fired on every action and every refresh. The scans run on a background thread but pile up (one full scan per trigger), and each result is applied as `O(all arena nodes)` on the main thread (main.rs:208-214).

**Files:**
- Modify: `src/main.rs` (git request channel carries the current root; coalesce duplicate triggers; the worker drains queued requests and scans once)
- Modify: `src/dispatch.rs` + call sites: send the *current arena root* through `git_tx` instead of `()`
- Test: `src/main.rs` is binary-only (hard to unit test); add a small pure coalescing helper in `src/git.rs` with a test, OR test the channel-drain coalescing via a `git`-module helper.

**Interfaces:**
- Change `git_tx`/`git_rx` from `Sender<()>`/`Receiver<()>` to `Sender<PathBuf>`/`Receiver<PathBuf>` carrying the root to scan. All `git_tx.send(())` call sites become `git_tx.send(<current arena root>)`.
- The git worker, on wake, drains all queued requests and scans only once for the most recent root (coalescing a burst into a single scan).

- [ ] **Step 1: Add a tested coalescing helper in git.rs**

In `src/git.rs`, add a tiny pure helper + test (the channel logic in main.rs will call it):

```rust
/// Given the first received root plus any others already queued, return
/// the last one (most recent) — a burst of triggers coalesces to one scan.
pub fn coalesce_latest(first: PathBuf, rest: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    rest.into_iter().last().unwrap_or(first)
}
```

```rust
    #[test]
    fn coalesce_takes_most_recent() {
        let r = coalesce_latest(
            PathBuf::from("/a"),
            vec![PathBuf::from("/b"), PathBuf::from("/c")],
        );
        assert_eq!(r, PathBuf::from("/c"));
    }

    #[test]
    fn coalesce_single_keeps_first() {
        let r = coalesce_latest(PathBuf::from("/a"), Vec::<PathBuf>::new());
        assert_eq!(r, PathBuf::from("/a"));
    }
```

(Add a `#[cfg(test)] mod tests` with `use super::*;` and `use std::path::PathBuf;` if git.rs has none.)

- [ ] **Step 2: Run the helper tests to verify they fail then pass**

Run: `cargo test --lib coalesce`
Expected: FAIL first (undefined), PASS after Step 1's code is in.

- [ ] **Step 3: Change the git channel to carry the root and coalesce in the worker**

**First, update the `refresh_*` signatures.** Tasks 4 and the existing `refresh_in_place` declared `git_tx: &mpsc::Sender<()>`. Change both `refresh_in_place` and `refresh_dirs` in `src/dispatch.rs` to take `git_tx: &mpsc::Sender<std::path::PathBuf>`, and change their internal `let _ = git_tx.send(())` to `let _ = git_tx.send(app.arena.nodes[app.arena.root].path.clone());`. Then update `src/main.rs`:

```rust
    let (git_tx, git_rx) = mpsc::channel::<PathBuf>();
    let (git_res_tx, git_res_rx) = mpsc::channel();
    {
        let start_root = start.clone();
        std::thread::spawn(move || {
            let send = |r| { let _ = git_res_tx.send(r); };
            send(scan_status(&start_root).unwrap_or_default());
            while let Ok(root) = git_rx.recv() {
                // Coalesce a burst of triggers into one scan of the latest root.
                let root = explorenova::git::coalesce_latest(root, git_rx.try_iter());
                send(scan_status(&root).unwrap_or_default());
            }
        });
    }
```

- [ ] **Step 4: Send the current arena root at every trigger**

Replace every `let _ = git_tx.send(());` (dispatch.rs:76, :150, :742, :861, :869, :877, and the new `refresh_dirs`) and `git_tx.send(())` in main.rs with the current root. Since `git_tx` is now `Sender<PathBuf>`, send:

```rust
    let _ = git_tx.send(app.arena.nodes[app.arena.root].path.clone());
```

In `refresh_in_place` and `refresh_dirs` (which take `app`), use `app.arena.nodes[app.arena.root].path.clone()`. At the main.rs op-finish and fs-refresh sites, `refresh_*` already sends internally — verify no double type mismatch. For the standalone sends in dispatch.rs (lines 742/861/869/877 — sort/reroot/tab actions), use the same current-root expression.

Note: `scan_status` already scopes via `Repository::discover(root)` + `repo.statuses` over the workdir, so passing the *current* root makes it discover the repo containing the visible directory rather than always the startup repo. The whole-working-tree walk remains (git2 limitation without pathspec), but it now tracks where the user actually is and no longer queues redundant scans.

- [ ] **Step 5: Build + run the full suite**

Run: `cargo build 2>&1 | grep -E 'warning|error'; cargo test`
Expected: empty build output; all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/git.rs src/main.rs src/dispatch.rs
git commit -m "perf(git): scope status to current root and coalesce burst triggers"
```

---

## Final verification

- [ ] `cargo test` — all green.
- [ ] `cargo clippy 2>&1 | grep -vE 'cargo-home|explorenova/src'` — no warnings on real source.
- [ ] Manual: with an actively-churning expanded directory, hold `j`/`k` — no periodic hitch; rest on a large archive/PDF — UI stays responsive while it loads; a single copy/move/delete refreshes once; the cursor stays put after refresh.

## Root-cause coverage map

- **#1 full-tree synchronous rebuild** → Task 4 (`reload_dir` + `refresh_dirs`).
- **#2 watcher discards changed path** → Task 4 (event paths carried through the channel).
- **#3 double rebuild per action** → Task 4 (self-write suppression window).
- **#4 preview blocks UI** → Task 5 (off-thread generation) + Tasks 1–3 (per-frame cost cuts: re-stat, full-list build, empty-probe storm).
- **#5 whole-repo git rescan, queued, O(nodes) apply** → Task 6 (current-root scope + burst coalescing).

## Out of scope / notes

- `reload_dir` orphans removed entries' node slots (append-only `Vec`); over a very long session with heavy churn this grows memory slowly. If it ever matters, add an occasional compacting full `refresh_in_place` (e.g. every Nth reload). Not implemented now — YAGNI until measured.
- git2 has no cheap pathspec-scoped status that avoids the full working-tree walk; Task 6 reduces *frequency and redundancy*, not the per-scan cost. A true subtree-scoped scan would need a different approach and is out of scope.
