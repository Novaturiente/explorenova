# Multi-Select + Batch Operations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** yazi-style multi-select — Space toggles marks on files/folders, existing keys (yy/dd/p/x/X) operate on the mark set when non-empty (else cursor row), with a leading `*` + accent fg on marked rows and an `N selected` footer segment.

**Architecture:** New path-keyed `BTreeSet<PathBuf>` in `App` (paths stable across arena rebuilds, NodeIds aren't); clipboard widened to a list; a single `App::targets()` chokepoint encodes the "selection-if-any-else-cursor" rule; each batch op iterates `targets()` and surfaces per-entry results.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28.

**Spec:** `docs/superpowers/specs/2026-05-20-multi-select-design.md`
**Base:** branch `master`, post-commit `cc8e32d`.

---

## File Structure

```
src/ui/theme.rs           + pub mark: Color (Yellow)
src/keymap.rs             + Action::{ToggleMark, SelectAllVisible} + defaults (space, ctrl-a)
src/app.rs                + pub marks: BTreeSet<PathBuf>;
                          clipboard -> Option<(Vec<PathBuf>, bool)>;
                          + toggle_mark/clear_marks/select_all_visible/targets;
                          apply(Cancel) also clears marks;
                          confirm_yes iterates targets()
src/ui/panes.rs           tree_row gains `marked: bool` (* prefix + mark fg)
src/ui/mod.rs             pass marked to tree_row; footer shows `N selected`
src/main.rs               handle: ToggleMark/SelectAllVisible arms; Copy/Cut/Paste/Trash/PermanentDelete iterate app.targets()
```

---

## Task 1: `theme.mark` colour

**Files:** Modify `src/ui/theme.rs`.

- [ ] **Step 1: Extend struct** — Find:
```rust
    pub git_conflict: Color,
    pub match_hl: Color,
```
Replace:
```rust
    pub git_conflict: Color,
    pub match_hl: Color,
    pub mark: Color,
```

- [ ] **Step 2: Extend `Default`** — Find:
```rust
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
```
Replace:
```rust
            git_conflict: Color::Red,
            match_hl: Color::Magenta,
            mark: Color::Yellow,
```

- [ ] **Step 3: Verify**
- `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
- `cargo test --lib 2>&1 | grep "test result:"` — 71 passed, 0 failed (additive). Integration: apps_int 1, fs_ops_int 8, git_int 1.

- [ ] **Step 4: Commit**
```bash
git add src/ui/theme.rs
git commit -m "feat: theme.mark colour for multi-select indicator"
```

---

## Task 2: `Action::ToggleMark` + `SelectAllVisible` keymap

**Files:** Modify `src/keymap.rs`.

- [ ] **Step 1: Add variants** — Find:
```rust
    Parent,
    Toggle,
```
Replace:
```rust
    Parent,
    ToggleMark,
    SelectAllVisible,
    Toggle,
```

- [ ] **Step 2: `from_name`** — Find:
```rust
            "parent" => Parent,
            "quit" => Quit, "cancel" => Cancel,
```
Replace:
```rust
            "parent" => Parent,
            "toggle_mark" => ToggleMark,
            "select_all_visible" => SelectAllVisible,
            "quit" => Quit, "cancel" => Cancel,
```

- [ ] **Step 3: defaults** — Find:
```rust
            (Parent, vec!["-"]),
```
Replace:
```rust
            (Parent, vec!["-"]),
            (ToggleMark, vec!["space"]),
            (SelectAllVisible, vec!["ctrl-a"]),
```

- [ ] **Step 4: test** — Add inside the existing `#[cfg(test)] mod tests` block in `src/keymap.rs` (before its closing `}`), matching the existing call style:
```rust
    #[test]
    fn space_toggles_mark_and_ctrl_a_selects_all() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "space"), Some(Action::ToggleMark));
        assert_eq!(
            km.resolve(Mode::Normal, "ctrl-a"),
            Some(Action::SelectAllVisible)
        );
        assert_eq!(Action::from_name("toggle_mark"), Some(Action::ToggleMark));
        assert_eq!(
            Action::from_name("select_all_visible"),
            Some(Action::SelectAllVisible)
        );
    }
```

> Note: `key_token` (`src/keymap.rs`) maps `KeyCode::Char(' ')` to `" "` (single-char path), not `"space"`. Update the existing `parse_chord` "space" branch and the `key_token` Char branch to also produce/consume `"space"` only if needed; the simpler fix is to bind the action to the literal `" "` (single-space) chord string. Verify which the current code uses by reading `key_token` — if it returns `" "` for KeyCode::Char(' '), change the default binding from `vec!["space"]` to `vec![" "]` in Step 3, and the test from `resolve(Mode::Normal, "space")` to `resolve(Mode::Normal, " ")`. The rest of the task is identical.

- [ ] **Step 5: Verify**
- `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — zero errors. New variants are no-ops at `App::apply` (`_ => {}`) and `handle`'s `other => app.apply(other)` until Task 5.
- `cargo test --lib keymap:: 2>&1 | tail -4` — new test passes.
- `cargo test --lib 2>&1 | grep "test result:"` — 72 passed (was 71 + 1).

- [ ] **Step 6: Commit**
```bash
git add src/keymap.rs
git commit -m "feat: keymap ToggleMark (space) + SelectAllVisible (ctrl-a)"
```

---

## Task 3: `App.marks` state + methods + tests (clipboard unchanged here)

**Files:** Modify `src/app.rs`.

- [ ] **Step 1: Import `BTreeSet`** — at the top of `src/app.rs`, find:
```rust
use std::path::PathBuf;
```
Replace:
```rust
use std::collections::BTreeSet;
use std::path::PathBuf;
```

- [ ] **Step 2: Add the field** — Find:
```rust
    pub clipboard: Option<(PathBuf, bool)>, // (path, is_cut)
```
Replace:
```rust
    pub clipboard: Option<(PathBuf, bool)>, // (path, is_cut) — widened in a later task
    pub marks: BTreeSet<PathBuf>,
```

- [ ] **Step 3: Initialise in `App::new`** — Find:
```rust
            clipboard: None,
            status: String::new(),
```
Replace:
```rust
            clipboard: None,
            marks: BTreeSet::new(),
            status: String::new(),
```

- [ ] **Step 4: `apply(Cancel)` clears marks** — Find:
```rust
            Action::Cancel => self.mode = Mode::Normal,
```
Replace:
```rust
            Action::Cancel => {
                self.mode = Mode::Normal;
                self.marks.clear();
            }
```

- [ ] **Step 5: Add public methods + `targets()`** — Find (the end of `confirm_yes`, just before the close of `impl App`):
```rust
        self.pending = None;
        self.mode = Mode::Normal;
    }
```
Replace:
```rust
        self.pending = None;
        self.mode = Mode::Normal;
    }

    pub fn toggle_mark(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            let p = self.arena.nodes[id].path.clone();
            if self.marks.contains(&p) {
                self.marks.remove(&p);
            } else {
                self.marks.insert(p);
            }
        }
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    pub fn select_all_visible(&mut self) {
        for &id in &self.visible {
            let p = self.arena.nodes[id].path.clone();
            self.marks.insert(p);
        }
    }

    /// Single chokepoint for the "selection-if-any-else-cursor" rule.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marks.is_empty() {
            self.marks.iter().cloned().collect()
        } else if let Some(&id) = self.visible.get(self.selected) {
            vec![self.arena.nodes[id].path.clone()]
        } else {
            Vec::new()
        }
    }
```

> The method block is inserted between `confirm_yes`'s closing `}` and the closing `}` of `impl App`. There may be other methods after `confirm_yes` (the YS-5 `focus_*`/`resize_*`/`reorder_*` helpers and `sel_dir`); the safe anchor is the LAST occurrence of `self.mode = Mode::Normal; }` that immediately precedes the impl-closing `}`. Read the file if the find-anchor is ambiguous and insert immediately before the final `}` of `impl App`.

- [ ] **Step 6: Tests** — Find (the end of the `#[cfg(test)] mod tests` block — specifically the closing `}` of the last existing test, just before the module-closing `}`):
```rust
    #[test]
    fn enter_mode_seeds_input() {
        let mut a = app();
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input.value(), "seed");
        assert_eq!(a.mode, Mode::Input);
    }
}
```
Replace:
```rust
    #[test]
    fn enter_mode_seeds_input() {
        let mut a = app();
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input.value(), "seed");
        assert_eq!(a.mode, Mode::Input);
    }

    #[test]
    fn toggle_mark_inserts_then_removes() {
        let mut a = app();
        let p = a.arena.nodes[a.visible[a.selected]].path.clone();
        a.toggle_mark();
        assert!(a.marks.contains(&p));
        a.toggle_mark();
        assert!(!a.marks.contains(&p));
    }

    #[test]
    fn clear_marks_empties() {
        let mut a = app();
        a.toggle_mark();
        assert!(!a.marks.is_empty());
        a.clear_marks();
        assert!(a.marks.is_empty());
    }

    #[test]
    fn select_all_visible_marks_every_visible_path() {
        let mut a = app();
        a.select_all_visible();
        assert_eq!(a.marks.len(), a.visible.len());
        for &id in &a.visible {
            assert!(a.marks.contains(&a.arena.nodes[id].path));
        }
    }

    #[test]
    fn targets_returns_marks_when_nonempty_else_cursor() {
        let mut a = app();
        let cursor_path = a.arena.nodes[a.visible[a.selected]].path.clone();
        assert_eq!(a.targets(), vec![cursor_path.clone()]);
        a.toggle_mark();
        let t = a.targets();
        assert_eq!(t.len(), 1);
        assert!(t.contains(&cursor_path));
    }

    #[test]
    fn cancel_clears_marks() {
        let mut a = app();
        a.toggle_mark();
        assert!(!a.marks.is_empty());
        a.apply(Action::Cancel);
        assert!(a.marks.is_empty());
    }

    #[test]
    fn marks_survive_arena_rebuild_by_path() {
        let mut a = app();
        let p = a.arena.nodes[a.visible[a.selected]].path.clone();
        a.toggle_mark();
        let root_path = a.arena.nodes[a.arena.root].path.clone();
        a.arena = crate::tree::Arena::new(&root_path).unwrap();
        a.visible = a.arena.flatten();
        assert!(a.marks.contains(&p), "marks are keyed by path, survive rebuild");
    }
}
```

- [ ] **Step 7: Verify**
- `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
- `cargo test --lib app:: 2>&1 | tail -10` — new tests pass (6 new + existing).
- `cargo test --lib 2>&1 | grep "test result:"` — `78 passed` (72 prior + 6 new), 0 failed. Integration: apps_int 1, fs_ops_int 8, git_int 1.

- [ ] **Step 8: Commit**
```bash
git add src/app.rs
git commit -m "feat: App.marks (path-keyed) + toggle/clear/select-all/targets + Cancel clears"
```

---

## Task 4: render — `*` prefix + accent name + footer count

**Files:** Modify `src/ui/panes.rs`, `src/ui/mod.rs`.

- [ ] **Step 1: `tree_row` gains `marked: bool`** — Read `src/ui/panes.rs`. Replace the entire `pub fn tree_row<'a>(...) -> Line<'a> { ... }` function (4-arg signature already, with `icons: bool`) with EXACTLY:
```rust
pub fn tree_row<'a>(
    arena: &'a Arena,
    id: NodeId,
    selected: bool,
    theme: &Theme,
    icons: bool,
    marked: bool,
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
    let mut type_color = match kind {
        crate::icons::Kind::Dir => theme.dir,
        crate::icons::Kind::Symlink => theme.symlink,
        crate::icons::Kind::Exec => theme.executable,
        crate::icons::Kind::Media => theme.media,
        crate::icons::Kind::Archive => theme.archive,
        crate::icons::Kind::File => theme.file,
    };
    if marked {
        type_color = theme.mark;
    }

    let g = n.git.glyph();
    let git_clean = matches!(n.git, crate::tree::GitStatus::Clean);
    let git_color = match n.git {
        crate::tree::GitStatus::Modified => theme.git_modified,
        crate::tree::GitStatus::Added => theme.git_added,
        crate::tree::GitStatus::Untracked => theme.git_untracked,
        crate::tree::GitStatus::Conflicted => theme.git_conflict,
        _ => theme.dim,
    };

    let mut spans: Vec<Span> = Vec::new();
    if marked {
        spans.push(Span::styled("* ", Style::default().fg(theme.mark)));
    }
    spans.push(Span::raw(indent));
    let marker_style = if selected {
        Style::default()
    } else {
        Style::default().fg(theme.dim)
    };
    spans.push(Span::styled(marker.to_string(), marker_style));
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
Then update the existing in-file test call from
```rust
let line = tree_row(&a, child, true, &Theme::default(), true);
```
to
```rust
let line = tree_row(&a, child, true, &Theme::default(), true, false);
```

- [ ] **Step 2: `src/ui/mod.rs` — pass `marked` to `tree_row`** — Find:
```rust
                        ratatui::widgets::ListItem::new(crate::ui::panes::tree_row(
                            &app.arena,
                            id,
                            idx == app.selected,
                            &theme,
                            app.config.icons,
                        ))
```
Replace:
```rust
                        ratatui::widgets::ListItem::new(crate::ui::panes::tree_row(
                            &app.arena,
                            id,
                            idx == app.selected,
                            &theme,
                            app.config.icons,
                            app.marks.contains(&app.arena.nodes[id].path),
                        ))
```

- [ ] **Step 3: footer marks segment** — Find:
```rust
    let footer = format!(
        "{}/{}  {}",
        app.selected + 1,
        app.visible.len().max(1),
        app.status
    );
```
Replace:
```rust
    let marks_segment = if app.marks.is_empty() {
        String::new()
    } else {
        format!("{} selected  ", app.marks.len())
    };
    let footer = format!(
        "{}/{}  {}{}",
        app.selected + 1,
        app.visible.len().max(1),
        marks_segment,
        app.status
    );
```

- [ ] **Step 4: Verify**
- `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
- `cargo test --lib 2>&1 | grep "test result:"` — `78 passed`, 0 failed.
- Integration: apps_int 1, fs_ops_int 8, git_int 1.
- `cargo build --release 2>&1 | tail -1` — Finished release.

- [ ] **Step 5: Manual smoke** (`cargo run -- /tmp`): `Space` toggles a `*` + yellow name; footer shows `1 selected`; multiple marks accumulate; selection bar (cursor) + mark overlay readable.

- [ ] **Step 6: Commit**
```bash
git add src/ui/panes.rs src/ui/mod.rs
git commit -m "feat: render marked rows (* + accent fg) + footer N-selected"
```

---

## Task 5: batch ops in `handle()` + clipboard widened to `Vec<PathBuf>` + `confirm_yes` batch

**Files:** Modify `src/app.rs`, `src/main.rs`.

- [ ] **Step 1: Widen `clipboard` in `App`** — Find:
```rust
    pub clipboard: Option<(PathBuf, bool)>, // (path, is_cut) — widened in a later task
```
Replace:
```rust
    pub clipboard: Option<(Vec<PathBuf>, bool)>, // (paths, is_cut) — batch
```

- [ ] **Step 2: Rewrite `confirm_yes` to iterate `targets()`** — Find:
```rust
    pub fn confirm_yes(&mut self) {
        if let Some(Action::PermanentDelete) = self.pending {
            if let Some(&id) = self.visible.get(self.selected) {
                let p = self.arena.nodes[id].path.clone();
                match crate::fs_ops::permanent_delete(&p) {
                    Ok(_) => self.status = format!("deleted {}", p.display()),
                    Err(e) => {
                        self.status = format!("delete failed: {}: {e}", p.display())
                    }
                }
            }
        }
        self.pending = None;
        self.mode = Mode::Normal;
    }
```
Replace:
```rust
    pub fn confirm_yes(&mut self) {
        if let Some(Action::PermanentDelete) = self.pending {
            let targets = self.targets();
            let n = targets.len();
            let mut ok = 0usize;
            let mut failed: Vec<String> = Vec::new();
            for p in &targets {
                match crate::fs_ops::permanent_delete(p) {
                    Ok(_) => ok += 1,
                    Err(_) => failed.push(p.display().to_string()),
                }
            }
            self.status = if failed.is_empty() {
                if n == 1 {
                    format!("deleted {}", targets[0].display())
                } else {
                    format!("deleted {n}")
                }
            } else {
                let preview = failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                let tail = if failed.len() > 2 { ", …" } else { "" };
                format!(
                    "deleted {}/{} ({} failed: {}{})",
                    ok,
                    n,
                    failed.len(),
                    preview,
                    tail
                )
            };
            self.marks.clear();
        }
        self.pending = None;
        self.mode = Mode::Normal;
    }
```

- [ ] **Step 3: `main.rs` — `Action::Trash` batch** — Find:
```rust
        Action::Trash => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if let Err(e) = explorenova::fs_ops::trash_path(&p) {
                    app.status = format!("trash failed: {e}");
                } else {
                    app.status = format!("trashed {}", p.display());
                }
            }
        }
```
Replace:
```rust
        Action::Trash => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to trash".into();
            } else {
                let mut ok = 0usize;
                let mut failed: Vec<String> = Vec::new();
                for p in &targets {
                    match explorenova::fs_ops::trash_path(p) {
                        Ok(_) => ok += 1,
                        Err(_) => failed.push(p.display().to_string()),
                    }
                }
                app.status = if failed.is_empty() {
                    if n == 1 {
                        format!("trashed {}", targets[0].display())
                    } else {
                        format!("trashed {n}")
                    }
                } else {
                    let preview =
                        failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    let tail = if failed.len() > 2 { ", …" } else { "" };
                    format!(
                        "trashed {}/{} ({} failed: {}{})",
                        ok,
                        n,
                        failed.len(),
                        preview,
                        tail
                    )
                };
                app.marks.clear();
            }
        }
```

- [ ] **Step 4: `main.rs` — `Action::PermanentDelete` prompts with count** — Find:
```rust
        Action::PermanentDelete => {
            app.pending = Some(Action::PermanentDelete);
            app.mode = Mode::Confirm;
            app.status = "permanently delete? y/N".into();
        }
```
Replace:
```rust
        Action::PermanentDelete => {
            let n = app.targets().len();
            if n == 0 {
                app.status = "nothing to delete".into();
            } else {
                app.pending = Some(Action::PermanentDelete);
                app.mode = Mode::Confirm;
                app.status = if n == 1 {
                    let name = app
                        .targets()
                        .into_iter()
                        .next()
                        .and_then(|p| {
                            p.file_name().map(|s| s.to_string_lossy().to_string())
                        })
                        .unwrap_or_default();
                    format!("permanently delete {name}? y/N")
                } else {
                    format!("permanently delete {n}? y/N")
                };
            }
        }
```

- [ ] **Step 5: `main.rs` — `Action::Copy` batch** — Find:
```rust
        Action::Copy => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), false));
                app.status = "copied".into();
            }
        }
```
Replace:
```rust
        Action::Copy => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to copy".into();
            } else {
                app.clipboard = Some((targets, false));
                app.status = format!("copied {n}");
            }
        }
```

- [ ] **Step 6: `main.rs` — `Action::Cut` batch** — Find:
```rust
        Action::Cut => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), true));
                app.status = "cut".into();
            }
        }
```
Replace:
```rust
        Action::Cut => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to cut".into();
            } else {
                app.clipboard = Some((targets, true));
                app.status = format!("cut {n}");
            }
        }
```

- [ ] **Step 7: `main.rs` — `Action::Paste` batch** — Find:
```rust
        Action::Paste => {
            if let Some((src, is_cut)) = app.clipboard.clone() {
                let dst_dir = app
                    .visible
                    .get(app.selected)
                    .map(|&id| {
                        let n = &app.arena.nodes[id];
                        if n.is_dir() {
                            n.path.clone()
                        } else {
                            n.path.parent().unwrap_or(cwd).to_path_buf()
                        }
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
                if is_cut {
                    app.clipboard = None;
                }
            }
        }
```
Replace:
```rust
        Action::Paste => {
            let snapshot = app.clipboard.clone();
            let Some((srcs, is_cut)) = snapshot else {
                app.status = "nothing to paste".into();
                return Ok(());
            };
            let dst_dir = app
                .visible
                .get(app.selected)
                .map(|&id| {
                    let n = &app.arena.nodes[id];
                    if n.is_dir() {
                        n.path.clone()
                    } else {
                        n.path.parent().unwrap_or(cwd).to_path_buf()
                    }
                })
                .unwrap_or_else(|| cwd.to_path_buf());
            let mode = if is_cut {
                explorenova::fs_ops::PasteMode::Move
            } else {
                explorenova::fs_ops::PasteMode::Copy
            };
            let n = srcs.len();
            let mut ok = 0usize;
            let mut failed: Vec<String> = Vec::new();
            let mut moved: Vec<std::path::PathBuf> = Vec::new();
            for src in &srcs {
                match explorenova::fs_ops::paste(src, &dst_dir, mode) {
                    Ok(_) => {
                        ok += 1;
                        if is_cut {
                            moved.push(src.clone());
                        }
                    }
                    Err(_) => failed.push(src.display().to_string()),
                }
            }
            app.status = if failed.is_empty() {
                format!("pasted {n} -> {}", dst_dir.display())
            } else {
                let preview = failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                let tail = if failed.len() > 2 { ", …" } else { "" };
                format!(
                    "pasted {}/{} -> {} ({} failed: {}{})",
                    ok,
                    n,
                    dst_dir.display(),
                    failed.len(),
                    preview,
                    tail
                )
            };
            if is_cut {
                app.clipboard = None;
                for p in &moved {
                    app.marks.remove(p);
                }
            }
        }
```

> The `return Ok(())` inside `handle()` is correct because `handle` returns `anyhow::Result<()>`. If the local block style differs (e.g. handle uses a labelled break instead), match the pattern — but `return Ok(())` is the simplest and most common form.

- [ ] **Step 8: `main.rs` — `Action::ToggleMark` + `Action::SelectAllVisible` arms** — Find (the `Action::Parent => ascend(app),` arm added in YS-5):
```rust
        Action::Parent => ascend(app),
```
Replace:
```rust
        Action::Parent => ascend(app),
        Action::ToggleMark => app.toggle_mark(),
        Action::SelectAllVisible => app.select_all_visible(),
```

- [ ] **Step 9: Verify**
- `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — zero errors.
- `cargo test --lib 2>&1 | grep "test result:"` — `78 passed`, 0 failed (no test relies on the old `Option<(PathBuf,bool)>` clipboard shape).
- `cargo test --test apps_int 2>&1 | grep "test result:"` (1) ; `--test fs_ops_int` (8) ; `--test git_int` (1).
- `cargo build --release 2>&1 | tail -1` — Finished release.
- `cargo clippy 2>&1 | grep -cE "^error"` — `0`.

- [ ] **Step 10: Manual smoke**
1. Select two files with `Space` × 2 → `*` + yellow names, footer `2 selected`.
2. `yy` → `copied 2`; `j` to navigate; `p` in a dir → `pasted 2 -> /target`.
3. `dd` two files → `cut 2`; `p` → `pasted 2 -> /dst`; marks cleared (moved sources gone); clipboard empty.
4. `Space` × 2 + `x` → `trashed 2`; marks cleared.
5. `Space` × 2 + `X` → `permanently delete 2? y/N`; `y` → `deleted 2`; marks cleared.
6. `Esc` clears marks; `Ctrl-a` marks every visible row.
7. No marks + `yy` → still works on cursor (`copied 1`).

- [ ] **Step 11: Commit**
```bash
git add src/app.rs src/main.rs
git commit -m "feat: batch yy/dd/p/x/X via App.targets(); clipboard widened to Vec"
```

---

## Task 6: Integration + release

**Files:** none (verification only).

- [ ] **Step 1:** `cd /home/nova/personal/explorer && cargo test 2>&1 | grep "test result:"` — all suites green; expect lib 78, apps_int 1, fs_ops_int 8, git_int 1, doctest 0, 0 failures.
- [ ] **Step 2:** `cargo clippy 2>&1 | grep -cE "^error"` — must be 0. List warnings; flag any newly introduced (vs the pre-existing parse_chord / io::Error::other / from_str / doc-quote set).
- [ ] **Step 3:** `cargo build --release 2>&1 | tail -1` — Finished release; `ls -la target/release/explorenova` exists.
- [ ] **Step 4: Manual e2e (human):** items 1-7 from Task 5 Step 10 again, plus: `Space` on a directory (marks the dir; `X` confirms with the dir name; deletion recurses correctly per the YS X-delete fix); a mix of one folder + one file marked then `dd`+`p` moves both; arena rebuild via fs-watch / `.`/toggle-hidden preserves the mark on a still-visible path; `Esc` clears the set.
- [ ] **Step 5:** No code changes — nothing to commit.

---

## Self-Review

**Spec coverage:**

- Semantic rule (selection-if-any-else-cursor) → Task 3 `targets()` chokepoint + Task 5 every batch arm goes through it ✓
- Toggle (`Space`) + visual mark (`*` + accent fg) → Task 2 binding, Task 4 render ✓
- Marks management: `Esc` clears (via `Cancel.apply`), `Ctrl-a` selects-all → Task 3 (Cancel arm) + Task 2 binding + Task 5 handle arm ✓
- Path-keyed marks survive arena rebuild → Task 3 field type + `marks_survive_arena_rebuild_by_path` test ✓
- Clipboard widened to `Vec<PathBuf>` + paste iterates → Task 5 Steps 1, 7 ✓
- Per-entry batch error collection ("verb N/total (M failed: a, b, …)") → Task 5 (Trash, Paste) + confirm_yes (PermanentDelete) ✓
- After-op semantics: destructive ops (Trash, PermanentDelete) clear marks; Copy/Cut do NOT clear; successful cut-paste removes moved marks AND clears the clipboard → Task 5 Steps 3, 5, 6, 7 + confirm_yes ✓
- Confirm prompt with count + singular form (1 → name) → Task 5 Step 4 + confirm_yes singular branch ✓
- Render: `*` prefix span + accent name override; marker dim only on non-selected; full-row cursor bar via `List.highlight_style` (unchanged) → Task 4 ✓
- Footer "N selected " segment only when marks non-empty → Task 4 Step 3 ✓
- Theme `mark` colour with fixed default → Task 1 ✓
- Out-of-scope items (no visual-mode, no mark-byte counts, no persistence, no per-pane marks, no mixed clipboard, no undo) → unimplemented by design, no task needed ✓

No spec requirement is left without a task.

**Placeholder scan:** Every code step has full code or an exact anchored replacement. Two steps include a clarifying note ("verify the space-vs-` ` binding form by reading `key_token`"; "the safe anchor is the LAST `self.mode = Mode::Normal; }` before impl-closing `}`") — these are concrete instructions, not placeholders. No "TBD"/"handle errors"/vague items.

**Type consistency:**
- `App.marks: BTreeSet<PathBuf>` (Task 3) read by `tree_row.marked` lookup (Task 4) and `targets()` (Task 3); written by `toggle_mark`/`clear_marks`/`select_all_visible` (Task 3) and `Cancel.apply` (Task 3) and `confirm_yes`/Trash arm (Task 5) and successful-cut-paste (Task 5 Step 7).
- `App.clipboard: Option<(Vec<PathBuf>, bool)>` (Task 5 Step 1) written by Copy/Cut (Task 5 Steps 5-6), read+cleared by Paste (Task 5 Step 7); old `(PathBuf, bool)` callers all updated in the same task.
- `tree_row` 6-arg signature `(arena, id, selected, theme, icons, marked)` — defined Task 4 Step 1, called Task 4 Step 2 with `app.marks.contains(...)`, in-file test updated Task 4 Step 1.
- `Action::{ToggleMark, SelectAllVisible}` defined Task 2, handled Task 5 Step 8 + `from_name`/defaults Task 2.
- `targets()` defined Task 3 Step 5, called Task 5 Steps 3-7 and `confirm_yes` Task 5 Step 2.

All signatures and field types are consistent across tasks; no name drift; no cross-task dangling reference.
