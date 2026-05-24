# Phase A UX Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `?` help overlay, 3-segment status bar (cursor/selection/disk), and persisted sort controls (name/size/mtime/ext, reverse, dir-first) to explorenova.

**Architecture:** Three feature tracks land in 6 tasks. Tasks 1-3 build sort end-to-end (Node fields → sort module → menu UI). Task 4 lands help overlay. Task 5 lands status bar. Task 6 is README + smoke.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, libc (statvfs), existing toml/serde for persistence.

**Spec:** `docs/superpowers/specs/2026-05-24-phase-a-ux-polish-design.md`.

**Branch:** `phase-a-ux-polish` (already created off master).

---

## Conventions

- **No `cargo fmt` repo-wide.** Project style is ~70-col line wrap. Format only files you edit and only if necessary; do NOT run `cargo fmt` (without `--check`) on the whole repo.
- **TDD:** Write the failing test, run it, implement minimal code, run again, commit.
- **Commits:** One per logical step inside a task (see Step 5/Step 9 etc). Use `feat:` / `test:` / `chore:` prefixes. No `Co-Authored-By` line (per project history pattern).
- **Tests:** `cargo test --lib --quiet` must pass after each commit. Existing baseline is 119 lib tests.

---

## Task 1: Node size/mtime fields + sort module + state persistence

**Files:**
- Modify: `src/tree/node.rs` (add `size: u64`, `mtime: SystemTime` to `Node`)
- Modify: `src/tree/arena.rs` (`classify` returns size+mtime; `load_children` populates)
- Create: `src/sort.rs` (`SortMode`, `SortState`, `sort_children`)
- Modify: `src/lib.rs` (add `pub mod sort;`)
- Modify: `src/state.rs` (add `sort: SortStatePersist` field)

- [ ] **Step 1: Add size/mtime to Node**

Edit `src/tree/node.rs`:

```rust
use std::path::PathBuf;
use std::time::SystemTime;

// ... existing enums ...

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
    pub empty: Option<bool>,
    pub size: u64,
    pub mtime: SystemTime,
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
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir)
    }
}
```

- [ ] **Step 2: Update arena `classify` to return size+mtime; populate in `load_children`**

Edit `src/tree/arena.rs`:

```rust
fn classify(p: &Path) -> (NodeKind, bool, u64, SystemTime) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => {
            let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            (NodeKind::Symlink, false, m.len(), mt)
        }
        Ok(m) if m.is_dir() => {
            let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            (NodeKind::Dir, false, 0, mt)
        }
        Ok(m) => {
            #[cfg(unix)]
            let ex = {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let ex = false;
            let mt = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            (NodeKind::File, ex, m.len(), mt)
        }
        Err(_) => (NodeKind::File, false, 0, SystemTime::UNIX_EPOCH),
    }
}
```

In `load_children`, after `let (kind, exec) = Self::classify(&p);` becomes
`let (kind, exec, size, mtime) = Self::classify(&p);` — set `node.size = size; node.mtime = mtime;`.

Add `use std::time::SystemTime;` to top of file.

- [ ] **Step 3: Run existing tree tests; confirm green**

```
cargo test --lib --quiet tree
```

Expected: 7 passed (existing tree tests unchanged).

- [ ] **Step 4: Add `size_and_mtime_populated` test in arena.rs**

Append to `src/tree/arena.rs` test module:

```rust
#[test]
fn size_and_mtime_populated_on_load() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("hello.txt"), "12345").unwrap();
    let a = Arena::new(d.path()).unwrap();
    let kids = a.nodes[a.root].children.as_ref().unwrap();
    let file = kids.iter().map(|&i| &a.nodes[i])
        .find(|n| n.path.ends_with("hello.txt")).unwrap();
    assert_eq!(file.size, 5);
    assert!(file.mtime > std::time::UNIX_EPOCH);
}
```

Run: `cargo test --lib --quiet tree::arena::tests::size_and_mtime_populated_on_load` → PASS.

- [ ] **Step 5: Commit foundation**

```
git add src/tree/node.rs src/tree/arena.rs
git commit -m "feat(tree): Node.size + Node.mtime populated during load_children"
```

- [ ] **Step 6: Create `src/sort.rs` with SortMode/SortState + sort_children**

Create `src/sort.rs`:

```rust
use crate::tree::node::Node;
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
    fn default() -> Self { SortMode::Name }
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
        SortState { mode: SortMode::Name, reverse: false, dir_first: true }
    }
}

fn name_key(n: &Node) -> String {
    n.path.file_name().and_then(|s| s.to_str())
        .map(|s| s.to_lowercase()).unwrap_or_default()
}

fn ext_key(n: &Node) -> String {
    n.path.extension().and_then(|s| s.to_str())
        .map(|s| s.to_lowercase()).unwrap_or_else(|| "~".into())
    // "~" sorts after most lowercase chars => extensionless last
}

fn cmp_by(a: &Node, b: &Node, mode: SortMode) -> Ordering {
    match mode {
        SortMode::Name => name_key(a).cmp(&name_key(b)),
        SortMode::Size => a.size.cmp(&b.size).then_with(|| name_key(a).cmp(&name_key(b))),
        SortMode::Mtime => a.mtime.cmp(&b.mtime).then_with(|| name_key(a).cmp(&name_key(b))),
        SortMode::Ext => ext_key(a).cmp(&ext_key(b))
            .then_with(|| name_key(a).cmp(&name_key(b))),
    }
}

/// Reorder `nodes` in place per `sort`. `nodes` is a slice of indices into
/// `arena_nodes` (typically a node's `children`).
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
        if sort.reverse { combined.reverse() } else { combined }
    });
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
            mk("aaa",   NodeKind::Dir,  0, 0),
            mk("a.txt", NodeKind::File, 0, 0),
            mk("zzz",   NodeKind::Dir,  0, 0),
        ];
        let mut idx = vec![0, 1, 2, 3];
        let s = SortState::default();
        sort_children(&mut idx, &nodes, &s);
        // Dirs first (aaa, zzz), then files (a.txt, b.txt)
        assert_eq!(idx, vec![1, 3, 2, 0]);
    }

    #[test]
    fn name_desc_no_dir_first() {
        let nodes = vec![
            mk("a", NodeKind::File, 0, 0),
            mk("b", NodeKind::Dir,  0, 0),
            mk("c", NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState { mode: SortMode::Name, reverse: true, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        assert_eq!(idx, vec![2, 1, 0]);
    }

    #[test]
    fn size_ascending_with_name_tiebreak() {
        let nodes = vec![
            mk("c", NodeKind::File, 10, 0),
            mk("a", NodeKind::File, 10, 0),
            mk("b", NodeKind::File,  5, 0),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState { mode: SortMode::Size, reverse: false, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        // 5 first, then 10s tied broken by name (a, c)
        assert_eq!(idx, vec![2, 1, 0]);
    }

    #[test]
    fn mtime_ascending() {
        let nodes = vec![
            mk("a", NodeKind::File, 0, 100),
            mk("b", NodeKind::File, 0,  50),
            mk("c", NodeKind::File, 0, 200),
        ];
        let mut idx = vec![0, 1, 2];
        let s = SortState { mode: SortMode::Mtime, reverse: false, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        assert_eq!(idx, vec![1, 0, 2]);
    }

    #[test]
    fn ext_groups_with_dotless_last() {
        let nodes = vec![
            mk("a.rs",    NodeKind::File, 0, 0),
            mk("readme",  NodeKind::File, 0, 0),
            mk("b.rs",    NodeKind::File, 0, 0),
            mk("c.md",    NodeKind::File, 0, 0),
        ];
        let mut idx = vec![0, 1, 2, 3];
        let s = SortState { mode: SortMode::Ext, reverse: false, dir_first: false };
        sort_children(&mut idx, &nodes, &s);
        // .md first, then .rs (a then b), then no-ext (readme)
        assert_eq!(idx, vec![3, 0, 2, 1]);
    }
}
```

- [ ] **Step 7: Register `mod sort` in lib.rs**

Edit `src/lib.rs`: add `pub mod sort;` (alphabetical position).

- [ ] **Step 8: Run sort tests; confirm green**

```
cargo test --lib --quiet sort
```

Expected: 5 passed.

- [ ] **Step 9: Add SortState to state.rs + persistence test**

Edit `src/state.rs`:

```rust
use crate::sort::SortState;

// in struct State (alphabetical / grouped position):
pub sort: SortState,
```

Append test:

```rust
#[test]
fn sort_persists_through_toml() {
    use crate::sort::{SortMode, SortState};
    let mut s = State::default();
    s.sort = SortState { mode: SortMode::Size, reverse: true, dir_first: false };
    let back = State::from_str(&s.to_toml());
    assert_eq!(back.sort, s.sort);
}

#[test]
fn missing_sort_section_yields_default() {
    let s = State::from_str("bookmarks = []\n");
    assert_eq!(s.sort, crate::sort::SortState::default());
}
```

Run: `cargo test --lib --quiet state` → all green incl. new tests.

- [ ] **Step 10: Commit sort module + state**

```
git add src/sort.rs src/lib.rs src/state.rs
git commit -m "feat(sort): SortMode/SortState module + state persistence"
```

---

## Task 2: Wire sort into Arena + Action variants + dispatch (no menu yet)

**Files:**
- Modify: `src/tree/arena.rs` (sort children using SortState in load_children; add `resort_all`)
- Modify: `src/app.rs` (`App.sort` field; pass to arena; `apply_sort` method; load from state on `new`)
- Modify: `src/keymap.rs` (new Action variants; `s` placeholder removed for now — sort menu wired in Task 3)
- Modify: `src/main.rs` (dispatch arms for sort actions)

- [ ] **Step 1: Refactor Arena to accept SortState**

Edit `src/tree/arena.rs`. Add `use crate::sort::SortState;` at top.

Change `Arena` struct:

```rust
pub struct Arena {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub show_hidden: bool,
}
```

(no new field — sort is passed per-call). Replace the hardcoded sort in `load_children`:

```rust
// REMOVE:
entries.sort_by(|a, b| {
    let ad = a.is_dir();
    let bd = b.is_dir();
    bd.cmp(&ad).then(a.file_name().cmp(&b.file_name()))
});
```

Restructure `load_children` to take `sort: &SortState` and use it AFTER building child nodes (so we have size/mtime to sort by):

```rust
pub fn load_children(&mut self, id: NodeId, sort: &SortState) -> io::Result<()> {
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
                || !p.file_name().and_then(|n| n.to_str())
                    .map(|n| n.starts_with('.')).unwrap_or(false)
        })
        .collect();
    let show_hidden = self.show_hidden;
    let mut child_ids = Vec::with_capacity(entries.len());
    for p in entries {
        let (kind, exec, size, mtime) = Self::classify(&p);
        let mut node = Node::new(p.clone(), kind, depth, Some(id));
        node.exec = exec;
        node.size = size;
        node.mtime = mtime;
        if matches!(kind, NodeKind::Dir) {
            node.empty = Some(dir_is_empty(&p, show_hidden));
        }
        self.nodes.push(node);
        child_ids.push(self.nodes.len() - 1);
    }
    crate::sort::sort_children(&mut child_ids, &self.nodes, sort);
    self.nodes[id].children = Some(child_ids);
    Ok(())
}
```

`Arena::new` needs SortState too:

```rust
pub fn new(root_path: &Path, sort: &SortState) -> io::Result<Self> {
    let kind = NodeKind::Dir;
    let root = Node::new(root_path.to_path_buf(), kind, 0, None);
    let mut a = Arena { nodes: vec![root], root: 0, show_hidden: false };
    a.load_children(0, sort)?;
    a.nodes[0].expanded = true;
    Ok(a)
}
```

`expand` and `toggle` too:

```rust
pub fn expand(&mut self, id: NodeId, sort: &SortState) -> io::Result<()> {
    if self.nodes[id].is_dir() {
        self.load_children(id, sort)?;
        self.nodes[id].expanded = true;
    }
    Ok(())
}

pub fn toggle(&mut self, id: NodeId, sort: &SortState) -> io::Result<()> {
    if self.nodes[id].expanded { self.collapse(id); }
    else { self.expand(id, sort)?; }
    Ok(())
}
```

`reveal` similarly takes `sort`.

Add `resort_all`:

```rust
/// Re-sort every loaded children list under the new sort state. Use after
/// the user changes sort mode/reverse/dir_first at runtime.
pub fn resort_all(&mut self, sort: &SortState) {
    let n = self.nodes.len();
    for id in 0..n {
        if let Some(mut ch) = self.nodes[id].children.take() {
            crate::sort::sort_children(&mut ch, &self.nodes, sort);
            self.nodes[id].children = Some(ch);
        }
    }
}
```

- [ ] **Step 2: Update existing arena tests**

Existing tests like `Arena::new(d.path()).unwrap()` need a `&SortState`. Add a helper at top of `mod tests`:

```rust
fn default_sort() -> crate::sort::SortState { crate::sort::SortState::default() }
```

Then change call sites: `Arena::new(d.path(), &default_sort()).unwrap()`, `a.expand(sub, &default_sort()).unwrap()`, `a.toggle(a.root, &default_sort()).unwrap()`, `a.reveal(&target, &default_sort())`.

The `root_loads_children_hidden_excluded` test asserts `names[0] == "sub"` (dir-first). Default `SortState { dir_first: true, mode: Name }` preserves this. Update assertion if needed (should still pass).

- [ ] **Step 3: Update all production call sites of Arena methods**

Find them:
```
rg -n "Arena::new|\.expand\(|\.toggle\(|\.reveal\(|\.load_children\(" src/ --type rust
```

Likely files: `src/app.rs` (open_tab calls `Arena::new`), `src/main.rs` (startup, refresh), `src/nav.rs` (expand/collapse), and any place that does `arena.load_children`.

Pass `&self.sort` (or `&self.config.sort` — actually `&self.sort` directly on App) at each call site.

For `App::new` and `open_tab`: thread `&App.sort` (or `&state.sort`) into the `Arena::new` call.

- [ ] **Step 4: Add `App.sort` field**

Edit `src/app.rs`:

```rust
pub struct App {
    // ... existing fields ...
    pub sort: crate::sort::SortState,
}
```

In `App::new`:

```rust
let sort = state.sort;  // copy from persisted state
let visible = arena.flatten();
App {
    arena,
    visible,
    // ... existing fields ...
    sort,
}
```

NOTE: `Arena` was constructed by the caller before `App::new`. Need to either (a) take `sort` as a 4th arg, or (b) construct Arena inside `App::new`. Choose (a) for minimal churn — caller (main.rs) reads `state.sort` to build `Arena`, passes both to `App::new`.

Add helper methods on App:

```rust
pub fn apply_sort(&mut self) {
    self.arena.resort_all(&self.sort);
    self.visible = self.arena.flatten();
    for tab in self.tabs.iter_mut() {
        tab.arena.resort_all(&self.sort);
        tab.visible = tab.arena.flatten();
    }
    // Reclamp selection to bounds (sort may have moved cursor's row index)
    self.selected = self.selected.min(self.visible.len().saturating_sub(1));
}

pub fn set_sort_mode(&mut self, mode: crate::sort::SortMode) {
    self.sort.mode = mode;
    self.state.sort = self.sort;
}

pub fn toggle_sort_reverse(&mut self) {
    self.sort.reverse = !self.sort.reverse;
    self.state.sort = self.sort;
}

pub fn toggle_sort_dir_first(&mut self) {
    self.sort.dir_first = !self.sort.dir_first;
    self.state.sort = self.sort;
}
```

- [ ] **Step 5: Add Action variants to keymap.rs**

```rust
pub enum Action {
    // ... existing variants ...
    SortMenu,
    SortByName, SortBySize, SortByMtime, SortByExt,
    SortToggleReverse, SortToggleDirFirst,
}
```

Update `Action::from_name`:

```rust
"sort_menu" => Some(Action::SortMenu),
"sort_name" => Some(Action::SortByName),
"sort_size" => Some(Action::SortBySize),
"sort_mtime" => Some(Action::SortByMtime),
"sort_ext"  => Some(Action::SortByExt),
"sort_reverse"   => Some(Action::SortToggleReverse),
"sort_dir_first" => Some(Action::SortToggleDirFirst),
```

Defaults (in `fn defaults`):

```rust
(SortMenu, vec!["s"]),
```

The granular sort actions are not bound from `Mode::Normal` directly; they're bound via the SortMenu chord resolver (Task 3). Skip default chord bindings for them here.

- [ ] **Step 6: Update the `t_resolves_to_new_tab_and_w_to_close_tab` test for `s`**

If there's an existing test asserting `s` resolves to None or something else, update it. The previous "capital_t" test is precedent. Likely just add:

```rust
#[test]
fn s_resolves_to_sort_menu() {
    let km = Keymap::build(&BTreeMap::new());
    assert_eq!(km.resolve(Mode::Normal, "s"), Some(Action::SortMenu));
}
```

- [ ] **Step 7: Dispatch in main.rs**

Find the Action match (likely a large match in the main loop). Add:

```rust
Action::SortMenu => app.mode = Mode::SortMenu,
Action::SortByName  => { app.set_sort_mode(crate::sort::SortMode::Name);  app.apply_sort(); app.mode = explorenova::keymap::Mode::Normal; }
Action::SortBySize  => { app.set_sort_mode(crate::sort::SortMode::Size);  app.apply_sort(); app.mode = explorenova::keymap::Mode::Normal; }
Action::SortByMtime => { app.set_sort_mode(crate::sort::SortMode::Mtime); app.apply_sort(); app.mode = explorenova::keymap::Mode::Normal; }
Action::SortByExt   => { app.set_sort_mode(crate::sort::SortMode::Ext);   app.apply_sort(); app.mode = explorenova::keymap::Mode::Normal; }
Action::SortToggleReverse   => { app.toggle_sort_reverse();   app.apply_sort(); }
Action::SortToggleDirFirst  => { app.toggle_sort_dir_first(); app.apply_sort(); }
```

(use the actual `explorenova::...` path that matches local re-exports — match existing dispatch style for other Actions in main.rs.)

- [ ] **Step 8: Build + run full test suite**

```
cargo build --release 2>&1 | tail -20
cargo test --lib --quiet 2>&1 | tail -5
```

Both must succeed; tests must show all pass. Expected total: ~124 (119 baseline + 5 sort + 2 state + others).

- [ ] **Step 9: Commit Arena wiring + dispatch**

```
git add src/tree/arena.rs src/app.rs src/keymap.rs src/main.rs
git commit -m "feat(sort): wire SortState into Arena, App, dispatch (no menu UI yet)"
```

---

## Task 3: Sort menu modal + Mode::SortMenu + keymap routing

**Files:**
- Modify: `src/keymap.rs` (add `Mode::SortMenu`; resolve digit/letter keys in this mode)
- Modify: `src/sort.rs` (add `render_menu` fn)
- Modify: `src/ui/mod.rs` (render menu when in SortMenu mode)
- Modify: `src/main.rs` (route input through SortMenu resolver)

- [ ] **Step 1: Add `Mode::SortMenu` to keymap.rs Mode enum**

```rust
pub enum Mode {
    Normal,
    Search,
    Input,
    Confirm,
    SortMenu,
    // Help added in Task 4
}
```

Extend `Keymap::resolve`:

```rust
Mode::SortMenu => match buffer {
    "esc"        => Some(Action::Cancel),
    "1" | "n"    => Some(Action::SortByName),
    "2" | "s"    => Some(Action::SortBySize),
    "3" | "m"    => Some(Action::SortByMtime),
    "4" | "e"    => Some(Action::SortByExt),
    "r"          => Some(Action::SortToggleReverse),
    "d"          => Some(Action::SortToggleDirFirst),
    _            => None,
},
```

- [ ] **Step 2: Add test for SortMenu resolution**

```rust
#[test]
fn sort_menu_routes_digits_and_letters() {
    let km = Keymap::build(&BTreeMap::new());
    assert_eq!(km.resolve(Mode::SortMenu, "1"), Some(Action::SortByName));
    assert_eq!(km.resolve(Mode::SortMenu, "s"), Some(Action::SortBySize));
    assert_eq!(km.resolve(Mode::SortMenu, "r"), Some(Action::SortToggleReverse));
    assert_eq!(km.resolve(Mode::SortMenu, "esc"), Some(Action::Cancel));
}
```

Run: `cargo test --lib --quiet keymap::tests::sort_menu_routes` → PASS.

- [ ] **Step 3: Add `render_menu` to sort.rs**

Append to `src/sort.rs`:

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render_menu(area: Rect, frame: &mut Frame, sort: &SortState) {
    frame.render_widget(Clear, area);
    let block = Block::default().title(" Sort ").borders(Borders::ALL);
    let mark = |m: SortMode| if sort.mode == m { " (active)" } else { "" };
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
            Span::styled(on_off(sort.reverse), Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw("  d  Dir first: "),
            Span::styled(on_off(sort.dir_first), Style::default().add_modifier(Modifier::BOLD)),
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
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
```

- [ ] **Step 4: Render the menu in ui/mod.rs**

In `src/ui/mod.rs` at the end of `render_frame` (after panes drawn):

```rust
if app.mode == crate::keymap::Mode::SortMenu {
    let r = crate::sort::centered_rect(40, 12, frame.size());
    crate::sort::render_menu(r, frame, &app.sort);
}
```

- [ ] **Step 5: Route input through SortMenu resolver in main.rs**

Locate where `keymap.resolve(app.mode, &buffer)` (or equivalent) is called. The existing path likely already routes by `app.mode`. Confirm `Mode::SortMenu` reaches the resolver and the resulting action falls through to the dispatch added in Task 2. If not, add an explicit `Mode::SortMenu => km.resolve(Mode::SortMenu, &buf)` arm.

Also handle: when `Mode::SortMenu` receives `Action::Cancel`, set `app.mode = Mode::Normal` (don't fall into Normal-mode Cancel which clears marks).

```rust
// In dispatch:
Action::Cancel if app.mode == Mode::SortMenu => app.mode = Mode::Normal,
// keep existing Cancel arm below
```

Order matters — match the SortMenu-Cancel arm before the generic Cancel arm.

- [ ] **Step 6: Build + smoke**

```
cargo build --release 2>&1 | tail -5
cargo test --lib --quiet 2>&1 | tail -5
```

Both green.

- [ ] **Step 7: Commit sort menu**

```
git add src/keymap.rs src/sort.rs src/ui/mod.rs src/main.rs
git commit -m "feat(sort): Mode::SortMenu modal + key routing for 1-4/r/d/esc"
```

---

## Task 4: Help overlay (`?`)

**Files:**
- Create: `src/help.rs`
- Modify: `src/lib.rs` (register `pub mod help;`)
- Modify: `src/keymap.rs` (add `Help` Action + `Mode::Help`; defaults `?`)
- Modify: `src/main.rs` (dispatch `Help`; any key in `Mode::Help` exits)
- Modify: `src/ui/mod.rs` (render help when in Help mode)

- [ ] **Step 1: Add `Help` Action and `Mode::Help`**

`src/keymap.rs`:

```rust
pub enum Mode { Normal, Search, Input, Confirm, SortMenu, Help }

pub enum Action {
    // ... existing ...
    Help,
}
```

`Action::from_name`: `"help" => Some(Action::Help)`.

Defaults: `(Help, vec!["?"])`.

`Keymap::resolve`:

```rust
Mode::Help => Some(Action::Cancel),  // any key dismisses
```

NOTE: `Help` is a key-event resolution edge case. The resolver receives a `buffer` string from `key_token`. To make "any key dismisses" work, return `Some(Action::Cancel)` regardless of `buffer` value. Dispatch handles routing Cancel-in-Help-mode back to Normal.

- [ ] **Step 2: Test `?` resolves to Help and Help-mode any-key returns Cancel**

```rust
#[test]
fn question_mark_resolves_to_help() {
    let km = Keymap::build(&BTreeMap::new());
    assert_eq!(km.resolve(Mode::Normal, "?"), Some(Action::Help));
}

#[test]
fn help_mode_any_key_returns_cancel() {
    let km = Keymap::build(&BTreeMap::new());
    assert_eq!(km.resolve(Mode::Help, "j"), Some(Action::Cancel));
    assert_eq!(km.resolve(Mode::Help, "?"), Some(Action::Cancel));
    assert_eq!(km.resolve(Mode::Help, "esc"), Some(Action::Cancel));
}
```

Run: `cargo test --lib --quiet keymap` → all green.

- [ ] **Step 3: Create `src/help.rs`**

```rust
use crate::keymap::{Action, Keymap};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub struct HelpCategory {
    pub name: &'static str,
    pub rows: Vec<(String, String)>, // (label, keys)
}

/// (Action, category, human label). Order within a category preserved in render.
const ACTION_META: &[(Action, &str, &str)] = &[
    // Navigation
    (Action::Down, "Navigation", "Down"),
    (Action::Up, "Navigation", "Up"),
    (Action::Top, "Navigation", "Top"),
    (Action::Bottom, "Navigation", "Bottom"),
    (Action::HalfDown, "Navigation", "Half page down"),
    (Action::HalfUp, "Navigation", "Half page up"),
    (Action::Open, "Navigation", "Expand / open"),
    (Action::Collapse, "Navigation", "Collapse / parent"),
    (Action::Parent, "Navigation", "Up to parent dir"),
    (Action::Toggle, "Navigation", "Toggle / open default"),
    (Action::OpenDefault, "Navigation", "Open default"),
    (Action::OpenWith, "Navigation", "Open with…"),
    (Action::JumpHome, "Navigation", "Jump home"),
    (Action::PathInput, "Navigation", "Path input"),

    // Panes / Tabs
    (Action::FocusNext, "Panes / Tabs", "Cycle focus / swap tabs"),
    (Action::ToggleBookmarksPane, "Panes / Tabs", "Toggle bookmarks pane"),
    (Action::TogglePreviewPane, "Panes / Tabs", "Toggle preview pane"),
    (Action::ResizeShrink, "Panes / Tabs", "Shrink pane"),
    (Action::ResizeGrow, "Panes / Tabs", "Grow pane"),
    (Action::ReorderLeft, "Panes / Tabs", "Move pane left"),
    (Action::ReorderRight, "Panes / Tabs", "Move pane right"),
    (Action::NewTab, "Panes / Tabs", "New tab (split)"),
    (Action::CloseTab, "Panes / Tabs", "Close tab"),

    // Files
    (Action::Create, "Files", "Create file/dir"),
    (Action::Rename, "Files", "Rename"),
    (Action::Copy, "Files", "Copy"),
    (Action::Cut, "Files", "Cut"),
    (Action::Paste, "Files", "Paste"),
    (Action::Trash, "Files", "Move to trash"),
    (Action::PermanentDelete, "Files", "Permanent delete"),
    (Action::OpenTrash, "Files", "Open trash"),
    (Action::SysClipCopy, "Files", "Copy path to system clipboard"),

    // Selection
    (Action::ToggleMark, "Selection", "Toggle mark"),
    (Action::SelectAllVisible, "Selection", "Select all visible"),
    (Action::ToggleVisualMode, "Selection", "Visual mode"),

    // Search / Bookmarks
    (Action::Search, "Search / Bookmarks", "Fuzzy search"),
    (Action::SearchNext, "Search / Bookmarks", "Next match"),
    (Action::SearchPrev, "Search / Bookmarks", "Prev match"),
    (Action::ToggleBookmark, "Search / Bookmarks", "Toggle bookmark"),

    // Shell / Misc
    (Action::ShellOnce, "Shell / Misc", "Shell one-shot"),
    (Action::ShellDrop, "Shell / Misc", "Drop to shell"),
    (Action::ShowDevices, "Shell / Misc", "Devices modal"),
    (Action::ToggleHidden, "Shell / Misc", "Toggle hidden"),
    (Action::Refresh, "Shell / Misc", "Refresh"),
    (Action::SortMenu, "Shell / Misc", "Sort menu"),
    (Action::Help, "Shell / Misc", "Help (this overlay)"),
    (Action::Quit, "Shell / Misc", "Quit"),
];

pub fn build_entries(km: &Keymap) -> Vec<HelpCategory> {
    let mut out: Vec<HelpCategory> = Vec::new();
    for &(action, cat, label) in ACTION_META {
        let keys = km.keys_for(action).join(" / ");
        if keys.is_empty() { continue; }
        let row = (label.to_string(), keys);
        if let Some(last) = out.last_mut() {
            if last.name == cat { last.rows.push(row); continue; }
        }
        out.push(HelpCategory { name: cat, rows: vec![row] });
    }
    out
}

pub fn render(area: Rect, frame: &mut Frame, entries: &[HelpCategory]) {
    frame.render_widget(Clear, area);
    let block = Block::default().title(" explorenova keys ").borders(Borders::ALL);
    let inner_w = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for cat in entries {
        lines.push(Line::from(Span::styled(
            cat.name,
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (label, keys) in &cat.rows {
            let dots = inner_w.saturating_sub(label.len() + keys.len()).max(2);
            lines.push(Line::from(format!("  {}{}{}", label, " ".repeat(dots), keys)));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from("  [any key dismisses]"));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
```

- [ ] **Step 4: Add `Keymap::keys_for` lookup helper**

Edit `src/keymap.rs`:

```rust
impl Keymap {
    /// Reverse-lookup: all chords currently bound to `action` (deduped, sorted).
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        let mut v: Vec<String> = self.normal.iter()
            .filter(|(_, &a)| a == action)
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }
}
```

(Confirm `self.normal: BTreeMap<String, Action>` — the field name. Adjust if different.)

- [ ] **Step 5: Add help::build_entries test**

In `src/help.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn build_includes_at_least_one_row_per_category() {
        let km = Keymap::build(&BTreeMap::new());
        let cats = build_entries(&km);
        let names: Vec<_> = cats.iter().map(|c| c.name).collect();
        assert!(names.contains(&"Navigation"));
        assert!(names.contains(&"Files"));
        assert!(names.contains(&"Panes / Tabs"));
        for c in &cats { assert!(!c.rows.is_empty(), "{} empty", c.name); }
    }

    #[test]
    fn user_remap_reflected_in_keys() {
        let mut overrides = BTreeMap::new();
        overrides.insert("quit".to_string(), vec!["Q".to_string()]);
        let km = Keymap::build(&overrides);
        let cats = build_entries(&km);
        let quit_row = cats.iter().flat_map(|c| &c.rows)
            .find(|(l, _)| l == "Quit").unwrap();
        assert_eq!(quit_row.1, "Q");
    }
}
```

- [ ] **Step 6: Register and run tests**

Add `pub mod help;` to `src/lib.rs`.

```
cargo test --lib --quiet help
```

Expected: 2 passed.

- [ ] **Step 7: Render help in ui/mod.rs**

After the SortMenu render block:

```rust
if app.mode == crate::keymap::Mode::Help {
    let entries = crate::help::build_entries(&app.keymap);
    let content_h = entries.iter().map(|c| c.rows.len() as u16 + 2).sum::<u16>() + 4;
    let w = 80.min(frame.size().width.saturating_sub(4));
    let h = content_h.min(frame.size().height.saturating_sub(4));
    let r = crate::sort::centered_rect(w, h, frame.size());
    crate::help::render(r, frame, &entries);
}
```

(Confirm `app.keymap: Keymap` is on App — if not, the keymap lives elsewhere. Likely passed through from main.rs. If App doesn't hold it, plumb it through or accept it as a param to ui::render_frame.)

- [ ] **Step 8: Dispatch `Help` in main.rs**

```rust
Action::Help => app.mode = Mode::Help,
Action::Cancel if app.mode == Mode::Help => app.mode = Mode::Normal,
```

Ensure the Help-Cancel arm appears before the generic Cancel arm.

- [ ] **Step 9: Build + test**

```
cargo build --release 2>&1 | tail -5
cargo test --lib --quiet 2>&1 | tail -5
```

All green.

- [ ] **Step 10: Commit help overlay**

```
git add src/help.rs src/lib.rs src/keymap.rs src/main.rs src/ui/mod.rs
git commit -m "feat(help): ? overlay listing active keybindings (modal, any-key dismiss)"
```

---

## Task 5: Status bar — 3-segment renderer + disk free + marks bytes + transient fade

**Files:**
- Create: `src/status.rs`
- Modify: `src/lib.rs` (register `pub mod status;`)
- Modify: `src/app.rs` (`marks_bytes`, `disk_free`, `status_set_at`, `set_status`, `refresh_disk_free`, mark-toggle bookkeeping)
- Modify: `src/ui/mod.rs` (replace existing status row with `status::render`)
- Modify: dependency: add `libc = "0.2"` to Cargo.toml if not already present

- [ ] **Step 1: Check Cargo.toml for libc**

```
rg "^libc" Cargo.toml
```

If absent, add under `[dependencies]`: `libc = "0.2"`.

- [ ] **Step 2: Add `App` fields**

`src/app.rs`:

```rust
pub struct App {
    // ... existing ...
    pub marks_bytes: u64,
    pub disk_free: Option<(PathBuf, u64, std::time::Instant)>,
    pub status_set_at: std::time::Instant,
}
```

`App::new`:

```rust
marks_bytes: 0,
disk_free: None,
status_set_at: std::time::Instant::now(),
```

Add setter:

```rust
pub fn set_status<S: Into<String>>(&mut self, s: S) {
    self.status = s.into();
    self.status_set_at = std::time::Instant::now();
}
```

Then **migrate all existing `app.status = X` assignments** to `app.set_status(X)`. Search:

```
rg -n "\.status = " src/ --type rust
```

There will be many sites (main.rs especially). Replace each. For sites that assign `app.status = String::new()`, replace with `app.set_status("")`.

- [ ] **Step 3: Update mark toggle to track marks_bytes**

Find the mark-toggle path. Likely in `src/app.rs` or a method like `toggle_mark`. The mark set is `app.marks: BTreeSet<PathBuf>`. Wherever marks are added/removed, update `marks_bytes`.

Helper:

```rust
fn node_size_at(&self, p: &Path) -> u64 {
    self.arena.nodes.iter().find(|n| n.path == p && !n.is_dir())
        .map(|n| n.size).unwrap_or(0)
}

pub fn toggle_mark_at(&mut self, p: PathBuf) {
    let size = self.node_size_at(&p);
    if self.marks.contains(&p) {
        self.marks.remove(&p);
        self.marks_bytes = self.marks_bytes.saturating_sub(size);
    } else {
        self.marks.insert(p);
        self.marks_bytes = self.marks_bytes.saturating_add(size);
    }
}

pub fn clear_marks(&mut self) {
    self.marks.clear();
    self.marks_bytes = 0;
}

pub fn recompute_marks_bytes(&mut self) {
    self.marks_bytes = self.marks.iter()
        .map(|p| self.node_size_at(p)).sum();
}
```

Replace existing mark-set mutations to go through these methods. For `select-all-visible`, call `recompute_marks_bytes` after populating.

- [ ] **Step 4: Create `src/status.rs`**

```rust
use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::path::Path;
use std::time::Duration;

pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{}{}", n, UNITS[0]) }
    else if v >= 10.0 { format!("{:.0}{}", v, UNITS[i]) }
    else { format!("{:.1}{}", v, UNITS[i]) }
}

pub fn truncate_left(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let keep = max.saturating_sub(2);
    let start = s.len() - keep;
    let mut t = String::with_capacity(max);
    t.push_str("…/");
    t.push_str(&s[start..]);
    t
}

#[cfg(unix)]
pub fn disk_free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut buf) };
    if rc != 0 { return None; }
    Some(buf.f_bavail as u64 * buf.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn disk_free_bytes(_path: &Path) -> Option<u64> { None }

fn cursor_meta(app: &App) -> (String, String, String, String) {
    let path_s = match app.visible.get(app.selected)
        .and_then(|&id| app.arena.nodes.get(id))
    {
        Some(n) => n.path.display().to_string(),
        None => return ("".into(), "".into(), "".into(), "".into()),
    };
    let n = &app.arena.nodes[app.visible[app.selected]];
    let perms = format_perms(n);
    let size = if n.is_dir() { "-".into() } else { human_bytes(n.size) };
    let mtime = format_mtime(n.mtime);
    (path_s, perms, size, mtime)
}

#[cfg(unix)]
fn format_perms(n: &crate::tree::node::Node) -> String {
    use std::os::unix::fs::MetadataExt;
    let m = match std::fs::symlink_metadata(&n.path) { Ok(m) => m, Err(_) => return "?".into() };
    let mode = m.mode();
    let kind = if n.is_dir() { 'd' } else if m.file_type().is_symlink() { 'l' } else { '-' };
    let bits = |s: u32| {
        let r = if mode >> s & 0o4 != 0 { 'r' } else { '-' };
        let w = if mode >> s & 0o2 != 0 { 'w' } else { '-' };
        let x = if mode >> s & 0o1 != 0 { 'x' } else { '-' };
        format!("{r}{w}{x}")
    };
    format!("{kind}{}{}{}", bits(6), bits(3), bits(0))
}

#[cfg(not(unix))]
fn format_perms(_n: &crate::tree::node::Node) -> String { "".into() }

fn format_mtime(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = days_to_ymd(secs / 86400);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// Civil-from-days (Howard Hinnant). Pure integer date conversion, no chrono dep.
fn days_to_ymd(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub fn render(area: Rect, frame: &mut Frame, app: &App) {
    let segs = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(20), Constraint::Percentage(20)])
        .split(area);

    // Left: transient status (≤3s) else cursor meta
    let left = if !app.status.is_empty() && app.status_set_at.elapsed() < Duration::from_secs(3) {
        app.status.clone()
    } else {
        let (p, perms, size, mt) = cursor_meta(app);
        let max = segs[0].width as usize;
        let path_max = max.saturating_sub(perms.len() + size.len() + mt.len() + 6);
        format!("{}  {}  {}  {}", truncate_left(&p, path_max), perms, size, mt)
    };
    frame.render_widget(Paragraph::new(left), segs[0]);

    // Center: marks summary (or blank)
    let center = if app.marks.is_empty() { String::new() }
        else { format!("{} sel • {}", app.marks.len(), human_bytes(app.marks_bytes)) };
    frame.render_widget(Paragraph::new(Span::styled(center, Style::default().add_modifier(Modifier::BOLD))), segs[1]);

    // Right: items • free
    let items = app.visible.len();
    let free = app.disk_free.as_ref().map(|(_, b, _)| human_bytes(*b)).unwrap_or_else(|| "?".into());
    let right = format!("{} items • {} free", items, free);
    frame.render_widget(Paragraph::new(right), segs[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_boundaries() {
        assert_eq!(human_bytes(0), "0B");
        assert_eq!(human_bytes(1023), "1023B");
        assert_eq!(human_bytes(1024), "1.0K");
        assert_eq!(human_bytes(10 * 1024), "10K");
        assert_eq!(human_bytes(1024 * 1024), "1.0M");
        assert_eq!(human_bytes(50 * 1024 * 1024 * 1024), "50G");
    }

    #[test]
    fn truncate_left_keeps_tail() {
        let s = "/home/nova/personal/explorer/src/app.rs";
        let t = truncate_left(s, 20);
        assert!(t.starts_with("…/"));
        assert!(t.ends_with("src/app.rs"));
        assert!(t.len() <= 20);
    }

    #[test]
    fn days_to_ymd_known_dates() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(20596), (2026, 5, 24));
    }
}
```

- [ ] **Step 5: Refresh-disk-free path**

In `src/app.rs`:

```rust
pub fn refresh_disk_free(&mut self) {
    use std::time::{Duration, Instant};
    let cwd = self.arena.nodes[self.arena.root].path.clone();
    let stale = match &self.disk_free {
        None => true,
        Some((p, _, t)) => p != &cwd || t.elapsed() > Duration::from_secs(30),
    };
    if stale {
        if let Some(b) = crate::status::disk_free_bytes(&cwd) {
            self.disk_free = Some((cwd, b, Instant::now()));
        }
    }
}
```

Call `app.refresh_disk_free()` once per frame in the main loop (right before `terminal.draw(...)`).

- [ ] **Step 6: Replace existing status row in ui/mod.rs with status::render**

Find current status row render (likely a single-line `Paragraph::new(&app.status)` in `render_frame`). Replace with:

```rust
crate::status::render(status_area, frame, app);
```

where `status_area` is the existing bottom-row Rect.

- [ ] **Step 7: Register mod + run tests**

Add `pub mod status;` to `src/lib.rs`.

```
cargo build --release 2>&1 | tail -10
cargo test --lib --quiet 2>&1 | tail -5
```

All green. Expected: ~129 lib tests total.

- [ ] **Step 8: Commit status bar**

```
git add Cargo.toml src/status.rs src/lib.rs src/app.rs src/ui/mod.rs
git commit -m "feat(status): 3-segment status bar — cursor meta / marks / items+free disk"
```

---

## Task 6: README + manual smoke

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README keys table**

Add to the keys table:

```
| Help overlay | `?` |
| Sort menu | `s` (then `1`/`n` name, `2`/`s` size, `3`/`m` mtime, `4`/`e` ext, `r` reverse, `d` dir-first) |
```

Add a short note under Configuration about persisted sort:

> Sort settings (mode, reverse, dir-first) persist in `state.toml` under
> `[sort]`. Change them at runtime via the `s` menu.

- [ ] **Step 2: Manual smoke checklist**

Run `cargo build --release && target/release/explorenova`. Verify:
- [ ] `?` opens help overlay; pressing any key dismisses.
- [ ] `s` opens sort menu; pressing `2` sorts by size and closes menu.
- [ ] `s r` toggles reverse and menu shows `Reverse: on`; `esc` closes.
- [ ] Status bar shows `path  perms  size  mtime` on the left.
- [ ] Mark 3 files with `space`; center shows `3 sel • X.X MB`.
- [ ] Right shows `N items • Y free`.
- [ ] Transient status (e.g. after copy) overlays left segment ~3s then reverts.
- [ ] Restart binary; sort choice persists.
- [ ] Existing flows (tabs `t`/`w`/`Tab`, copy/paste, search `/`) still work.

- [ ] **Step 3: Commit README**

```
git add README.md
git commit -m "docs: README updates for ? help, s sort menu, persisted [sort] state"
```

- [ ] **Step 4: Tally + final build**

```
git log --oneline master..HEAD
cargo test --lib --quiet 2>&1 | tail -3
cargo build --release 2>&1 | tail -3
```

Confirm 6-9 commits on branch, all tests pass, binary builds clean.

---

## Done

Phase A complete. Branch `phase-a-ux-polish` ready to merge to master (no remote, no PR per project pattern):

```
git checkout master && git merge --no-ff phase-a-ux-polish \
  -m "merge: Phase A UX polish (? help, s sort, 3-segment status)"
git branch -d phase-a-ux-polish
```
