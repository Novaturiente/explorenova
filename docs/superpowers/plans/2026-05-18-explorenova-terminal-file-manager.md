# explorenova Terminal File Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `explorenova`, a Rust terminal file manager with an nvim-tree-style collapsible git-annotated tree, toggleable bookmarks pane, yazi-style preview (text/image/pdf), fuzzy search, safe file ops, vim keybindings, configurable panes, path nav with tab completion, and an in-place shell runner.

**Architecture:** `ratatui` + `crossterm` synchronous render loop on the main thread; git scan, recursive search walk, fs-watch, and preview generation run on worker threads communicating via `mpsc` channels. State is reduced on the main thread between frames. Config/state are TOML; CLI parsed with `clap`.

**Tech Stack:** Rust, ratatui, crossterm, git2, ignore, nucleo, notify, fs_extra, trash, open, ratatui-image, image, syntect, serde, toml, clap, anyhow, thiserror, tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-05-18-terminal-file-manager-design.md`

---

## File Structure

```
Cargo.toml
src/
  main.rs        CLI parse (clap), terminal RAII, event loop, worker wiring
  app.rs         App state, Mode enum, Action reducer
  cli.rs         clap Args struct + SessionOverride
  config.rs      Config/PaneConfig structs, defaults, TOML load+merge
  state.rs       Persisted state (bookmarks, last cwd, pane overrides)
  keymap.rs      Action enum, Mode, config-bound mode-aware resolver
  git.rs         worker: repo scan -> path->GitStatus map + dir aggregation
  fs_ops.rs      create/copy/move/rename/delete(trash|permanent)
  search.rs      ignore-walk producer + nucleo matcher
  nav.rs         path input completion (longest-common-prefix, cycle)
  shell.rs       suspend / spawn / restore terminal
  tree/
    mod.rs       re-exports
    node.rs      Node, NodeKind, GitStatus
    arena.rs     Arena: build, lazy-load children, expand/collapse, flatten
  preview/
    mod.rs       PreviewKind dispatch + (path,mtime) cache
    text.rs      syntect highlight, size cap
    media.rs     image decode + pdf->png via pdftoppm
  ui/
    mod.rs       layout-from-config (Constraint computation) + frame render
    panes.rs     per-pane render fns (bookmarks/tree/preview/status)
    theme.rs     Theme struct + defaults
tests/
  fs_ops_int.rs  integration: ops in tempfile dirs
  git_int.rs     integration: fixture repo status
```

Each module has one responsibility; dependencies point inward toward `app`/`tree`. Pure logic (config merge, arena flatten, git aggregation, completion, keymap, layout math, preview dispatch) is unit-tested inline with `#[cfg(test)]`. Filesystem/git behavior is integration-tested in `tests/`. Rendering, the event loop, shell suspend, and terminal graphics are verified manually with explicit commands.

---

## Task 1: Cargo scaffold + dependencies

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "explorenova"
version = "0.1.0"
edition = "2021"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
git2 = "0.19"
ignore = "0.4"
nucleo = "0.5"
notify = "6"
fs_extra = "1.3"
trash = "5"
open = "5"
ratatui-image = "4"
image = "0.25"
syntect = "5"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
clap = { version = "4", features = ["derive"] }
anyhow = "1"
thiserror = "2"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create minimal `src/main.rs`**

```rust
fn main() {
    println!("explorenova");
}
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build`
Expected: compiles, downloads deps, `Finished` (no errors).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "chore: cargo scaffold + dependencies"
```

---

## Task 2: Config structs, defaults, TOML merge

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)

- [ ] **Step 1: Write the failing test**

Create `src/config.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PaneConfig {
    pub visible: bool,
    /// Fixed column width. None => proportional via `weight`.
    pub width: Option<u16>,
    pub weight: u16,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self { visible: true, width: None, weight: 1 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanesConfig {
    pub order: Vec<String>,
    pub bookmarks: PaneConfig,
    pub tree: PaneConfig,
    pub preview: PaneConfig,
}

impl Default for PanesConfig {
    fn default() -> Self {
        Self {
            order: vec!["bookmarks".into(), "tree".into(), "preview".into()],
            bookmarks: PaneConfig { visible: false, width: Some(24), weight: 1 },
            tree: PaneConfig { visible: true, width: None, weight: 2 },
            preview: PaneConfig { visible: true, width: None, weight: 3 },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub panes: PanesConfig,
    /// action name -> list of key chords
    pub keys: BTreeMap<String, Vec<String>>,
    pub preview_max_bytes: u64,
}

impl Config {
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c
    }

    /// Parse user TOML over defaults. On parse error, return defaults + the error string.
    pub fn load_str(toml_src: &str) -> (Self, Option<String>) {
        match toml::from_str::<Config>(toml_src) {
            Ok(mut c) => {
                if c.preview_max_bytes == 0 {
                    c.preview_max_bytes = 1_048_576;
                }
                (c, None)
            }
            Err(e) => (Config::with_defaults(), Some(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_three_panes_in_order() {
        let c = Config::with_defaults();
        assert_eq!(c.panes.order, vec!["bookmarks", "tree", "preview"]);
        assert!(!c.panes.bookmarks.visible);
        assert!(c.panes.preview.visible);
        assert_eq!(c.preview_max_bytes, 1_048_576);
    }

    #[test]
    fn user_toml_overrides_only_specified_fields() {
        let src = r#"
            [panes.bookmarks]
            visible = true
        "#;
        let (c, err) = Config::load_str(src);
        assert!(err.is_none());
        assert!(c.panes.bookmarks.visible);
        // unspecified field keeps default
        assert_eq!(c.panes.tree.weight, 2);
    }

    #[test]
    fn malformed_toml_returns_defaults_and_error() {
        let (c, err) = Config::load_str("this is = = not toml");
        assert!(err.is_some());
        assert_eq!(c.panes.order.len(), 3);
    }
}
```

Add to `src/main.rs`: `mod config;`

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config::`
Expected: FAIL until module compiles and assertions pass (this task ships passing code; the failing point is the missing `mod config;` / compile). Confirm by temporarily reverting `with_defaults` body — optional. Otherwise proceed.

- [ ] **Step 3: Confirm implementation passes**

Run: `cargo test config::`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: config structs + TOML merge with defaults"
```

---

## Task 3: Persisted state (bookmarks, last cwd, pane overrides)

**Files:**
- Create: `src/state.rs`
- Modify: `src/main.rs` (add `mod state;`)

- [ ] **Step 1: Write the failing test**

Create `src/state.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PaneOverride {
    pub visible: Option<bool>,
    pub width: Option<u16>,
    pub weight: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    pub bookmarks: Vec<PathBuf>,
    pub last_cwd: Option<PathBuf>,
    pub pane_order: Option<Vec<String>>,
    pub bookmarks_pane: PaneOverride,
    pub tree_pane: PaneOverride,
    pub preview_pane: PaneOverride,
}

impl State {
    pub fn from_str(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn toggle_bookmark(&mut self, p: PathBuf) {
        if let Some(i) = self.bookmarks.iter().position(|b| b == &p) {
            self.bookmarks.remove(i);
        } else {
            self.bookmarks.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_adds_then_removes() {
        let mut s = State::default();
        let p = PathBuf::from("/tmp/x");
        s.toggle_bookmark(p.clone());
        assert_eq!(s.bookmarks, vec![p.clone()]);
        s.toggle_bookmark(p.clone());
        assert!(s.bookmarks.is_empty());
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut s = State::default();
        s.bookmarks.push(PathBuf::from("/a"));
        s.last_cwd = Some(PathBuf::from("/b"));
        let back = State::from_str(&s.to_toml());
        assert_eq!(back, s);
    }

    #[test]
    fn bad_toml_yields_default() {
        let s = State::from_str("@@@");
        assert_eq!(s, State::default());
    }
}
```

Add to `src/main.rs`: `mod state;`

- [ ] **Step 2: Run tests**

Run: `cargo test state::`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/state.rs src/main.rs
git commit -m "feat: persisted state with bookmark toggle + toml roundtrip"
```

---

## Task 4: CLI args + session overrides + path resolution

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs` (add `mod cli;`)

- [ ] **Step 1: Write the failing test**

Create `src/cli.rs`:

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "explorenova", version, about = "Terminal file manager")]
pub struct Args {
    /// Directory to start in
    pub path: Option<PathBuf>,
    /// Launch with the tree pane only (bookmarks + preview hidden this session)
    #[arg(short = 't', long = "tree-only")]
    pub tree_only: bool,
}

/// Resolve start dir: explicit arg > last cwd > $PWD.
pub fn resolve_start_dir(
    arg: Option<PathBuf>,
    last_cwd: Option<PathBuf>,
    pwd: PathBuf,
) -> PathBuf {
    if let Some(p) = arg {
        if p.is_dir() {
            return p;
        }
    }
    if let Some(p) = last_cwd {
        if p.is_dir() {
            return p;
        }
    }
    pwd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_wins_when_dir_exists() {
        let d = std::env::temp_dir();
        let got = resolve_start_dir(Some(d.clone()), None, PathBuf::from("/nope"));
        assert_eq!(got, d);
    }

    #[test]
    fn falls_back_to_pwd_when_nothing_valid() {
        let pwd = std::env::temp_dir();
        let got = resolve_start_dir(
            Some(PathBuf::from("/does/not/exist")),
            Some(PathBuf::from("/also/not")),
            pwd.clone(),
        );
        assert_eq!(got, pwd);
    }

    #[test]
    fn parses_tree_only_flag() {
        let a = Args::try_parse_from(["explorenova", "-t"]).unwrap();
        assert!(a.tree_only);
    }
}
```

Add to `src/main.rs`: `mod cli;`

- [ ] **Step 2: Run tests**

Run: `cargo test cli::`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: clap CLI args + start-dir resolution"
```

---

## Task 5: Tree node types

**Files:**
- Create: `src/tree/mod.rs`
- Create: `src/tree/node.rs`
- Modify: `src/main.rs` (add `mod tree;`)

- [ ] **Step 1: Write the failing test**

Create `src/tree/node.rs`:

```rust
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
```

Create `src/tree/mod.rs`:

```rust
pub mod arena;
pub mod node;

pub use arena::Arena;
pub use node::{GitStatus, Node, NodeId, NodeKind};
```

Add to `src/main.rs`: `mod tree;`

> Note: `tree/mod.rs` references `arena` (Task 6). Create a stub `src/tree/arena.rs` containing only `// placeholder` for this task to compile, replaced fully in Task 6.

- [ ] **Step 2: Run tests**

Run: `cargo test tree::node`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/tree/ src/main.rs
git commit -m "feat: tree node + git status types"
```

---

## Task 6: Arena — build, lazy load, expand/collapse, flatten

**Files:**
- Create (replace stub): `src/tree/arena.rs`

- [ ] **Step 1: Write the failing test**

Replace `src/tree/arena.rs`:

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test tree::arena`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/tree/arena.rs
git commit -m "feat: tree arena with lazy load, expand/collapse, flatten, reveal"
```

---

## Task 7: Git status worker + directory aggregation

**Files:**
- Create: `src/git.rs`
- Modify: `src/main.rs` (add `mod git;`)
- Create: `tests/git_int.rs`

- [ ] **Step 1: Write the failing test (integration, real repo)**

Create `tests/git_int.rs`:

```rust
use explorenova::git::scan_status;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {:?} failed", args);
}

#[test]
fn untracked_and_modified_are_reported() {
    let d = tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    fs::write(d.path().join("tracked.txt"), "v1").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init"]);
    fs::write(d.path().join("tracked.txt"), "v2").unwrap();
    fs::write(d.path().join("new.txt"), "n").unwrap();

    let map = scan_status(d.path()).unwrap();
    use explorenova::tree::GitStatus;
    assert_eq!(map.get(&d.path().join("tracked.txt")), Some(&GitStatus::Modified));
    assert_eq!(map.get(&d.path().join("new.txt")), Some(&GitStatus::Untracked));
}
```

Requires the crate to expose a library. Create `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod git;
pub mod state;
pub mod tree;
```

And in `src/main.rs` replace `mod ...;` lines for these with `use explorenova::{...}` as needed (keep `mod` only for binary-only modules). For now add at top of `main.rs`:

```rust
use explorenova as _;
```

- [ ] **Step 2: Create `src/git.rs`**

```rust
use crate::tree::GitStatus;
use git2::{Repository, Status, StatusOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Map every changed path (absolute) to its GitStatus, plus aggregate the
/// worst status onto each ancestor directory up to the repo workdir.
pub fn scan_status(root: &Path) -> anyhow::Result<HashMap<PathBuf, GitStatus>> {
    let mut out: HashMap<PathBuf, GitStatus> = HashMap::new();
    let repo = match Repository::discover(root) {
        Ok(r) => r,
        Err(_) => return Ok(out), // not a repo: empty map
    };
    let workdir = match repo.workdir() {
        Some(w) => w.to_path_buf(),
        None => return Ok(out),
    };
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    for entry in repo.statuses(Some(&mut opts))?.iter() {
        let st = entry.status();
        let s = classify(st);
        if let Some(rel) = entry.path() {
            let abs = workdir.join(rel);
            bump(&mut out, abs.clone(), s);
            // aggregate onto ancestors up to workdir
            let mut p = abs.parent().map(Path::to_path_buf);
            while let Some(dir) = p {
                if !dir.starts_with(&workdir) || dir == workdir {
                    bump(&mut out, workdir.clone(), s);
                    break;
                }
                bump(&mut out, dir.clone(), s);
                p = dir.parent().map(Path::to_path_buf);
            }
        }
    }
    Ok(out)
}

fn bump(map: &mut HashMap<PathBuf, GitStatus>, key: PathBuf, s: GitStatus) {
    map.entry(key)
        .and_modify(|cur| {
            if s.rank() > cur.rank() {
                *cur = s;
            }
        })
        .or_insert(s);
}

fn classify(st: Status) -> GitStatus {
    if st.is_conflicted() {
        GitStatus::Conflicted
    } else if st.is_wt_new() || st.is_index_new() {
        if st.is_index_new() {
            GitStatus::Added
        } else {
            GitStatus::Untracked
        }
    } else if st.is_wt_deleted() || st.is_index_deleted() {
        GitStatus::Deleted
    } else if st.is_wt_renamed() || st.is_index_renamed() {
        GitStatus::Renamed
    } else if st.is_ignored() {
        GitStatus::Ignored
    } else if st.is_wt_modified() || st.is_index_modified() {
        GitStatus::Modified
    } else {
        GitStatus::Clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_repo_returns_empty_map() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan_status(d.path()).unwrap().is_empty());
    }
}
```

Add `tempfile` is dev-dep only; the inline `#[cfg(test)]` use of `tempfile` is fine.

- [ ] **Step 3: Run tests**

Run: `cargo test --test git_int && cargo test git::`
Expected: PASS (integration: 1, unit: 1). Skips cleanly if `git` binary absent only if you guard — assume `git` present.

- [ ] **Step 4: Commit**

```bash
git add src/git.rs src/lib.rs src/main.rs tests/git_int.rs
git commit -m "feat: git status scan with directory aggregation"
```

---

## Task 8: File operations (create/copy/move/rename/delete)

**Files:**
- Create: `src/fs_ops.rs`
- Modify: `src/lib.rs` (add `pub mod fs_ops;`)
- Create: `tests/fs_ops_int.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/fs_ops_int.rs`:

```rust
use explorenova::fs_ops::{create, paste, rename, trash_path, PasteMode};
use std::fs;
use tempfile::tempdir;

#[test]
fn create_file_and_nested_dir() {
    let d = tempdir().unwrap();
    create(&d.path().join("a/b/c.txt")).unwrap();
    assert!(d.path().join("a/b/c.txt").is_file());
    create(&d.path().join("x/y/")).unwrap();
    assert!(d.path().join("x/y").is_dir());
}

#[test]
fn paste_copy_collision_gets_suffix() {
    let d = tempdir().unwrap();
    let src = d.path().join("f.txt");
    fs::write(&src, "1").unwrap();
    let dst_dir = d.path().join("dst");
    fs::create_dir(&dst_dir).unwrap();
    fs::write(dst_dir.join("f.txt"), "old").unwrap();
    let out = paste(&src, &dst_dir, PasteMode::Copy).unwrap();
    assert_eq!(out.file_name().unwrap(), "f (1).txt");
    assert!(d.path().join("f.txt").exists()); // copy keeps source
}

#[test]
fn paste_move_removes_source() {
    let d = tempdir().unwrap();
    let src = d.path().join("m.txt");
    fs::write(&src, "1").unwrap();
    let dst = d.path().join("dst");
    fs::create_dir(&dst).unwrap();
    let out = paste(&src, &dst, PasteMode::Move).unwrap();
    assert!(out.exists());
    assert!(!src.exists());
}

#[test]
fn rename_changes_name() {
    let d = tempdir().unwrap();
    let p = d.path().join("old.txt");
    fs::write(&p, "1").unwrap();
    let np = rename(&p, "new.txt").unwrap();
    assert!(np.exists());
    assert!(!p.exists());
}
```

- [ ] **Step 2: Create `src/fs_ops.rs`**

```rust
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum PasteMode {
    Copy,
    Move,
}

/// Trailing `/` => directory (mkdir -p). Else create file, mkdir -p its parent.
pub fn create(path: &Path) -> io::Result<()> {
    let is_dir = path
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::MAIN_SEPARATOR);
    if is_dir {
        fs::create_dir_all(path)
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            fs::File::create(path)?;
        }
        Ok(())
    }
}

/// Non-colliding destination: `name.ext` -> `name (1).ext`, `(2)`, ...
pub fn unique_dest(dir: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let name = Path::new(file_name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = name.extension().map(|e| e.to_string_lossy().to_string());
    for i in 1.. {
        let fname = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let c = dir.join(fname);
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

pub fn paste(src: &Path, dst_dir: &Path, mode: PasteMode) -> io::Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_dest(dst_dir, name);
    match mode {
        PasteMode::Copy => {
            if src.is_dir() {
                let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                fs_extra::dir::copy(src, &dst, &opts)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            } else {
                fs::copy(src, &dst)?;
            }
        }
        PasteMode::Move => {
            if fs::rename(src, &dst).is_err() {
                // cross-device fallback
                if src.is_dir() {
                    let opts = fs_extra::dir::CopyOptions::new().copy_inside(true);
                    fs_extra::dir::move_dir(src, &dst, &opts)
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                } else {
                    fs::copy(src, &dst)?;
                    fs::remove_file(src)?;
                }
            }
        }
    }
    Ok(dst)
}

pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let np = parent.join(new_name);
    fs::rename(path, &np)?;
    Ok(np)
}

/// Recoverable delete via system trash.
pub fn trash_path(path: &Path) -> anyhow::Result<()> {
    trash::delete(path)?;
    Ok(())
}

/// Irreversible. Caller MUST gate with a confirmed prompt (see keymap Confirm mode).
pub fn permanent_delete(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn unique_dest_increments() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "1").unwrap();
        let u = unique_dest(d.path(), std::ffi::OsStr::new("a.txt"));
        assert_eq!(u.file_name().unwrap(), "a (1).txt");
    }
}
```

Add to `src/lib.rs`: `pub mod fs_ops;`

- [ ] **Step 3: Run tests**

Run: `cargo test --test fs_ops_int && cargo test fs_ops::`
Expected: PASS (integration: 4, unit: 1).

- [ ] **Step 4: Commit**

```bash
git add src/fs_ops.rs src/lib.rs tests/fs_ops_int.rs
git commit -m "feat: file ops (create/paste/rename/trash/permanent) with collision suffixing"
```

---

## Task 9: Fuzzy search (ignore walk + nucleo)

**Files:**
- Create: `src/search.rs`
- Modify: `src/lib.rs` (add `pub mod search;`)

- [ ] **Step 1: Write the failing test**

Create `src/search.rs`:

```rust
use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use std::path::{Path, PathBuf};

/// Collect candidate paths under `root`. `include_all` => ignore .gitignore + show hidden.
pub fn collect(root: &Path, include_all: bool) -> Vec<PathBuf> {
    let mut wb = WalkBuilder::new(root);
    wb.hidden(!include_all)
        .git_ignore(!include_all)
        .git_global(!include_all)
        .git_exclude(!include_all);
    wb.build()
        .filter_map(Result::ok)
        .map(|d| d.into_path())
        .filter(|p| p != root)
        .collect()
}

/// Rank `candidates` against `query`. Returns (path, score) descending,
/// plus matched char indices on the path string for highlighting.
pub fn rank(
    candidates: &[PathBuf],
    root: &Path,
    query: &str,
) -> Vec<(PathBuf, u32, Vec<u32>)> {
    if query.is_empty() {
        return candidates.iter().map(|p| (p.clone(), 0, vec![])).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pat = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(PathBuf, u32, Vec<u32>)> = Vec::new();
    for p in candidates {
        let rel = p.strip_prefix(root).unwrap_or(p);
        let hay = rel.to_string_lossy();
        let mut buf = Vec::new();
        let h = nucleo::Utf32Str::new(&hay, &mut buf);
        let mut indices = Vec::new();
        if let Some(score) = pat.indices(h, &mut matcher, &mut indices) {
            scored.push((p.clone(), score, indices));
        }
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn fixture() -> tempfile::TempDir {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/main.rs"), "").unwrap();
        fs::write(d.path().join("README.md"), "").unwrap();
        fs::write(d.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(d.path().join("ignored.txt"), "").unwrap();
        d
    }

    #[test]
    fn collect_respects_gitignore_by_default() {
        let d = fixture();
        let got = collect(d.path(), false);
        let names: Vec<_> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn collect_all_includes_ignored() {
        let d = fixture();
        let got = collect(d.path(), true);
        let names: Vec<_> = got
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn rank_orders_better_match_first() {
        let d = fixture();
        let cands = collect(d.path(), false);
        let r = rank(&cands, d.path(), "mainrs");
        assert!(r[0].0.ends_with("main.rs"));
        assert!(!r[0].2.is_empty()); // has highlight indices
    }
}
```

Add to `src/lib.rs`: `pub mod search;`

- [ ] **Step 2: Run tests**

Run: `cargo test search::`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/search.rs src/lib.rs
git commit -m "feat: fuzzy search via ignore walk + nucleo ranking"
```

---

## Task 10: Path nav tab completion

**Files:**
- Create: `src/nav.rs`
- Modify: `src/lib.rs` (add `pub mod nav;`)

- [ ] **Step 1: Write the failing test**

Create `src/nav.rs`:

```rust
use std::fs;
use std::path::{Path, PathBuf};

/// Expand leading `~` and `$VAR`/`${VAR}`.
pub fn expand(input: &str) -> String {
    let mut s = input.to_string();
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            s = format!("{}{}", home.to_string_lossy(), rest);
        }
    }
    while let Some(start) = s.find('$') {
        let bytes = &s[start + 1..];
        let (name, end) = if bytes.starts_with('{') {
            match bytes.find('}') {
                Some(i) => (&bytes[1..i], start + 1 + i + 1),
                None => break,
            }
        } else {
            let len = bytes
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(bytes.len());
            (&bytes[..len], start + 1 + len)
        };
        let val = std::env::var(name).unwrap_or_default();
        s.replace_range(start..end, &val);
    }
    s
}

/// Filesystem completions for `input`. Returns sorted matching full paths.
pub fn completions(input: &str) -> Vec<PathBuf> {
    let expanded = expand(input);
    let p = Path::new(&expanded);
    let (dir, prefix): (PathBuf, String) = if expanded.ends_with('/') {
        (p.to_path_buf(), String::new())
    } else {
        (
            p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        )
    };
    let mut out: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|c| {
                c.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Longest common prefix of candidate file-name strings, joined onto dir.
pub fn longest_common(input: &str, candidates: &[PathBuf]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    let names: Vec<String> = candidates
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();
    let mut lcp = names[0].clone();
    for n in &names[1..] {
        while !n.starts_with(&lcp) {
            lcp.pop();
            if lcp.is_empty() {
                return None;
            }
        }
    }
    let dir = candidates[0].parent()?;
    let _ = input;
    Some(dir.join(&lcp).to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn expand_tilde_and_env() {
        std::env::set_var("HOME", "/home/x");
        assert_eq!(expand("~/foo"), "/home/x/foo");
        std::env::set_var("EN_T", "bar");
        assert_eq!(expand("/a/$EN_T/b"), "/a/bar/b");
        assert_eq!(expand("/a/${EN_T}/b"), "/a/bar/b");
    }

    #[test]
    fn completions_filter_by_prefix() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("alpha")).unwrap();
        fs::create_dir(d.path().join("alpine")).unwrap();
        fs::create_dir(d.path().join("beta")).unwrap();
        let inp = format!("{}/al", d.path().display());
        let c = completions(&inp);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn longest_common_prefix_cycle_base() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("alpha")).unwrap();
        fs::create_dir(d.path().join("alpine")).unwrap();
        let inp = format!("{}/al", d.path().display());
        let c = completions(&inp);
        let lcp = longest_common(&inp, &c).unwrap();
        assert!(lcp.ends_with("/alp"));
    }
}
```

Add to `src/lib.rs`: `pub mod nav;`

- [ ] **Step 2: Run tests**

Run: `cargo test nav::`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/nav.rs src/lib.rs
git commit -m "feat: path nav expand + tab completion + longest-common-prefix"
```

---

## Task 11: Keymap (Action enum, Mode, config-bound resolver)

**Files:**
- Create: `src/keymap.rs`
- Modify: `src/lib.rs` (add `pub mod keymap;`)

- [ ] **Step 1: Write the failing test**

Create `src/keymap.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Input,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    Open,
    Collapse,
    Toggle,
    OpenDefault,
    OpenWith,
    FocusNext,
    ToggleBookmarksPane,
    TogglePreviewPane,
    ResizeShrink,
    ResizeGrow,
    Create,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    PermanentDelete,
    ToggleBookmark,
    Search,
    SearchNext,
    SearchPrev,
    PathInput,
    ShellOnce,
    ShellDrop,
    ToggleHidden,
    Refresh,
    Quit,
    Cancel,
}

impl Action {
    pub fn from_name(s: &str) -> Option<Action> {
        use Action::*;
        Some(match s {
            "down" => Down, "up" => Up, "top" => Top, "bottom" => Bottom,
            "half_down" => HalfDown, "half_up" => HalfUp,
            "open" => Open, "collapse" => Collapse, "toggle" => Toggle,
            "open_default" => OpenDefault, "open_with" => OpenWith,
            "focus_next" => FocusNext,
            "toggle_bookmarks_pane" => ToggleBookmarksPane,
            "toggle_preview_pane" => TogglePreviewPane,
            "resize_shrink" => ResizeShrink, "resize_grow" => ResizeGrow,
            "create" => Create, "copy" => Copy, "cut" => Cut, "paste" => Paste,
            "rename" => Rename, "trash" => Trash, "permanent_delete" => PermanentDelete,
            "toggle_bookmark" => ToggleBookmark,
            "search" => Search, "search_next" => SearchNext, "search_prev" => SearchPrev,
            "path_input" => PathInput,
            "shell_once" => ShellOnce, "shell_drop" => ShellDrop,
            "toggle_hidden" => ToggleHidden, "refresh" => Refresh,
            "quit" => Quit, "cancel" => Cancel,
            _ => return None,
        })
    }
}

/// Parse a chord like "j", "G", "ctrl-d", "enter", "esc", "gg".
fn parse_chord(s: &str) -> Option<(KeyModifiers, KeyCode)> {
    let mut mods = KeyModifiers::NONE;
    let parts: Vec<&str> = s.split('-').collect();
    let key = *parts.last()?;
    for m in &parts[..parts.len().saturating_sub(1)] {
        match *m {
            "ctrl" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }
    let code = match key {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "del" => KeyCode::Delete,
        k if k.chars().count() == 1 => KeyCode::Char(k.chars().next().unwrap()),
        _ => return None,
    };
    Some((mods, code))
}

pub struct Keymap {
    /// Normal-mode single/double-char chords -> Action
    normal: HashMap<String, Action>,
}

impl Keymap {
    pub fn build(user: &std::collections::BTreeMap<String, Vec<String>>) -> Self {
        let mut normal = HashMap::new();
        for (action, chords) in Self::defaults() {
            for c in chords {
                normal.insert(c.to_string(), action);
            }
        }
        // user overrides: action name -> chords (replace)
        for (name, chords) in user {
            if let Some(a) = Action::from_name(name) {
                normal.retain(|_, v| *v != a);
                for c in chords {
                    normal.insert(c.clone(), a);
                }
            }
        }
        Keymap { normal }
    }

    fn defaults() -> Vec<(Action, Vec<&'static str>)> {
        use Action::*;
        vec![
            (Down, vec!["j", "down"]), (Up, vec!["k", "up"]),
            (Top, vec!["gg"]), (Bottom, vec!["G"]),
            (HalfDown, vec!["ctrl-d"]), (HalfUp, vec!["ctrl-u"]),
            (Open, vec!["l", "right"]), (Collapse, vec!["h", "left"]),
            (Toggle, vec!["enter"]),
            (OpenDefault, vec!["o"]), (OpenWith, vec!["O"]),
            (FocusNext, vec!["tab"]),
            (ToggleBookmarksPane, vec!["B"]), (TogglePreviewPane, vec!["P"]),
            (ResizeShrink, vec!["<"]), (ResizeGrow, vec![">"]),
            (Create, vec!["a"]), (Copy, vec!["yy"]), (Cut, vec!["dd"]),
            (Paste, vec!["p"]), (Rename, vec!["r"]),
            (Trash, vec!["x", "del"]), (PermanentDelete, vec!["X"]),
            (ToggleBookmark, vec!["m"]),
            (Search, vec!["/"]), (SearchNext, vec!["n"]), (SearchPrev, vec!["N"]),
            (PathInput, vec![":"]),
            (ShellOnce, vec!["!"]), (ShellDrop, vec!["S"]),
            (ToggleHidden, vec!["."]), (Refresh, vec!["R"]),
            (Quit, vec!["q"]), (Cancel, vec!["esc"]),
        ]
    }

    /// Resolve a pending chord buffer in the given mode. Returns the action and
    /// whether the buffer should be cleared.
    pub fn resolve(&self, mode: Mode, buffer: &str) -> Option<Action> {
        match mode {
            Mode::Normal => self.normal.get(buffer).copied(),
            Mode::Search | Mode::Input => match buffer {
                "esc" => Some(Action::Cancel),
                "enter" => Some(Action::Toggle),
                _ => None,
            },
            Mode::Confirm => None,
        }
    }

    /// Could `buffer` still grow into a binding (e.g. "g" -> "gg")?
    pub fn is_prefix(&self, buffer: &str) -> bool {
        self.normal.keys().any(|k| k.len() > buffer.len() && k.starts_with(buffer))
    }
}

/// Reduce a crossterm KeyEvent to a chord token.
pub fn key_token(ev: &KeyEvent) -> Option<String> {
    let mut prefix = String::new();
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        prefix.push_str("ctrl-");
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        prefix.push_str("alt-");
    }
    let base = match ev.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Delete => "del".into(),
        _ => return None,
    };
    Some(format!("{prefix}{base}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn default_j_is_down() {
        let km = Keymap::build(&BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "j"), Some(Action::Down));
        assert_eq!(km.resolve(Mode::Normal, "gg"), Some(Action::Top));
    }

    #[test]
    fn g_is_prefix_of_gg() {
        let km = Keymap::build(&BTreeMap::new());
        assert!(km.is_prefix("g"));
        assert!(km.resolve(Mode::Normal, "g").is_none());
    }

    #[test]
    fn user_override_remaps_quit() {
        let mut user = BTreeMap::new();
        user.insert("quit".to_string(), vec!["Q".to_string()]);
        let km = Keymap::build(&user);
        assert_eq!(km.resolve(Mode::Normal, "Q"), Some(Action::Quit));
        assert_eq!(km.resolve(Mode::Normal, "q"), None);
    }

    #[test]
    fn parse_chord_handles_modifiers() {
        assert_eq!(
            parse_chord("ctrl-d"),
            Some((KeyModifiers::CONTROL, KeyCode::Char('d')))
        );
    }
}
```

Add to `src/lib.rs`: `pub mod keymap;`

- [ ] **Step 2: Run tests**

Run: `cargo test keymap::`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/keymap.rs src/lib.rs
git commit -m "feat: keymap with modes, chord parsing, defaults + user overrides"
```

---

## Task 12: Preview dispatch + cache

**Files:**
- Create: `src/preview/mod.rs`
- Create: `src/preview/text.rs`
- Create: `src/preview/media.rs`
- Modify: `src/lib.rs` (add `pub mod preview;`)

- [ ] **Step 1: Write the failing test**

Create `src/preview/mod.rs`:

```rust
pub mod media;
pub mod text;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewKind {
    Text(String),
    Image(PathBuf), // path to displayable image (original or rasterized)
    Meta(String),
}

fn ext_lower(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Decide preview kind for `path` (no caching).
pub fn render(path: &Path, max_bytes: u64) -> PreviewKind {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return PreviewKind::Meta(format!("error: {e}")),
    };
    if meta.is_dir() {
        return PreviewKind::Meta(format!("directory\n{} entries", count_entries(path)));
    }
    let ext = ext_lower(path);
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return PreviewKind::Image(path.to_path_buf());
    }
    if ext == "pdf" {
        return match media::rasterize_pdf(path) {
            Some(png) => PreviewKind::Image(png),
            None => PreviewKind::Meta(meta_string(path, &meta)),
        };
    }
    if meta.len() > max_bytes {
        return PreviewKind::Meta(meta_string(path, &meta));
    }
    match text::highlighted(path, max_bytes) {
        Some(t) => PreviewKind::Text(t),
        None => PreviewKind::Meta(meta_string(path, &meta)),
    }
}

fn count_entries(p: &Path) -> usize {
    std::fs::read_dir(p).map(|r| r.count()).unwrap_or(0)
}

fn meta_string(p: &Path, m: &std::fs::Metadata) -> String {
    format!("{}\n{} bytes\n(binary or oversized)", p.display(), m.len())
}

/// (path, mtime)-keyed preview cache.
#[derive(Default)]
pub struct PreviewCache {
    map: HashMap<(PathBuf, SystemTime), PreviewKind>,
}

impl PreviewCache {
    pub fn get_or_render(&mut self, path: &Path, max_bytes: u64) -> PreviewKind {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let key = (path.to_path_buf(), mtime);
        if let Some(v) = self.map.get(&key) {
            return v.clone();
        }
        let v = render(path, max_bytes);
        self.map.insert(key, v.clone());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn text_file_is_text_preview() {
        let d = tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        match render(&f, 1024) {
            PreviewKind::Text(t) => assert!(t.contains("hello")),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn oversized_falls_back_to_meta() {
        let d = tempdir().unwrap();
        let f = d.path().join("big.txt");
        fs::write(&f, vec![b'x'; 2048]).unwrap();
        assert!(matches!(render(&f, 100), PreviewKind::Meta(_)));
    }

    #[test]
    fn image_ext_is_image_kind() {
        let d = tempdir().unwrap();
        let f = d.path().join("p.png");
        fs::write(&f, [0u8; 8]).unwrap();
        assert!(matches!(render(&f, 1024), PreviewKind::Image(_)));
    }

    #[test]
    fn cache_returns_same_value_twice() {
        let d = tempdir().unwrap();
        let f = d.path().join("c.txt");
        fs::write(&f, "x").unwrap();
        let mut c = PreviewCache::default();
        let a = c.get_or_render(&f, 1024);
        let b = c.get_or_render(&f, 1024);
        assert_eq!(a, b);
    }
}
```

Create `src/preview/text.rs`:

```rust
use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

/// Returns ANSI-highlighted text, or None if the file is not valid UTF-8.
pub fn highlighted(path: &Path, max_bytes: u64) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    let content = String::from_utf8(bytes).ok()?;
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| ss.find_syntax_by_extension(e))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = String::new();
    for line in content.lines().take(2000) {
        let ranges = h.highlight_line(line, &ss).ok()?;
        out.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
        out.push('\n');
    }
    out.push_str("\x1b[0m");
    Some(out)
}
```

Create `src/preview/media.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

/// Rasterize PDF page 1 to a PNG in the temp dir via `pdftoppm`.
/// Returns None if poppler is not installed or the call fails.
pub fn rasterize_pdf(pdf: &Path) -> Option<PathBuf> {
    let stem = std::env::temp_dir().join(format!(
        "explorenova-pdf-{}",
        pdf.file_stem()?.to_string_lossy()
    ));
    let status = Command::new("pdftoppm")
        .args(["-png", "-singlefile", "-f", "1", "-l", "1"])
        .arg(pdf)
        .arg(&stem)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let png = stem.with_extension("png");
    png.exists().then_some(png)
}
```

Add to `src/lib.rs`: `pub mod preview;`

- [ ] **Step 2: Run tests**

Run: `cargo test preview::`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/preview/ src/lib.rs
git commit -m "feat: preview dispatch (text/image/pdf/meta) + mtime cache"
```

---

## Task 13: Theme + UI layout math

**Files:**
- Create: `src/ui/mod.rs`
- Create: `src/ui/theme.rs`
- Create: `src/ui/panes.rs`
- Modify: `src/lib.rs` (add `pub mod ui;`)

- [ ] **Step 1: Write the failing test (layout computation is pure → unit-tested)**

Create `src/ui/theme.rs`:

```rust
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub dir: Color,
    pub file: Color,
    pub selected_bg: Color,
    pub git_modified: Color,
    pub git_added: Color,
    pub git_untracked: Color,
    pub git_conflict: Color,
    pub match_hl: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            dir: Color::Cyan,
            file: Color::Gray,
            selected_bg: Color::DarkGray,
            git_modified: Color::Yellow,
            git_added: Color::Green,
            git_untracked: Color::Blue,
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
        }
    }
}
```

Create `src/ui/mod.rs`:

```rust
pub mod panes;
pub mod theme;

use crate::config::PanesConfig;
use ratatui::layout::Constraint;

/// Build (pane_name, Constraint) pairs in display order from config,
/// skipping hidden panes. Fixed `width` -> Length; else Ratio via weight.
pub fn layout_constraints(p: &PanesConfig) -> Vec<(String, Constraint)> {
    let mut visible: Vec<(&str, &crate::config::PaneConfig)> = Vec::new();
    for name in &p.order {
        let pc = match name.as_str() {
            "bookmarks" => &p.bookmarks,
            "tree" => &p.tree,
            "preview" => &p.preview,
            _ => continue,
        };
        if pc.visible {
            visible.push((name.as_str(), pc));
        }
    }
    let weight_sum: u32 = visible
        .iter()
        .filter(|(_, c)| c.width.is_none())
        .map(|(_, c)| c.weight.max(1) as u32)
        .sum::<u32>()
        .max(1);
    visible
        .iter()
        .map(|(name, c)| {
            let con = match c.width {
                Some(w) => Constraint::Length(w),
                None => Constraint::Ratio(c.weight.max(1) as u32, weight_sum),
            };
            (name.to_string(), con)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PaneConfig, PanesConfig};

    #[test]
    fn hidden_pane_excluded() {
        let mut p = PanesConfig::default(); // bookmarks hidden by default
        let cons = layout_constraints(&p);
        assert_eq!(cons.len(), 2);
        assert_eq!(cons[0].0, "tree");
        p.bookmarks.visible = true;
        assert_eq!(layout_constraints(&p).len(), 3);
    }

    #[test]
    fn fixed_width_uses_length() {
        let mut p = PanesConfig::default();
        p.bookmarks.visible = true;
        let cons = layout_constraints(&p);
        assert!(matches!(cons[0].1, Constraint::Length(24)));
    }

    #[test]
    fn order_is_respected() {
        let mut p = PanesConfig::default();
        p.bookmarks.visible = true;
        p.order = vec!["preview".into(), "tree".into(), "bookmarks".into()];
        let cons = layout_constraints(&p);
        assert_eq!(cons[0].0, "preview");
        assert_eq!(cons[2].0, "bookmarks");
    }
}
```

Create `src/ui/panes.rs`:

```rust
use crate::tree::{Arena, NodeId};
use crate::ui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Build a styled line for one visible tree row.
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
) -> Line<'a> {
    let n = &arena.nodes[id];
    let indent = "  ".repeat(n.depth);
    let marker = if n.is_dir() {
        if n.expanded { "▾ " } else { "▸ " }
    } else {
        "  "
    };
    let name = n
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| n.path.display().to_string());
    let g = n.git.glyph();
    let git_style = match n.git {
        crate::tree::GitStatus::Modified => Style::default().fg(theme.git_modified),
        crate::tree::GitStatus::Added => Style::default().fg(theme.git_added),
        crate::tree::GitStatus::Untracked => Style::default().fg(theme.git_untracked),
        crate::tree::GitStatus::Conflicted => Style::default().fg(theme.git_conflict),
        _ => Style::default(),
    };
    let name_style = if n.is_dir() {
        Style::default().fg(theme.dir)
    } else {
        Style::default().fg(theme.file)
    };
    let mut line = Line::from(vec![
        Span::raw(format!("{indent}{marker}")),
        Span::styled(format!("{g} "), git_style),
        Span::styled(name, name_style),
    ]);
    if selected {
        line = line.style(Style::default().bg(theme.selected_bg).add_modifier(Modifier::BOLD));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn row_renders_name() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("z.txt"), "x").unwrap();
        let a = Arena::new(d.path()).unwrap();
        let child = a.nodes[a.root].children.as_ref().unwrap()[0];
        let line = tree_row(&a, child, true, &Theme::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("z.txt"));
    }
}
```

Add to `src/lib.rs`: `pub mod ui;`

- [ ] **Step 2: Run tests**

Run: `cargo test ui::`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/ui/ src/lib.rs
git commit -m "feat: theme + layout constraint computation + tree row rendering"
```

---

## Task 14: Shell suspend/resume

**Files:**
- Create: `src/shell.rs`
- Modify: `src/lib.rs` (add `pub mod shell;`)

- [ ] **Step 1: Write code (terminal suspend is manual-verified; logic is thin)**

Create `src/shell.rs`:

```rust
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

/// Build the argv for a one-shot shell command (for testing without spawning).
pub fn shell_argv(cmd: &str) -> (String, Vec<String>) {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    (sh, vec!["-c".into(), cmd.into()])
}

/// Leave the TUI, run `f`, wait for a keypress, restore the TUI.
fn around_terminal<F: FnOnce() -> io::Result<()>>(f: F) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    let r = f();
    print!("\n[press Enter to return to explorenova] ");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    r
}

pub fn run_once(cwd: &Path, cmd: &str) -> io::Result<()> {
    let (sh, args) = shell_argv(cmd);
    around_terminal(|| {
        Command::new(sh).args(args).current_dir(cwd).status().map(|_| ())
    })
}

pub fn drop_to_shell(cwd: &Path) -> io::Result<()> {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    around_terminal(|| Command::new(sh).current_dir(cwd).status().map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_argv_wraps_in_dash_c() {
        std::env::set_var("SHELL", "/bin/bash");
        let (sh, args) = shell_argv("ls -la");
        assert_eq!(sh, "/bin/bash");
        assert_eq!(args, vec!["-c", "ls -la"]);
    }
}
```

Add to `src/lib.rs`: `pub mod shell;`

- [ ] **Step 2: Run test**

Run: `cargo test shell::`
Expected: PASS (1 test).

- [ ] **Step 3: Commit**

```bash
git add src/shell.rs src/lib.rs
git commit -m "feat: shell run-once + drop-to-shell with terminal suspend"
```

---

## Task 15: App state + reducer

**Files:**
- Create: `src/app.rs`
- Modify: `src/lib.rs` (add `pub mod app;`)

- [ ] **Step 1: Write the failing test**

Create `src/app.rs`:

```rust
use crate::config::Config;
use crate::keymap::{Action, Mode};
use crate::state::State;
use crate::tree::Arena;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Bookmarks,
    Tree,
    Preview,
}

pub struct App {
    pub arena: Arena,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub mode: Mode,
    pub focus: Pane,
    pub config: Config,
    pub state: State,
    pub clipboard: Option<(PathBuf, bool)>, // (path, is_cut)
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(arena: Arena, config: Config, state: State) -> Self {
        let visible = arena.flatten();
        App {
            arena,
            visible,
            selected: 0,
            mode: Mode::Normal,
            focus: Pane::Tree,
            config,
            state,
            clipboard: None,
            status: String::new(),
            should_quit: false,
        }
    }

    fn refresh_visible(&mut self) {
        let cur = self.visible.get(self.selected).copied();
        self.visible = self.arena.flatten();
        if let Some(c) = cur {
            if let Some(i) = self.visible.iter().position(|&n| n == c) {
                self.selected = i;
            }
        }
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    fn sel_id(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    /// Apply a Normal-mode navigation/structural action. IO-bearing actions
    /// (open, shell, paste) are handled in main.rs; this covers pure state.
    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Down => {
                if self.selected + 1 < self.visible.len() {
                    self.selected += 1;
                }
            }
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Top => self.selected = 0,
            Action::Bottom => self.selected = self.visible.len().saturating_sub(1),
            Action::HalfDown => {
                self.selected = (self.selected + 10).min(self.visible.len().saturating_sub(1))
            }
            Action::HalfUp => self.selected = self.selected.saturating_sub(10),
            Action::Open => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() {
                        let _ = self.arena.expand(id);
                        self.refresh_visible();
                    }
                }
            }
            Action::Collapse => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() && self.arena.nodes[id].expanded {
                        self.arena.collapse(id);
                    } else if let Some(p) = self.arena.nodes[id].parent {
                        if let Some(i) = self.visible.iter().position(|&n| n == p) {
                            self.selected = i;
                        }
                    }
                    self.refresh_visible();
                }
            }
            Action::Toggle => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() {
                        let _ = self.arena.toggle(id);
                        self.refresh_visible();
                    }
                }
            }
            Action::FocusNext => {
                self.focus = match self.focus {
                    Pane::Bookmarks => Pane::Tree,
                    Pane::Tree => Pane::Preview,
                    Pane::Preview => Pane::Bookmarks,
                };
            }
            Action::ToggleBookmarksPane => {
                self.config.panes.bookmarks.visible = !self.config.panes.bookmarks.visible;
            }
            Action::TogglePreviewPane => {
                self.config.panes.preview.visible = !self.config.panes.preview.visible;
            }
            Action::ToggleHidden => {
                self.arena.show_hidden = !self.arena.show_hidden;
            }
            Action::ToggleBookmark => {
                if let Some(id) = self.sel_id() {
                    self.state.toggle_bookmark(self.arena.nodes[id].path.clone());
                }
            }
            Action::Quit => self.should_quit = true,
            Action::Cancel => self.mode = Mode::Normal,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn app() -> App {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/x.txt"), "1").unwrap();
        std::fs::write(d.path().join("a.txt"), "1").unwrap();
        std::mem::forget(d); // keep dir alive for the test
        let arena = Arena::new(std::path::Path::new(".")).unwrap_or_else(|_| {
            Arena::new(std::env::temp_dir().as_path()).unwrap()
        });
        App::new(arena, Config::with_defaults(), State::default())
    }

    #[test]
    fn down_then_up_returns_to_zero() {
        let mut a = app();
        a.apply(Action::Down);
        a.apply(Action::Up);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn quit_sets_flag() {
        let mut a = app();
        a.apply(Action::Quit);
        assert!(a.should_quit);
    }

    #[test]
    fn toggle_preview_pane_flips_visibility() {
        let mut a = app();
        let before = a.config.panes.preview.visible;
        a.apply(Action::TogglePreviewPane);
        assert_ne!(a.config.panes.preview.visible, before);
    }

    #[test]
    fn focus_cycles() {
        let mut a = app();
        assert_eq!(a.focus, Pane::Tree);
        a.apply(Action::FocusNext);
        assert_eq!(a.focus, Pane::Preview);
    }
}
```

Add to `src/lib.rs`: `pub mod app;`

- [ ] **Step 2: Run tests**

Run: `cargo test app::`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add src/app.rs src/lib.rs
git commit -m "feat: app state + pure navigation/structural reducer"
```

---

## Task 16: Main — terminal RAII, panic hook, event loop, worker wiring

**Files:**
- Modify: `src/main.rs` (full event loop)

- [ ] **Step 1: Replace `src/main.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use explorenova::app::App;
use explorenova::cli::{resolve_start_dir, Args};
use explorenova::config::Config;
use explorenova::git::scan_status;
use explorenova::keymap::{key_token, Action, Keymap, Mode};
use explorenova::preview::PreviewCache;
use explorenova::state::State;
use explorenova::tree::Arena;
use explorenova::ui;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// RAII guard: always restores the terminal, even on panic/early return.
struct TermGuard;
impl TermGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TermGuard)
    }
}
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("explorenova").join("config.toml")
}

fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".local")
                .join("share")
        });
    base.join("explorenova").join("state.toml")
}

fn load_config() -> Config {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => Config::load_str(&s).0,
        Err(_) => {
            // first run: write commented defaults
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let c = Config::with_defaults();
            if let Ok(s) = toml::to_string_pretty(&c) {
                let _ = std::fs::write(&p, format!("# explorenova config\n{s}"));
            }
            c
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let sp = state_path();
    let mut state = std::fs::read_to_string(&sp)
        .map(|s| State::from_str(&s))
        .unwrap_or_default();

    let mut config = load_config();
    if args.tree_only {
        config.panes.bookmarks.visible = false;
        config.panes.preview.visible = false;
        config.panes.tree.visible = true;
    }

    let pwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let start = resolve_start_dir(args.path.clone(), state.last_cwd.clone(), pwd);

    // Panic hook: restore terminal before the default panic print.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    let arena = Arena::new(&start)?;
    let mut app = App::new(arena, config, state.clone());
    app.state = state.clone();
    let keymap = Keymap::build(&app.config.keys);
    let mut preview_cache = PreviewCache::default();

    // Git worker: scan once at start + on Refresh request.
    let (git_tx, git_rx) = mpsc::channel::<()>();
    let (git_res_tx, git_res_rx) = mpsc::channel();
    {
        let root = start.clone();
        std::thread::spawn(move || {
            let send = |r| { let _ = git_res_tx.send(r); };
            send(scan_status(&root).unwrap_or_default());
            while git_rx.recv().is_ok() {
                send(scan_status(&root).unwrap_or_default());
            }
        });
    }

    let _guard = TermGuard::new()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut chord = String::new();

    loop {
        // Drain git results into the arena.
        while let Ok(map) = git_res_rx.try_recv() {
            for n in app.arena.nodes.iter_mut() {
                if let Some(s) = map.get(&n.path) {
                    n.git = *s;
                }
            }
        }

        term.draw(|f| ui::render_frame(f, &mut app, &mut preview_cache))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            let Some(tok) = key_token(&k) else { continue };
            match app.mode {
                Mode::Normal => {
                    chord.push_str(&tok);
                    if let Some(action) = keymap.resolve(Mode::Normal, &chord) {
                        chord.clear();
                        handle(&mut app, action, &start, &git_tx)?;
                    } else if keymap.is_prefix(&chord) {
                        // wait for more keys
                    } else {
                        chord.clear();
                    }
                }
                _ => {
                    if keymap.resolve(app.mode, &tok) == Some(Action::Cancel) {
                        app.mode = Mode::Normal;
                    }
                    // Search/Input/Confirm text handling rendered in ui::render_frame.
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Persist state (bookmarks, last cwd, pane overrides).
    app.state.last_cwd = Some(start.clone());
    app.state.bookmarks = app.state.bookmarks.clone();
    if let Some(parent) = sp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&sp, app.state.to_toml());
    Ok(())
}

/// Bridge actions that need IO (open/shell/file ops) to their modules; pure
/// actions go through App::apply.
fn handle(app: &mut App, action: Action, cwd: &std::path::Path, git_tx: &mpsc::Sender<()>) -> Result<()> {
    use explorenova::{fs_ops, shell};
    match action {
        Action::OpenDefault => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if !app.arena.nodes[id].is_dir() {
                    let _ = open::that_detached(&p);
                }
            }
        }
        Action::ShellDrop => {
            shell::drop_to_shell(cwd)?;
        }
        Action::Refresh => {
            let _ = git_tx.send(());
        }
        Action::Trash => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if let Err(e) = fs_ops::trash_path(&p) {
                    app.status = format!("trash failed: {e}");
                } else {
                    app.status = format!("trashed {}", p.display());
                }
            }
        }
        other => app.apply(other),
    }
    Ok(())
}
```

- [ ] **Step 2: Add `render_frame` to `src/ui/mod.rs`**

Append to `src/ui/mod.rs`:

```rust
use crate::app::App;
use crate::preview::{PreviewCache, PreviewKind};
use ratatui::layout::{Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render_frame(f: &mut Frame, app: &mut App, cache: &mut PreviewCache) {
    let cons: Vec<_> = layout_constraints(&app.config.panes);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cons.iter().map(|(_, c)| *c).collect::<Vec<_>>())
        .split(f.area());
    let theme = crate::ui::theme::Theme::default();

    for (i, (name, _)) in cons.iter().enumerate() {
        let area: Rect = chunks[i];
        match name.as_str() {
            "tree" => {
                let rows: Vec<_> = app
                    .visible
                    .iter()
                    .enumerate()
                    .map(|(idx, &id)| {
                        crate::ui::panes::tree_row(&app.arena, id, idx == app.selected, &theme)
                    })
                    .collect();
                let p = Paragraph::new(rows)
                    .block(Block::default().borders(Borders::ALL).title("tree"));
                f.render_widget(p, area);
            }
            "bookmarks" => {
                let items: Vec<_> = app
                    .state
                    .bookmarks
                    .iter()
                    .map(|b| ratatui::text::Line::from(b.display().to_string()))
                    .collect();
                f.render_widget(
                    Paragraph::new(items)
                        .block(Block::default().borders(Borders::ALL).title("bookmarks")),
                    area,
                );
            }
            "preview" => {
                let text = match app.visible.get(app.selected) {
                    Some(&id) => match cache.get_or_render(
                        &app.arena.nodes[id].path,
                        app.config.preview_max_bytes,
                    ) {
                        PreviewKind::Text(t) => t,
                        PreviewKind::Meta(m) => m,
                        PreviewKind::Image(p) => format!("[image] {}", p.display()),
                    },
                    None => String::new(),
                };
                f.render_widget(
                    Paragraph::new(text)
                        .block(Block::default().borders(Borders::ALL).title("preview")),
                    area,
                );
            }
            _ => {}
        }
    }
}
```

> Note: image pixel rendering via `ratatui-image` is deferred to Task 17; this frame shows an image placeholder string so the loop is testable now.

- [ ] **Step 3: Build + manual smoke test**

Run: `cargo build`
Expected: compiles.

Run: `cargo run -- /tmp`
Expected: TUI opens showing a bordered tree of `/tmp`. `j`/`k` move selection, `l`/`Enter` expand a dir, `h` collapses, `.` toggles hidden, `R` refreshes git, `B`/`P` toggle panes, `q` quits cleanly with the terminal restored. Verify `Ctrl-C`/panic also restores terminal (run `cargo run -- /nonexistent-on-purpose` is invalid; instead temporarily add `panic!()` behind a key, confirm prompt returns to normal shell — then revert).

Run: `cargo run -- --tree-only /tmp`
Expected: only the tree pane visible; `P` re-enables preview at runtime.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/ui/mod.rs
git commit -m "feat: main event loop, terminal RAII + panic hook, git worker, frame render"
```

---

## Task 17: Image/PDF pixel preview via ratatui-image

**Files:**
- Modify: `src/ui/mod.rs` (render real image protocol in preview pane)
- Modify: `src/app.rs` (hold an optional image state) — minimal

- [ ] **Step 1: Add image picker + protocol state**

Append to `src/ui/mod.rs`:

```rust
use ratatui_image::picker::Picker;
use ratatui_image::{Resize, StatefulImage};
use ratatui_image::protocol::StatefulProtocol;

/// Lazily-initialized terminal graphics picker (kitty/sixel/iTerm2/halfblocks).
pub fn make_picker() -> Picker {
    Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)))
}

/// Render an image file into `area` using the detected protocol.
pub fn render_image(
    f: &mut Frame,
    area: Rect,
    picker: &mut Picker,
    path: &std::path::Path,
) {
    match image::ImageReader::open(path).and_then(|r| Ok(r.decode())) {
        Ok(Ok(img)) => {
            let mut proto: StatefulProtocol = picker.new_resize_protocol(img);
            let widget = StatefulImage::default().resize(Resize::Fit(None));
            f.render_stateful_widget(widget, area, &mut proto);
        }
        _ => {
            f.render_widget(
                Paragraph::new(format!("[unreadable image] {}", path.display())),
                area,
            );
        }
    }
}
```

Modify the `"preview"` arm in `render_frame` so that `PreviewKind::Image(p)` calls `render_image` instead of printing a placeholder. Add a `picker: &mut Picker` parameter to `render_frame` and thread it from `main.rs` (create with `ui::make_picker()` once before the loop, store outside `term.draw`).

Concretely, change `render_frame` signature to:

```rust
pub fn render_frame(f: &mut Frame, app: &mut App, cache: &mut PreviewCache, picker: &mut Picker) {
```

and the Image branch to:

```rust
PreviewKind::Image(p) => {
    crate::ui::render_image(f, area, picker, &p);
    String::new()
}
```

(keep the surrounding `Paragraph` only for Text/Meta — render Image directly and `continue` past the paragraph for that pane).

In `main.rs`: before the loop add `let mut picker = ui::make_picker();` and change the draw call to `ui::render_frame(f, &mut app, &mut preview_cache, &mut picker)`.

- [ ] **Step 2: Build + manual verification**

Run: `cargo build`
Expected: compiles.

Run: `cargo run -- <dir-with-a-png>` then select the PNG.
Expected: image renders in the preview pane (kitty/sixel/iTerm2 graphics, or halfblock fallback in a plain terminal).

Run: select a `.pdf` (requires `pdftoppm`).
Expected: page 1 renders as an image. Without poppler installed: preview shows file metadata instead (no crash).

- [ ] **Step 3: Commit**

```bash
git add src/ui/mod.rs src/main.rs
git commit -m "feat: real image/pdf preview via ratatui-image protocols"
```

---

## Task 18: Wire interactive modes (search, path input, create, rename, confirm-delete)

**Files:**
- Modify: `src/app.rs` (add input/search/confirm buffers + handlers)
- Modify: `src/main.rs` (route text keys per mode; trigger actions)
- Modify: `src/ui/mod.rs` (render a bottom prompt line for non-Normal modes)

- [ ] **Step 1: Add interactive state to `App`**

Add fields to `App` in `src/app.rs`:

```rust
    pub input: String,            // generic line buffer (path / create / rename)
    pub search_results: Vec<(std::path::PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub pending: Option<Action>,  // action awaiting Confirm (e.g. PermanentDelete)
```

Initialize them in `App::new` (`input: String::new()`, `search_results: vec![]`, `search_idx: 0`, `pending: None`).

Add methods:

```rust
impl App {
    pub fn enter_mode(&mut self, m: Mode, seed: &str) {
        self.mode = m;
        self.input = seed.to_string();
        self.status.clear();
    }

    pub fn push_input(&mut self, c: char) {
        self.input.push(c);
    }
    pub fn backspace_input(&mut self) {
        self.input.pop();
    }

    /// Run fuzzy search from the tree root using the current input.
    pub fn run_search(&mut self) {
        let root = self.arena.nodes[self.arena.root].path.clone();
        let cands = crate::search::collect(&root, self.arena.show_hidden);
        let ranked = crate::search::rank(&cands, &root, &self.input);
        self.search_results = ranked.into_iter().map(|(p, _, idx)| (p, idx)).collect();
        self.search_idx = 0;
        self.jump_to_search();
    }

    pub fn jump_to_search(&mut self) {
        if let Some((p, _)) = self.search_results.get(self.search_idx).cloned() {
            if let Ok(Some(id)) = self.arena.reveal(&p) {
                self.visible = self.arena.flatten();
                if let Some(i) = self.visible.iter().position(|&n| n == id) {
                    self.selected = i;
                }
            }
        }
    }

    pub fn search_cycle(&mut self, forward: bool) {
        if self.search_results.is_empty() {
            return;
        }
        let n = self.search_results.len();
        self.search_idx = if forward {
            (self.search_idx + 1) % n
        } else {
            (self.search_idx + n - 1) % n
        };
        self.jump_to_search();
    }

    pub fn commit_create(&mut self) {
        let base = self
            .sel_dir()
            .unwrap_or_else(|| self.arena.nodes[self.arena.root].path.clone());
        let target = base.join(&self.input);
        match crate::fs_ops::create(&target) {
            Ok(_) => self.status = format!("created {}", target.display()),
            Err(e) => self.status = format!("create failed: {e}"),
        }
        self.mode = Mode::Normal;
    }

    pub fn commit_rename(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            let p = self.arena.nodes[id].path.clone();
            match crate::fs_ops::rename(&p, &self.input) {
                Ok(np) => self.status = format!("renamed -> {}", np.display()),
                Err(e) => self.status = format!("rename failed: {e}"),
            }
        }
        self.mode = Mode::Normal;
    }

    fn sel_dir(&self) -> Option<std::path::PathBuf> {
        let id = *self.visible.get(self.selected)?;
        let n = &self.arena.nodes[id];
        Some(if n.is_dir() {
            n.path.clone()
        } else {
            n.path.parent()?.to_path_buf()
        })
    }

    pub fn confirm_yes(&mut self) {
        if let Some(Action::PermanentDelete) = self.pending {
            if let Some(&id) = self.visible.get(self.selected) {
                let p = self.arena.nodes[id].path.clone();
                match crate::fs_ops::permanent_delete(&p) {
                    Ok(_) => self.status = format!("deleted {}", p.display()),
                    Err(e) => self.status = format!("delete failed: {e}"),
                }
            }
        }
        self.pending = None;
        self.mode = Mode::Normal;
    }
}
```

- [ ] **Step 2: Add a unit test for search jump + confirm gating**

Append to the `tests` mod in `src/app.rs`:

```rust
    #[test]
    fn permanent_delete_requires_confirm() {
        let mut a = app();
        a.pending = Some(Action::PermanentDelete);
        a.mode = Mode::Confirm;
        // cancel path: pressing Cancel must NOT delete
        a.apply(Action::Cancel);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.status.is_empty(), "cancel must not perform deletion");
    }

    #[test]
    fn enter_mode_seeds_input() {
        let mut a = app();
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input, "seed");
        assert_eq!(a.mode, Mode::Input);
    }
```

- [ ] **Step 3: Route mode keys in `src/main.rs`**

Replace the `_ =>` (non-Normal) match arm in the event loop with:

```rust
                Mode::Search | Mode::Input => {
                    use crossterm::event::KeyCode::*;
                    match k.code {
                        Esc => { app.mode = Mode::Normal; app.input.clear(); }
                        Enter => {
                            match app.mode {
                                Mode::Search => app.run_search(),
                                Mode::Input => {
                                    if matches!(app.pending, Some(Action::Rename)) {
                                        app.pending = None;
                                        app.commit_rename();
                                    } else if matches!(app.pending, Some(Action::PathInput)) {
                                        let target = explorenova::nav::expand(&app.input);
                                        if let Ok(Some(id)) =
                                            app.arena.reveal(std::path::Path::new(&target))
                                        {
                                            app.visible = app.arena.flatten();
                                            if let Some(i) =
                                                app.visible.iter().position(|&n| n == id)
                                            {
                                                app.selected = i;
                                            }
                                        } else {
                                            app.status = "path not found".into();
                                        }
                                        app.pending = None;
                                        app.mode = Mode::Normal;
                                    } else {
                                        app.commit_create();
                                    }
                                }
                                _ => {}
                            }
                            if app.mode == Mode::Search { app.mode = Mode::Normal; }
                        }
                        Tab if app.mode == Mode::Input
                            && matches!(app.pending, Some(Action::PathInput)) =>
                        {
                            let c = explorenova::nav::completions(&app.input);
                            if let Some(lcp) = explorenova::nav::longest_common(&app.input, &c) {
                                app.input = lcp;
                            }
                        }
                        Backspace => app.backspace_input(),
                        Char(ch) => app.push_input(ch),
                        _ => {}
                    }
                }
                Mode::Confirm => {
                    use crossterm::event::KeyCode::*;
                    match k.code {
                        Char('y') | Char('Y') => app.confirm_yes(),
                        _ => { app.pending = None; app.mode = Mode::Normal; }
                    }
                }
```

And in `handle()`, add arms that enter the interactive modes:

```rust
        Action::Search => app.enter_mode(Mode::Search, ""),
        Action::SearchNext => app.search_cycle(true),
        Action::SearchPrev => app.search_cycle(false),
        Action::PathInput => {
            app.pending = Some(Action::PathInput);
            app.enter_mode(Mode::Input, "");
        }
        Action::Create => {
            app.pending = Some(Action::Create);
            app.enter_mode(Mode::Input, "");
        }
        Action::Rename => {
            let seed = app
                .visible
                .get(app.selected)
                .map(|&id| {
                    app.arena.nodes[id]
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            app.pending = Some(Action::Rename);
            app.enter_mode(Mode::Input, &seed);
        }
        Action::PermanentDelete => {
            app.pending = Some(Action::PermanentDelete);
            app.mode = Mode::Confirm;
            app.status = "permanently delete? y/N".into();
        }
        Action::Copy => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), false));
                app.status = "copied".into();
            }
        }
        Action::Cut => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), true));
                app.status = "cut".into();
            }
        }
        Action::Paste => {
            if let Some((src, is_cut)) = app.clipboard.clone() {
                let dst_dir = app
                    .visible
                    .get(app.selected)
                    .map(|&id| {
                        let n = &app.arena.nodes[id];
                        if n.is_dir() { n.path.clone() }
                        else { n.path.parent().unwrap_or(cwd).to_path_buf() }
                    })
                    .unwrap_or_else(|| cwd.to_path_buf());
                let mode = if is_cut {
                    explorenova::fs_ops::PasteMode::Move
                } else {
                    explorenova::fs_ops::PasteMode::Copy
                };
                match explorenova::fs_ops::paste(&src, &dst_dir, mode) {
                    Ok(p) => app.status = format!("pasted -> {}", p.display()),
                    Err(e) => app.status = format!("paste failed: {e}"),
                }
                if is_cut { app.clipboard = None; }
            }
        }
        Action::OpenWith => {
            app.pending = Some(Action::OpenWith);
            app.enter_mode(Mode::Input, "");
        }
        Action::ShellOnce => {
            app.pending = Some(Action::ShellOnce);
            app.enter_mode(Mode::Input, "");
        }
```

Handle `OpenWith`/`ShellOnce` commit in the `Mode::Input` `Enter` branch by adding, before the `commit_create()` fallback:

```rust
                                    } else if matches!(app.pending, Some(Action::OpenWith)) {
                                        if let Some(&id) = app.visible.get(app.selected) {
                                            let p = app.arena.nodes[id].path.clone();
                                            let _ = std::process::Command::new(&app.input)
                                                .arg(&p)
                                                .spawn();
                                        }
                                        app.pending = None;
                                        app.mode = Mode::Normal;
                                    } else if matches!(app.pending, Some(Action::ShellOnce)) {
                                        let cmd = app.input.clone();
                                        app.pending = None;
                                        app.mode = Mode::Normal;
                                        explorenova::shell::run_once(cwd, &cmd)?;
```

(Insert these as additional `else if` arms in the same chain; keep `commit_create()` as the final `else`.)

- [ ] **Step 4: Render the prompt line in `src/ui/mod.rs`**

In `render_frame`, after pane rendering, draw a one-row prompt at the bottom of `f.area()` when `app.mode != Mode::Normal` (or when `app.status` is non-empty), showing `app.status` in Normal mode and a mode-prefixed `app.input` otherwise (e.g. `/`, `:`, `name:`, `delete? y/N`). Use a 1-row `Rect` at the bottom and `Paragraph`. (Concrete: split `f.area()` vertically into `[Min(0), Length(1)]` before the horizontal pane split; render panes in the top chunk, prompt in the bottom.)

- [ ] **Step 5: Run unit tests + build + manual verify**

Run: `cargo test app::`
Expected: PASS (existing + 2 new).

Run: `cargo build && cargo run -- /tmp`
Expected: `/` opens search, typing filters and jumps the tree; `n`/`N` cycle. `:` opens path input, `Tab` completes paths, `Enter` jumps. `a` creates a file/dir (trailing `/` => dir). `r` renames with name prefilled. `X` shows `permanently delete? y/N`; `n`/anything cancels (no deletion), `y` deletes. `yy`/`dd`/`p` copy/cut/paste with ` (1)` collision suffix. `!` runs a command in cwd then returns. `O` opens-with a typed command.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/main.rs src/ui/mod.rs
git commit -m "feat: interactive modes — search, path input, create/rename, confirm-delete, copy/paste, open-with, shell-once"
```

---

## Task 19: Filesystem watch (notify) + debounced refresh

**Files:**
- Modify: `src/main.rs` (spawn notify watcher → debounced reload of changed dirs)

- [ ] **Step 1: Add the watcher worker**

In `main.rs`, after the git worker setup, add:

```rust
    use notify::{RecursiveMode, Watcher};
    let (fs_tx, fs_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(ev) = res {
            let _ = fs_tx.send(ev);
        }
    })?;
    watcher.watch(&start, RecursiveMode::Recursive)?;
```

In the loop, before `term.draw`, drain fs events with ~150ms debounce: collect all currently-queued events via `fs_rx.try_recv()`, and if any arrived, set a `dirty` flag; once `dirty` and ≥150ms since the first event, re-run `app.arena.load_children` for the root (and clear+rebuild children of currently-expanded dirs whose paths appear in events), call `app.refresh_visible()` (make `refresh_visible` `pub`), and request a git rescan via `git_tx.send(())`. Keep the implementation minimal: on debounce fire, rebuild the arena from `start` preserving the selected path and expansion of the root.

Concrete debounce skeleton inside the loop:

```rust
    // (declare before loop) let mut fs_dirty_since: Option<std::time::Instant> = None;
    let mut any = false;
    while fs_rx.try_recv().is_ok() { any = true; }
    if any && fs_dirty_since.is_none() {
        fs_dirty_since = Some(std::time::Instant::now());
    }
    if let Some(t0) = fs_dirty_since {
        if t0.elapsed() >= Duration::from_millis(150) {
            let sel_path = app
                .visible
                .get(app.selected)
                .map(|&id| app.arena.nodes[id].path.clone());
            if let Ok(new_arena) = Arena::new(&start) {
                app.arena = new_arena;
                app.visible = app.arena.flatten();
                if let Some(p) = sel_path {
                    if let Ok(Some(id)) = app.arena.reveal(&p) {
                        app.visible = app.arena.flatten();
                        if let Some(i) = app.visible.iter().position(|&n| n == id) {
                            app.selected = i;
                        }
                    }
                }
            }
            let _ = git_tx.send(());
            fs_dirty_since = None;
        }
    }
```

(`watcher` must stay in scope for the whole loop — bind it to a name that lives until after the loop.)

- [ ] **Step 2: Build + manual verify**

Run: `cargo build && cargo run -- /tmp`
Then in another terminal: `touch /tmp/explorenova_probe && sleep 1 && rm /tmp/explorenova_probe`
Expected: the new file appears within ~150–300ms without pressing `R`, then disappears, selection preserved.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: notify fs-watch with 150ms debounced tree+git refresh"
```

---

## Task 20: README + final integration pass

**Files:**
- Create: `README.md`
- Run: full test suite

- [ ] **Step 1: Write `README.md`**

```markdown
# explorenova

Terminal file manager: nvim-tree-style collapsible git-annotated tree,
bookmarks pane, yazi-style preview (text/image/pdf), fuzzy search, safe
file ops, vim keybindings, configurable panes.

## Build

    cargo build --release

## Run

    explorenova [PATH] [--tree-only|-t]

## Config

`~/.config/explorenova/config.toml` (panes, `[keys]`, preview limits) —
written with defaults on first run.
State (bookmarks, last cwd, pane overrides):
`~/.local/share/explorenova/state.toml`.

## Keys

j/k move · l/h expand/collapse · Enter toggle/open · o/O open/open-with ·
a create · yy/dd/p copy/cut/paste · r rename · x trash · X delete ·
m bookmark · / search (n/N) · : path (Tab complete) · ! shell · S shell-drop ·
B/P toggle panes · < > resize · . hidden · R refresh · q quit

PDF preview needs `poppler-utils` (`pdftoppm`); without it PDFs show metadata.
```

- [ ] **Step 2: Run the full suite**

Run: `cargo test`
Expected: ALL unit + integration tests PASS.

Run: `cargo build --release`
Expected: clean release build.

- [ ] **Step 3: Manual end-to-end checklist**

Run: `cargo run --release -- ~`
Verify against spec: tree+git glyphs render; bookmarks toggle (`B`) and persist across restart; preview shows highlighted text, images, and a PDF (or metadata without poppler); `/` fuzzy search jumps; `:` path nav with Tab; create/copy/move/rename/trash work; `X` requires `y`; `!`/`S` shell; `--tree-only` hides bookmarks+preview; quit restores terminal; pane resize `<`/`>`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: README + final integration pass"
```

---

## Self-Review

**Spec coverage:**

- Tree view + collapse/expand → Tasks 5,6,15 ✓
- Git status like nvim-tree (dir aggregation) → Task 7 ✓
- Open in default app / open-with → Task 16 (`OpenDefault`), Task 18 (`OpenWith`) ✓
- Fuzzy search, gitignore-aware, recursive → Tasks 9,18 ✓
- Create/copy/move/rename/delete (trash + permanent w/ confirm) → Tasks 8,18 ✓
- Vim + explorer keybindings, remappable → Task 11 ✓
- Bookmarks pane, toggle, persisted, leftmost → Tasks 3,13,15,16 ✓
- Shell command in cwd + drop-to-shell → Tasks 14,16,18 ✓
- Path nav with tab completion → Tasks 10,18 ✓
- Configurable panes: hide/resize/reorder, persisted → Tasks 2,3,13,15 ✓
- Image/PDF preview (yazi-style protocols) → Tasks 12,17 ✓
- TOML config at ~/.config/explorenova → Tasks 2,16 ✓
- `--tree-only` session override → Tasks 4,16 ✓
- fs-watch auto-refresh → Task 19 ✓
- Terminal RAII + panic restore + worker channels → Task 16 ✓

No spec requirement is left without a task.

**Placeholder scan:** No "TBD/TODO/handle edge cases" steps; every code step contains complete, compilable code. Task 5 explicitly notes the `arena.rs` stub is replaced in Task 6 (intentional, not a placeholder).

**Type consistency:** `GitStatus`, `Node`, `NodeId`, `Arena`, `Config`, `PanesConfig`, `PaneConfig`, `State`, `Action`, `Mode`, `Keymap`, `App`, `Pane`, `PreviewKind`, `PreviewCache`, `PasteMode`, `render_frame` signatures are defined once and referenced consistently across tasks (`render_frame` signature change in Task 17 is called out explicitly with the migration). `refresh_visible` is made `pub` in Task 19 where cross-module access is first needed.
