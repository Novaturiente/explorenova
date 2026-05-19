# Parent Navigation + Yazi-style Icons & Styling — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add parent-directory navigation (`-` / `h` at root) and a yazi-style look: Nerd-Font filetype icons, richer type colours, full-row selection bar, path header + status footer, rounded borders, dimmed secondary text.

**Architecture:** One new pure module (`icons`), additive fields on `Config`/`Node`, a keymap action, theme colours, a `handle()` ascend path, and a render refactor of the tree pane to `List`+`ListState`. Each task keeps `cargo build` + the full suite green at commit.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, serde, tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-05-19-parent-nav-and-yazi-styling-design.md`
**Base:** branch `master`, post-commit `47fe393`.

---

## File Structure

```
src/icons.rs       NEW  Kind enum, classify(), icon_for() — pure, tested
src/lib.rs              + pub mod icons;
src/config.rs           + pub icons: bool (serde default true) + with_defaults
src/tree/node.rs        + pub exec: bool on Node (+ Node::new)
src/tree/arena.rs       classify() now returns (NodeKind,bool exec); load_children sets it
src/keymap.rs           + Action::Parent, from_name "parent", default "-"
src/ui/theme.rs         + executable/symlink/media/archive/selection_bg/header/dim
src/main.rs             ascend() helper; handle(): Action::Parent + Collapse-at-root
src/ui/panes.rs         tree_row: icon + type colour + dim, no per-row bg, +icons param
src/ui/mod.rs           tree pane -> List+ListState; [header,body,footer] layout; rounded
```

---

## Task 1: `icons` module (pure)

**Files:** Create `src/icons.rs`; modify `src/lib.rs`.

- [ ] **Step 1: Write `src/icons.rs`**

```rust
//! Filetype classification + Nerd Font glyphs for the tree.

use crate::tree::NodeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Symlink,
    Exec,
    Media,
    Archive,
    File,
}

const MEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "mp4", "mkv", "mov",
    "mp3", "flac", "wav",
];
const ARCHIVE: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar"];

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Classify a node for colour + icon (single source of truth).
pub fn classify(kind: NodeKind, exec: bool, name: &str) -> Kind {
    match kind {
        NodeKind::Dir => Kind::Dir,
        NodeKind::Symlink => Kind::Symlink,
        _ => {
            if exec {
                return Kind::Exec;
            }
            let e = ext(name);
            if MEDIA.contains(&e.as_str()) {
                Kind::Media
            } else if ARCHIVE.contains(&e.as_str()) {
                Kind::Archive
            } else {
                Kind::File
            }
        }
    }
}

/// Nerd Font glyph for a node.
pub fn icon_for(kind: NodeKind, expanded: bool, name: &str) -> &'static str {
    match kind {
        NodeKind::Dir => {
            if expanded {
                "\u{f07c}"
            } else {
                "\u{f07b}"
            }
        }
        NodeKind::Symlink => "\u{f481}",
        _ => match ext(name).as_str() {
            "rs" => "\u{e7a8}",
            "py" => "\u{e606}",
            "js" | "mjs" | "cjs" => "\u{e74e}",
            "ts" => "\u{e628}",
            "tsx" | "jsx" => "\u{e7ba}",
            "go" => "\u{e627}",
            "c" | "h" => "\u{e61e}",
            "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
            "rb" => "\u{e21e}",
            "java" => "\u{e738}",
            "kt" | "kts" => "\u{e634}",
            "swift" => "\u{e755}",
            "php" => "\u{e73d}",
            "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
            "lua" => "\u{e620}",
            "vim" => "\u{e62b}",
            "el" => "\u{e632}",
            "ex" | "exs" => "\u{e62d}",
            "hs" => "\u{e777}",
            "ml" | "mli" => "\u{e67a}",
            "nim" => "\u{e677}",
            "zig" => "\u{e6a9}",
            "dart" => "\u{e798}",
            "md" | "markdown" => "\u{f48a}",
            "txt" | "text" => "\u{f15c}",
            "json" => "\u{e60b}",
            "toml" => "\u{e6b2}",
            "yaml" | "yml" => "\u{e615}",
            "lock" => "\u{f023}",
            "xml" => "\u{f72d}",
            "html" | "htm" => "\u{f13b}",
            "css" => "\u{e749}",
            "scss" | "sass" => "\u{e749}",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => "\u{f1c5}",
            "svg" => "\u{f1c5}",
            "pdf" => "\u{f1c1}",
            "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" => {
                "\u{f410}"
            }
            "mp3" | "flac" | "wav" => "\u{f001}",
            "mp4" | "mkv" | "mov" => "\u{f03d}",
            "csv" => "\u{f1c3}",
            "sql" => "\u{f1c0}",
            "db" | "sqlite" => "\u{f1c0}",
            "" => "\u{f016}",
            _ => "\u{f15b}",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_categories() {
        assert_eq!(classify(NodeKind::Dir, false, "src"), Kind::Dir);
        assert_eq!(classify(NodeKind::Symlink, false, "lnk"), Kind::Symlink);
        assert_eq!(classify(NodeKind::File, true, "run"), Kind::Exec);
        assert_eq!(classify(NodeKind::File, false, "a.PNG"), Kind::Media);
        assert_eq!(classify(NodeKind::File, false, "a.zip"), Kind::Archive);
        assert_eq!(classify(NodeKind::File, false, "a.rs"), Kind::File);
    }

    #[test]
    fn dir_icon_changes_with_expanded() {
        let c = icon_for(NodeKind::Dir, false, "x");
        let o = icon_for(NodeKind::Dir, true, "x");
        assert_ne!(c, o);
        assert!(!c.is_empty() && !o.is_empty());
    }

    #[test]
    fn symlink_has_glyph() {
        assert_eq!(icon_for(NodeKind::Symlink, false, "l"), "\u{f481}");
    }

    #[test]
    fn known_ext_differs_from_default_and_is_case_insensitive() {
        let def = icon_for(NodeKind::File, false, "x.unknownext");
        let rs = icon_for(NodeKind::File, false, "main.rs");
        assert_ne!(rs, def);
        assert_eq!(
            icon_for(NodeKind::File, false, "M.RS"),
            icon_for(NodeKind::File, false, "m.rs")
        );
    }
}
```

- [ ] **Step 2: Declare module** — in `src/lib.rs`, add `pub mod icons;` between `pub mod git;` and `pub mod keymap;` (alphabetical).

- [ ] **Step 3: Verify**

Run: `cd /home/nova/personal/explorer && cargo test --lib icons:: 2>&1 | tail -3` → `4 passed; 0 failed`.
Run: `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean.
Run: `cargo test --lib 2>&1 | grep "test result:"` → expect `67 passed` (63 prior + 4), 0 failed.
Run: `cargo test --test apps_int 2>&1 | grep "test result:"` (1) ; `--test fs_ops_int` (4) ; `--test git_int` (1).

- [ ] **Step 4: Commit**

```bash
git add src/icons.rs src/lib.rs
git commit -m "feat: icons module — filetype classify + Nerd Font glyphs"
```

---

## Task 2: `Config.icons` + `Node.exec` + arena exec bit

**Files:** Modify `src/config.rs`, `src/tree/node.rs`, `src/tree/arena.rs`.

- [ ] **Step 1: `src/config.rs` — add the field + default fn**

Find:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub panes: PanesConfig,
    /// action name -> list of key chords
    pub keys: BTreeMap<String, Vec<String>>,
    pub preview_max_bytes: u64,
}
```
Replace:
```rust
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub panes: PanesConfig,
    /// action name -> list of key chords
    pub keys: BTreeMap<String, Vec<String>>,
    pub preview_max_bytes: u64,
    /// Show Nerd Font filetype icons in the tree.
    #[serde(default = "default_true")]
    pub icons: bool,
}
```

Find:
```rust
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c
    }
```
Replace:
```rust
    pub fn with_defaults() -> Self {
        let mut c = Config { preview_max_bytes: 1_048_576, ..Default::default() };
        c.panes = PanesConfig::default();
        c.icons = true;
        c
    }
```

- [ ] **Step 2: `src/config.rs` — tests**

Find the `#[cfg(test)] mod tests {` block in `src/config.rs`; add these two tests inside it (before its closing `}`):
```rust
    #[test]
    fn icons_defaults_true_via_with_defaults() {
        assert!(Config::with_defaults().icons);
    }

    #[test]
    fn icons_missing_in_toml_defaults_true_else_parses() {
        let (c, e) = Config::load_str("preview_max_bytes = 99\n");
        assert!(e.is_none());
        assert!(c.icons, "missing icons key must default true");
        let (c2, _) = Config::load_str("icons = false\n");
        assert!(!c2.icons);
    }
```

> If `Config::load_str` is not the loader name, read `src/config.rs` and use the actual public parse fn (it returns `(Config, Option<...>)`). Adjust the two calls accordingly; keep the assertions.

- [ ] **Step 3: `src/tree/node.rs` — add `exec`**

Find:
```rust
    pub git: GitStatus,
    pub depth: usize,
    pub parent: Option<NodeId>,
}
```
Replace:
```rust
    pub git: GitStatus,
    pub depth: usize,
    pub parent: Option<NodeId>,
    pub exec: bool,
}
```

Find (inside `Node::new`):
```rust
            git: GitStatus::Clean,
            depth,
            parent,
        }
```
Replace:
```rust
            git: GitStatus::Clean,
            depth,
            parent,
            exec: false,
        }
```

- [ ] **Step 4: `src/tree/arena.rs` — `classify` returns exec; set it**

Read `src/tree/arena.rs`. Replace the entire `fn classify(p: &Path) -> NodeKind { ... }` function (it currently does `fs::symlink_metadata` and returns a `NodeKind`) with:
```rust
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
```

Find (the single call site inside `load_children`):
```rust
            let kind = Self::classify(&p);
            let node = Node::new(p, kind, depth, Some(id));
```
Replace:
```rust
            let (kind, exec) = Self::classify(&p);
            let mut node = Node::new(p, kind, depth, Some(id));
            node.exec = exec;
```

> `Arena::new` does NOT call `classify` (it hardcodes the root as `NodeKind::Dir`), so this is the only call site. If a build error reports another `classify` caller, update it to destructure `(kind, _)`.

- [ ] **Step 5: `src/tree/arena.rs` — exec test**

Add to the `#[cfg(test)] mod tests` block in `src/tree/arena.rs` (before its closing `}`):
```rust
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
```

- [ ] **Step 6: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean.
Run: `cargo test --lib config:: 2>&1 | tail -3` (the 2 new pass) ; `cargo test --lib tree::arena 2>&1 | tail -3` (exec_bit_detected passes).
Run: `cargo test --lib 2>&1 | grep "test result:"` → expect `70 passed` (67 + 2 config + 1 arena), 0 failed.
Run: `cargo test --test apps_int 2>&1 | grep "test result:"` (1) ; `--test fs_ops_int` (4) ; `--test git_int` (1).

- [ ] **Step 7: Commit**

```bash
git add src/config.rs src/tree/node.rs src/tree/arena.rs
git commit -m "feat: Config.icons (default true) + Node.exec from mode bits"
```

---

## Task 3: `Action::Parent` keymap

**Files:** Modify `src/keymap.rs`.

- [ ] **Step 1: Add the variant** — Find:
```rust
    Open,
    Collapse,
    Toggle,
```
Replace:
```rust
    Open,
    Collapse,
    Parent,
    Toggle,
```

- [ ] **Step 2: `from_name`** — Find:
```rust
            "quit" => Quit, "cancel" => Cancel,
```
Replace:
```rust
            "parent" => Parent,
            "quit" => Quit, "cancel" => Cancel,
```

- [ ] **Step 3: default binding** — Find:
```rust
            (Open, vec!["l", "right"]), (Collapse, vec!["h", "left"]),
```
Replace:
```rust
            (Open, vec!["l", "right"]), (Collapse, vec!["h", "left"]),
            (Parent, vec!["-"]),
```

- [ ] **Step 4: test** — Add to the `#[cfg(test)] mod tests` block in `src/keymap.rs` (before its closing `}`):
```rust
    #[test]
    fn dash_resolves_to_parent() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "-"), Some(Action::Parent));
        assert_eq!(Action::from_name("parent"), Some(Action::Parent));
    }
```

> If `Keymap::build`/`resolve`/`from_name` signatures differ, read `src/keymap.rs` and match the existing test style in that module; keep the two assertions (dash→Parent, from_name("parent")→Parent).

- [ ] **Step 5: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean (adding an `Action` variant compiles: `from_name` is non-exhaustive, `App::apply` has `_ => {}`, `handle` has `other => app.apply(other)` — `Action::Parent` is a harmless no-op until Task 5).
Run: `cargo test --lib keymap:: 2>&1 | tail -3` (new test passes).
Run: `cargo test --lib 2>&1 | grep "test result:"` → `71 passed`, 0 failed. Integration: apps_int 1, fs_ops_int 4, git_int 1.

- [ ] **Step 6: Commit**

```bash
git add src/keymap.rs
git commit -m "feat: Action::Parent keymap (default '-')"
```

---

## Task 4: Theme colours

**Files:** Modify `src/ui/theme.rs`.

- [ ] **Step 1: Extend the struct** — Find:
```rust
    pub git_conflict: Color,
    pub match_hl: Color,
}
```
Replace:
```rust
    pub git_conflict: Color,
    pub match_hl: Color,
    pub executable: Color,
    pub symlink: Color,
    pub media: Color,
    pub archive: Color,
    pub selection_bg: Color,
    pub header: Color,
    pub dim: Color,
}
```

- [ ] **Step 2: Extend `Default`** — Find:
```rust
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
        }
```
Replace:
```rust
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
            executable: Color::Green,
            symlink: Color::Cyan,
            media: Color::Magenta,
            archive: Color::Red,
            selection_bg: Color::Rgb(45, 55, 72),
            header: Color::Rgb(130, 170, 255),
            dim: Color::DarkGray,
        }
```

- [ ] **Step 3: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean (new pub fields unused yet — `Theme` is in the lib crate, no dead-code lint).
Run: `cargo test --lib 2>&1 | grep "test result:"` → `71 passed`, 0 failed (no behaviour change). Integration unchanged (apps_int 1, fs_ops_int 4, git_int 1).

- [ ] **Step 4: Commit**

```bash
git add src/ui/theme.rs
git commit -m "feat: theme colours for exec/symlink/media/archive/selection/header/dim"
```

---

## Task 5: Parent-nav in `handle()`

**Files:** Modify `src/main.rs`.

- [ ] **Step 1: Add an `ascend` helper**

Find:
```rust
fn handle(
```
Replace:
```rust
/// Re-root the tree to the parent of the current root, selecting the
/// directory just left. No-op at the filesystem root. Filesystem IO, so
/// it lives here (not in the pure `App::apply`).
fn ascend(app: &mut App) {
    let root_path = app.arena.nodes[app.arena.root].path.clone();
    let parent = match root_path.parent() {
        Some(p) if p != root_path => p.to_path_buf(),
        _ => {
            app.status = "at filesystem root".into();
            return;
        }
    };
    let prev_hidden = app.arena.show_hidden;
    match Arena::new(&parent) {
        Ok(mut na) => {
            if prev_hidden {
                na.show_hidden = true;
                na.nodes[na.root].children = None;
                let _ = na.load_children(na.root);
                na.nodes[na.root].expanded = true;
            }
            app.arena = na;
            app.visible = app.arena.flatten();
            if let Ok(Some(id)) = app.arena.reveal(&root_path) {
                app.visible = app.arena.flatten();
                if let Some(i) = app.visible.iter().position(|&n| n == id) {
                    app.selected = i;
                }
            }
        }
        Err(_) => {
            app.status = format!("cannot open {}", parent.display());
        }
    }
}

fn handle(
```

- [ ] **Step 2: Wire `Parent` and root-`Collapse`**

Find:
```rust
        other => app.apply(other),
    }
    Ok(())
}
```
Replace:
```rust
        Action::Parent => ascend(app),
        Action::Collapse => {
            if app.visible.get(app.selected).copied() == Some(app.arena.root) {
                ascend(app);
            } else {
                app.apply(Action::Collapse);
            }
        }
        other => app.apply(other),
    }
    Ok(())
}
```

- [ ] **Step 3: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean.
Run: `cargo test --lib 2>&1 | grep "test result:"` → `71 passed`, 0 failed (ascend is event-loop IO glue, covered manually; `App::apply(Collapse)` path unchanged for non-root rows). Integration: apps_int 1, fs_ops_int 4, git_int 1.

- [ ] **Step 4: Manual smoke**

`cargo run -- /tmp/some/deep/dir` → `-` re-roots to parent, cursor on the dir just left; `h`/`←` on the root row ascends; `h`/`←` elsewhere still collapses; repeated ascend stops at `/` with status "at filesystem root".

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: parent navigation (- key, h/left at root) via arena re-root"
```

---

## Task 6: Render — icons, type colours, full-row bar, header/footer, rounded

**Files:** Modify `src/ui/panes.rs`, `src/ui/mod.rs`.

- [ ] **Step 1: Rewrite `tree_row`** — replace the entire `pub fn tree_row<'a>(...) -> Line<'a> { ... }` function (read it for the exact end) with:
```rust
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
) -> Line<'a> {
    let n = &arena.nodes[id];
    let indent = "  ".repeat(n.depth);
    let marker = if n.is_dir() {
        if n.expanded {
            "▾ "
        } else {
            "▸ "
        }
    } else {
        "  "
    };
    let name = n
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| n.path.display().to_string());

    let kind = crate::icons::classify(n.kind, n.exec, &name);
    let type_color = match kind {
        crate::icons::Kind::Dir => theme.dir,
        crate::icons::Kind::Symlink => theme.symlink,
        crate::icons::Kind::Exec => theme.executable,
        crate::icons::Kind::Media => theme.media,
        crate::icons::Kind::Archive => theme.archive,
        crate::icons::Kind::File => theme.file,
    };

    let g = n.git.glyph();
    let git_clean = matches!(n.git, crate::tree::GitStatus::Clean);
    let git_color = match n.git {
        crate::tree::GitStatus::Modified => theme.git_modified,
        crate::tree::GitStatus::Added => theme.git_added,
        crate::tree::GitStatus::Untracked => theme.git_untracked,
        crate::tree::GitStatus::Conflicted => theme.git_conflict,
        _ => theme.dim,
    };

    let dim_or = |c: ratatui::style::Color| {
        if selected {
            Style::default().fg(c)
        } else {
            Style::default().fg(theme.dim)
        }
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(indent));
    spans.push(Span::styled(marker.to_string(), dim_or(theme.dim)));
    if icons {
        let ic = crate::icons::icon_for(n.kind, n.expanded, &name);
        spans.push(Span::styled(
            format!("{ic} "),
            Style::default().fg(type_color),
        ));
    }
    let git_style = if git_clean && !selected {
        Style::default().fg(theme.dim)
    } else {
        Style::default().fg(git_color)
    };
    spans.push(Span::styled(format!("{g} "), git_style));
    spans.push(Span::styled(name, Style::default().fg(type_color)));
    Line::from(spans)
}
```

> Notes: the per-row `bg`/`BOLD` block is removed (the `List` `highlight_style` draws the full-width bar). `selected` now only controls whether secondary glyphs are dimmed. Update the existing `row_renders_name` test call in this file from `tree_row(&a, child, true, &Theme::default())` to `tree_row(&a, child, true, &Theme::default(), true)` so it still compiles and asserts the name is present.

- [ ] **Step 2: `src/ui/mod.rs` — `[header, body, footer]` layout**

Find:
```rust
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());
    let body = outer[0];
    let prompt_area = outer[1];
```
Replace:
```rust
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());
    let header_area = outer[0];
    let body = outer[1];
    let prompt_area = outer[2];
    {
        let root_path = app.arena.nodes[app.arena.root].path.display().to_string();
        f.render_widget(
            Paragraph::new(root_path).style(
                ratatui::style::Style::default()
                    .fg(crate::ui::theme::Theme::default().header)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            header_area,
        );
    }
```

- [ ] **Step 3: `src/ui/mod.rs` — tree pane to `List` + footer count + rounded**

Find:
```rust
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
```
Replace:
```rust
            "tree" => {
                let items: Vec<ratatui::widgets::ListItem> = app
                    .visible
                    .iter()
                    .enumerate()
                    .map(|(idx, &id)| {
                        ratatui::widgets::ListItem::new(crate::ui::panes::tree_row(
                            &app.arena,
                            id,
                            idx == app.selected,
                            &theme,
                            app.config.icons,
                        ))
                    })
                    .collect();
                let list = ratatui::widgets::List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .title("tree"),
                    )
                    .highlight_style(
                        ratatui::style::Style::default().bg(theme.selection_bg),
                    );
                let mut st = ratatui::widgets::ListState::default();
                st.select(Some(app.selected));
                f.render_stateful_widget(list, area, &mut st);
            }
```

- [ ] **Step 4: `src/ui/mod.rs` — footer shows count, rounded bookmarks/preview, rounded modal**

Find the status/footer render line (it renders `app.status` into `prompt_area`):
```rust
    f.render_widget(Paragraph::new(app.status.clone()), prompt_area);
```
Replace:
```rust
    let footer = format!(
        "{}/{}  {}",
        app.selected + 1,
        app.visible.len().max(1),
        app.status
    );
    f.render_widget(Paragraph::new(footer), prompt_area);
```

In the same file, every other `Block::default().borders(Borders::ALL)` (the `bookmarks` pane, the `preview` pane, and each modal `Block::default().borders(Borders::ALL).title(...)`) — append `.border_type(ratatui::widgets::BorderType::Rounded)` immediately after `.borders(Borders::ALL)`. Read the file and apply to every occurrence so the look is consistent. Do not change titles or other chaining.

- [ ] **Step 5: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` → clean. If `List`/`ListState`/`ListItem`/`BorderType::Rounded`/`render_stateful_widget` is not the ratatui 0.29 API, STOP and report BLOCKED with the verbatim error + ratatui version (do not substitute).
Run: `cargo test --lib 2>&1 | grep "test result:"` → `71 passed`, 0 failed (the updated `row_renders_name` test passes with the new 5-arg signature).
Run: `cargo test --test apps_int 2>&1 | grep "test result:"` (1) ; `--test fs_ops_int` (4) ; `--test git_int` (1).
Run: `cargo build --release 2>&1 | tail -1` → Finished release.

- [ ] **Step 6: Manual smoke**

`cargo run -- ~` (Nerd Font terminal): icons per filetype, dir glyph changes on expand, full-width selection bar follows the cursor, long tree scrolls (selected stays visible), header shows the root path, footer shows `i/N  msg`, rounded borders, dimmed markers on non-selected rows. `config.icons = false` → no icons, plain look.

- [ ] **Step 7: Commit**

```bash
git add src/ui/panes.rs src/ui/mod.rs
git commit -m "feat: yazi-style tree — icons, type colours, full-row bar, header/footer, rounded"
```

---

## Task 7: Integration + release

**Files:** none (verification only).

- [ ] **Step 1:** `cd /home/nova/personal/explorer && cargo test 2>&1 | grep "test result:"` — all green; report totals (expect lib 71, apps_int 1, fs_ops_int 4, git_int 1, 0 failed).
- [ ] **Step 2:** `cargo clippy 2>&1 | grep -cE "^error"` — must be `0`. List warnings; confirm none newly introduced by this feature (the pre-existing parse_chord/io::Error::other/from_str/doc-quote set is acceptable).
- [ ] **Step 3:** `cargo build --release 2>&1 | tail -1` — `Finished release`; `ls -la target/release/explorenova` exists.
- [ ] **Step 4: Manual e2e (human):** parent nav (`-`, `h` at root, `/` no-op, came-from selected); icons on/off via `~/.config/explorenova/config.toml` `icons`; full-row bar; header path; footer count; rounded borders; long-tree scroll; existing features (rename/search/open-with/preview/git) unaffected.
- [ ] **Step 5:** No code changes — nothing to commit if all green.

---

## Self-Review

**Spec coverage:**

- Parent nav: `-` from anywhere + `h`/`←` at root, came-from selected, `/` no-op, IO in handle() → Tasks 3 (key) + 5 (ascend/Collapse intercept) ✓
- `Node.exec` from Unix mode (no extra syscall — folded into `classify` which already did `symlink_metadata`) → Task 2 ✓
- icons module: `classify` + `icon_for`, curated ~70 map, dir collapsed/open, symlink, unknown→default, case-insensitive → Task 1 ✓
- `Config.icons` bool default true (with_defaults + serde field default) → Task 2 ✓
- Theme colours exec/symlink/media/archive/selection_bg/header/dim, fixed defaults → Task 4 ✓
- tree_row: icon (if cfg) + type colour from shared `classify` + git colour + dim secondary on non-selected, no per-row bg → Task 6 ✓
- Tree pane `List`+`ListState` full-width `highlight_style` bar + viewport scroll → Task 6 ✓
- Layout `[header, body, footer]`; header = root path; footer = `i/N  status`; modal still over `body` → Task 6 (header/footer); modal `centered_rect(..., body)` unchanged (body var preserved) ✓
- Rounded borders on panes + modal → Task 6 Step 4 ✓
- Error handling: ascend `/` no-op + status, `Arena::new` err → status, classify never panics, non-Unix exec=false → Tasks 1/2/5 ✓
- Tests: icons (classify/glyph/case), config (icons default/parse), arena (exec bit unix-gated), keymap (dash→Parent) → Tasks 1–3 ✓

No spec requirement is left without a task.

**Placeholder scan:** No TBD/"handle errors"/vague steps; every code step has full code or an exact anchored replacement. Three steps say "read the file to get the exact end/old text" for whole-function replacements (`classify`, `tree_row`) and the repeated rounded-border edit — these give the complete *new* code and a precise locator, not placeholders.

**Type consistency:** `icons::{Kind, classify(NodeKind,bool,&str)->Kind, icon_for(NodeKind,bool,&str)->&'static str}` defined Task 1, used Task 6. `Node.exec: bool` defined Task 2, read in Task 6 via `classify`. `Action::Parent` defined Task 3, handled Task 5. Theme fields defined Task 4, used Task 6. `tree_row` signature gains `icons: bool` (5 args) in Task 6 and its sole call site (ui/mod.rs) + its in-file test are both updated in Task 6 (same task → no cross-task break). `ascend(&mut App)` defined Task 5, called by `Action::Parent` and root-`Collapse` in the same task. `body` rect variable kept (renamed source `outer[1]`) so the existing modal `centered_rect(..., body)` calls still resolve. Layout indices updated consistently (`header_area=outer[0]`, `body=outer[1]`, `prompt_area=outer[2]`).
