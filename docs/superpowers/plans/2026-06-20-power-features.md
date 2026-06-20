# Power Features Implementation Plan (filter, op-cancel, bulk-rename, content-search, zoxide/cwd, openers, linemodes)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the biggest UX gaps vs yazi by adding a tree filter, cancellable file ops, `$EDITOR` bulk rename, ripgrep content search, zoxide + cwd-on-exit shell integration, extension-based opener rules, and toggleable linemodes — all without new crate dependencies.

**Architecture:** Each feature is wired through the existing seams: new `Action` variants (rebindable via `[keys]`), the `Mode::Input` + `app.pending` text-prompt machinery (no new `Mode`), the arena `flatten` for filtering, the existing `OpState` poll loop for cancel, and `tree_row` for linemodes. Two tiny new modules (`linemode.rs`, `zoxide.rs`) and one new `search.rs` helper hold the only non-trivial pure logic; everything else extends current files. External tools (`rg`, `zoxide`, `$EDITOR`) are shelled out via the existing `shell`/`command` engines and degrade with a status message when absent.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, nucleo (fuzzy, already present), `std::process::Command` for tool shell-outs. No new crates.

## Global Constraints

- Errors use `anyhow` everywhere. No custom error enum; `thiserror` was removed as unused — do not reintroduce it.
- **No new dependencies.** `rg`/`zoxide`/`$EDITOR` are external binaries shelled out, not crates. mtime is humanized as a relative duration (no `chrono`).
- Keep `cargo clippy` clean (CI-clean is the standard).
- `src/cargo-home/` and `src/explorenova/` are makepkg build artifacts — never edit; scope greps with `git ls-files 'src/*.rs'`.
- Mark deliberate simplifications/known ceilings with a `ponytail:` comment.
- Config parsing stays serde-over-defaults: missing fields take defaults, never panic. The `sample_config_parses_to_defaults` drift test in `src/config.rs` must keep passing — any new config field must be **commented** in `SAMPLE_CONFIG` (active lines must round-trip to `with_defaults()`).
- New keybindings must be rebindable: every new `Action` gets a name in `Action::from_name` and a default chord in `Keymap::defaults`. Defaults chosen to avoid collisions: `f` filter, `F` content_search, `z` zoxide_jump, `b` bulk_rename, `L` cycle_linemode. (Op-cancel reuses Esc/Ctrl-c while an op is in flight — no binding.)

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/linemode.rs` | `LineMode` enum + cell rendering (size/mtime/perms) | **Create** |
| `src/zoxide.rs` | `zoxide query`/`add` shell-outs | **Create** |
| `src/keymap.rs` | `Action` variants, names, default chords | Modify: add 5 actions |
| `src/state.rs` | persist `line_mode` | Modify: add field |
| `src/config.rs` | `openers` table + `SAMPLE_CONFIG` docs | Modify: add field, document |
| `src/app.rs` | filter state+methods, linemode field, opener/cancel helpers | Modify |
| `src/tree/arena.rs` | filtered flatten | Modify: `filter` field + `flatten` branch |
| `src/ops.rs` | cancel flag threaded into worker | Modify |
| `src/search.rs` | ripgrep run + output parse | Modify: add `content_search`/`parse_rg` |
| `src/fs_ops.rs` | bulk-rename plan (pure, testable) | Modify: add `plan_bulk_rename` |
| `src/shell.rs` | `$EDITOR` launch + bulk-rename temp-file edit | Modify: add `editor()` + `edit_lines` |
| `src/dispatch.rs` | action handlers + input-prompt wiring for all features | Modify |
| `src/ui/panes.rs` | `tree_row` renders the linemode cell | Modify: add param |
| `src/ui/panes_render.rs` | pass `app.line_mode` to `tree_row` | Modify |
| `src/ui/overlays.rs` | op-modal "esc cancel" hint; Input title "Filter" | Modify |
| `src/main.rs` | cwd-file write on quit | Modify |
| `src/cli.rs` | `--cwd-file` flag | Modify |
| `src/lib.rs` | register `linemode`, `zoxide` modules | Modify |
| `README.md` | document new keys, config, shell wrapper | Modify |

---

## Task 0: Safety fixes (C1 cross-fs move, C2 batch-trash confirm)

Prepended per review. Two small, low-risk correctness fixes found in the audit.

**Files:**
- Modify: `src/fs_ops.rs` (extract a testable cross-fs single-file move with dest-cleanup), `src/ops.rs` (call it), `src/dispatch.rs` (confirm batch trash)
- Test: `src/fs_ops.rs`, `src/dispatch.rs`

**Interfaces:**
- Produces: `fs_ops::move_file_cross_fs(src: &Path, dst: &Path) -> Result<(), String>` — copy then remove source; on remove failure, delete the just-written `dst` and return `Err` (so "failed" means source-intact, never silently duplicated).

### C1 — cross-filesystem single-file move leaves no duplicate

- [ ] **Step 1: Write the failing test (unix: read-only source dir forces remove failure)**

Add to the `tests` module in `src/fs_ops.rs`:

```rust
    #[cfg(unix)]
    #[test]
    fn move_file_cross_fs_cleans_dest_when_source_remove_fails() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir().unwrap();
        let srcdir = d.path().join("ro");
        std::fs::create_dir(&srcdir).unwrap();
        let src = srcdir.join("f.txt");
        std::fs::write(&src, b"data").unwrap();
        let dst = d.path().join("f.txt");
        // Make the source's parent read-only so remove_file(src) fails
        // AFTER copy(src,dst) has already succeeded.
        std::fs::set_permissions(&srcdir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = move_file_cross_fs(&src, &dst);
        // restore perms so tempdir cleanup works
        std::fs::set_permissions(&srcdir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err(), "remove failure must surface as Err");
        assert!(!dst.exists(), "dest must be cleaned up, not left as a duplicate");
        assert!(src.exists(), "source must remain intact");
    }

    #[test]
    fn move_file_cross_fs_happy_path_moves() {
        let d = tempdir().unwrap();
        let src = d.path().join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        let dst = d.path().join("b.txt");
        assert!(move_file_cross_fs(&src, &dst).is_ok());
        assert!(dst.exists() && !src.exists());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib move_file_cross_fs`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement the helper**

In `src/fs_ops.rs`, add:

```rust
/// Cross-filesystem single-file move: copy then remove the source. If the
/// source removal fails after a successful copy, delete the freshly-written
/// destination so a reported failure never means a silent duplicate.
pub fn move_file_cross_fs(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::copy(src, dst).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::remove_file(src) {
        let _ = std::fs::remove_file(dst);
        return Err(format!("move failed (source kept): {e}"));
    }
    Ok(())
}
```

- [ ] **Step 4: Call it from the worker**

In `src/ops.rs` `spawn_paste`, replace the cross-device single-file branch (lines ~185-190):

```rust
                            } else {
                                crate::fs_ops::move_file_cross_fs(src, &dst)
                            }
```

For the cross-device **directory** branch just above it (line ~171-184), `move_dir_with_progress` can abort mid-tree with no rollback. Don't attempt a full rollback (complex); make the failure legible by mapping its error:

```rust
                                .map_err(|e| format!("dir move partial — check both locations: {e}"))
```

(Replace the existing `.map_err(|e| e.to_string())` on that dir branch only.)

- [ ] **Step 5: Run tests**

Run: `cargo test --lib fs_ops`
Expected: PASS.

### C2 — confirm a batch trash even when `confirm_trash` is off

Keeps fast single-file trash (the intended default) but guards a multi-file fat-finger.

- [ ] **Step 6: Write the failing dispatch test**

Add to the `tests` module in `src/dispatch.rs`:

```rust
    #[test]
    fn batch_trash_confirms_even_when_confirm_trash_off() {
        let d = tempdir().unwrap();
        for f in ["a.txt", "b.txt"] {
            std::fs::write(d.path().join(f), "x").unwrap();
        }
        let (mut app, tx, _rx) = app_at(d.path());
        assert!(!app.config.confirm_trash, "default off");
        // Mark both files.
        for &id in &app.visible.clone() {
            if !app.arena.nodes[id].is_dir() {
                app.marks.insert(app.arena.nodes[id].path.clone());
            }
        }
        handle(&mut app, Action::Trash, d.path(), &tx).unwrap();
        assert_eq!(app.mode, crate::keymap::Mode::Confirm, "batch trash must confirm");
        // Single-file path stays fast: clear marks, cursor on one file.
        app.marks.clear();
        app.mode = crate::keymap::Mode::Normal;
        handle(&mut app, Action::Trash, d.path(), &tx).unwrap();
        assert_ne!(app.mode, crate::keymap::Mode::Confirm, "single trash stays immediate");
    }
```

- [ ] **Step 7: Run to verify failure**

Run: `cargo test --lib batch_trash_confirms_even_when_confirm_trash_off`
Expected: FAIL — single-file path is fine but the batch case currently trashes immediately (mode stays Normal).

- [ ] **Step 8: Gate the Trash action on count**

In `src/dispatch.rs` `handle`, change the `Action::Trash` arm (line ~908-914):

```rust
        Action::Trash => {
            // Confirm when explicitly opted in, OR whenever more than one
            // item is targeted (a batch fat-finger has no in-app undo).
            if app.config.confirm_trash || app.targets().len() > 1 {
                prompt_trash(app);
            } else {
                trash_targets(app, git_tx);
            }
        }
```

- [ ] **Step 9: Run tests**

Run: `cargo test --lib`
Expected: PASS (new test + existing `confirm_trash_routes_to_confirm_mode`).

- [ ] **Step 10: Build, clippy, commit**

Run: `cargo build && cargo clippy`

```bash
git add src/fs_ops.rs src/ops.rs src/dispatch.rs
git commit -m "fix(ops): clean dest on failed cross-fs move; confirm batch trash"
```

---

## Task 1: Linemodes (size/mtime/perms column)

Self-contained and isolated — good warm-up. `Node` already stores `size`, `mtime`, `mode` (`src/tree/node.rs:68-74`).

**Files:**
- Create: `src/linemode.rs`
- Modify: `src/lib.rs` (register module), `src/state.rs` (persist), `src/app.rs` (field + init), `src/keymap.rs` (action), `src/dispatch.rs` (handler), `src/ui/panes.rs` (`tree_row`), `src/ui/panes_render.rs` (pass-through)
- Test: `src/linemode.rs`

**Interfaces:**
- Produces: `LineMode { None, Size, Mtime, Perms }` (Copy, serde, Default=None); `LineMode::next(self) -> LineMode`; `LineMode::label(self) -> &'static str`; `LineMode::cell(self, n: &crate::tree::node::Node) -> String` (empty string for `None`); `App.line_mode: LineMode`.

- [ ] **Step 1: Write the failing test (create the module with tests first)**

Create `src/linemode.rs`:

```rust
//! Optional right-aligned metadata column in the tree (size / mtime /
//! permissions), cycled with the `L` key and persisted in state.

use crate::tree::node::{Node, NodeKind};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineMode {
    #[default]
    None,
    Size,
    Mtime,
    Perms,
}

impl LineMode {
    pub fn next(self) -> LineMode {
        match self {
            LineMode::None => LineMode::Size,
            LineMode::Size => LineMode::Mtime,
            LineMode::Mtime => LineMode::Perms,
            LineMode::Perms => LineMode::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineMode::None => "none",
            LineMode::Size => "size",
            LineMode::Mtime => "mtime",
            LineMode::Perms => "perms",
        }
    }

    /// The right-aligned cell text for `n`. Empty for `None` and for the
    /// size of a directory (we don't recurse to size dirs).
    pub fn cell(self, n: &Node) -> String {
        match self {
            LineMode::None => String::new(),
            LineMode::Size => {
                if n.is_dir() {
                    String::new()
                } else {
                    crate::status::human_bytes(n.size)
                }
            }
            LineMode::Mtime => rel_time(n.mtime),
            LineMode::Perms => perms(n.kind, n.mode),
        }
    }
}

/// Humanize "time since mtime" as a compact relative string. No chrono:
/// just bucket the elapsed duration. Future timestamps clamp to "0s".
fn rel_time(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 86_400 * 7 {
        format!("{}d", secs / 86_400)
    } else if secs < 86_400 * 365 {
        format!("{}w", secs / (86_400 * 7))
    } else {
        format!("{}y", secs / (86_400 * 365))
    }
}

/// Unix `rwxr-xr-x`-style string from mode bits. Leading type char:
/// `d` dir, `l` symlink, `-` file. mode==0 (non-unix/unreadable) → "?".
fn perms(kind: NodeKind, mode: u32) -> String {
    if mode == 0 {
        return "?".to_string();
    }
    let t = match kind {
        NodeKind::Dir => 'd',
        NodeKind::Symlink => 'l',
        NodeKind::File => '-',
    };
    let bit = |shift: u32, ch: char| if mode & (1 << shift) != 0 { ch } else { '-' };
    format!(
        "{t}{}{}{}{}{}{}{}{}{}",
        bit(8, 'r'), bit(7, 'w'), bit(6, 'x'),
        bit(5, 'r'), bit(4, 'w'), bit(3, 'x'),
        bit(2, 'r'), bit(1, 'w'), bit(0, 'x'),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn file(size: u64, mode: u32) -> Node {
        let mut n = Node::new(PathBuf::from("/x/f"), NodeKind::File, 0, None);
        n.size = size;
        n.mode = mode;
        n
    }

    #[test]
    fn cycle_wraps() {
        assert_eq!(LineMode::None.next(), LineMode::Size);
        assert_eq!(LineMode::Perms.next(), LineMode::None);
    }

    #[test]
    fn none_cell_is_empty() {
        assert_eq!(LineMode::None.cell(&file(10, 0o644)), "");
    }

    #[test]
    fn size_cell_human() {
        // human_bytes(1024) is "1.0K" / "1 KiB"-ish — just assert non-empty
        // and that a directory yields empty.
        assert!(!LineMode::Size.cell(&file(2048, 0)).is_empty());
        let mut d = Node::new(PathBuf::from("/x/d"), NodeKind::Dir, 0, None);
        d.size = 0;
        assert_eq!(LineMode::Size.cell(&d), "");
    }

    #[test]
    fn perms_formats_rwx() {
        assert_eq!(perms(NodeKind::File, 0o755), "-rwxr-xr-x");
        assert_eq!(perms(NodeKind::Dir, 0o750), "drwxr-x---");
        assert_eq!(perms(NodeKind::File, 0), "?");
    }

    #[test]
    fn rel_time_buckets() {
        let now = SystemTime::now();
        assert_eq!(rel_time(now), "0s");
        assert_eq!(rel_time(now - Duration::from_secs(120)), "2m");
        assert_eq!(rel_time(now - Duration::from_secs(3 * 3600)), "3h");
    }
}
```

- [ ] **Step 2: Register the module**

In `src/lib.rs`, add after `pub mod keymap;` (alphabetical-ish, line ~13):

```rust
pub mod linemode;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test --lib linemode`
Expected: PASS (5 tests). If `human_bytes` is private, see Step 4.

- [ ] **Step 4: Ensure `status::human_bytes` is public**

Confirm `src/status.rs` exposes `pub fn human_bytes`. It is already used by `overlays.rs` (`crate::status::human_bytes`), so it is public. If `cargo build` says otherwise, add `pub`.

- [ ] **Step 5: Persist line_mode in State**

In `src/state.rs`, add to `struct State` (after `sort`, line ~22):

```rust
    pub line_mode: crate::linemode::LineMode,
```

`#[serde(default)]` on the struct already makes a missing key default to `LineMode::None`. Add a round-trip test to the `tests` module:

```rust
    #[test]
    fn line_mode_persists() {
        let s = State { line_mode: crate::linemode::LineMode::Perms, ..State::default() };
        assert_eq!(State::parse(&s.to_toml()).line_mode, crate::linemode::LineMode::Perms);
    }
```

Run: `cargo test --lib state::tests::line_mode_persists` → PASS.

- [ ] **Step 6: Add the App field**

In `src/app.rs`, add to `struct App` (after `sort: ...`, line ~107):

```rust
    /// Active tree linemode column (size/mtime/perms/none). Cycled with
    /// `L`, mirrored into `state.line_mode`, persisted on quit.
    pub line_mode: crate::linemode::LineMode,
```

In `App::new` (line ~146), capture it from state before the `App { ... }` literal:

```rust
        let line_mode = state.line_mode;
```

and add `line_mode,` to the struct literal (near `sort,`).

- [ ] **Step 7: Add the action + binding**

In `src/keymap.rs`:
- Add `CycleLinemode,` to `enum Action` (after `SortToggleDirFirst`, line ~70).
- Add to `Action::from_name` (near the sort names, line ~109): `"cycle_linemode" => CycleLinemode,`
- Add a default chord in `defaults()` (line ~177 area): `(CycleLinemode, vec!["L"]),`

- [ ] **Step 8: Handle the action**

In `src/dispatch.rs` `handle` (add an arm before `other => app.apply(other)`, line ~1072):

```rust
        Action::CycleLinemode => {
            app.line_mode = app.line_mode.next();
            app.state.line_mode = app.line_mode;
            app.set_status(format!("linemode: {}", app.line_mode.label()));
        }
```

- [ ] **Step 9: Render the linemode cell in tree_row**

In `src/ui/panes.rs`, add a parameter to `tree_row` (signature line ~10) — add `line_mode: crate::linemode::LineMode,` after `width: u16`:

```rust
    width: u16,
    line_mode: crate::linemode::LineMode,
) -> Line<'a> {
```

Then replace the git-glyph padding block (lines ~97-105) so the linemode cell sits right-aligned before the git glyph:

```rust
    // Right side: optional linemode cell + 2-cell git glyph, right-aligned.
    let lm = line_mode.cell(n);
    let lm_cells = if lm.is_empty() { 0 } else { lm.chars().count() + 1 };
    let total = width as usize;
    let git_cells = 2usize;
    if used + git_cells + lm_cells < total {
        let pad = total - used - git_cells - lm_cells;
        spans.push(Span::raw(" ".repeat(pad)));
    } else {
        spans.push(Span::raw(" "));
    }
    if !lm.is_empty() {
        spans.push(Span::styled(format!("{lm} "), Style::default().fg(theme.dim)));
    }
```

Update the two existing `tree_row(...)` test calls in this file's `tests` module (lines ~126, ~138) to pass a trailing `crate::linemode::LineMode::None`.

- [ ] **Step 10: Pass app.line_mode from the renderer**

In `src/ui/panes_render.rs`, find the `tree_row(` call inside `render_pane_tree` and append `, app.line_mode` as the final argument. (Grep: `rg -n "tree_row\(" src/ui/panes_render.rs`.)

- [ ] **Step 11: Build, test, clippy**

Run: `cargo build && cargo test --lib && cargo clippy`
Expected: clean. Manually confirm `L` cycles the column when you run the app.

- [ ] **Step 12: Commit**

```bash
git add src/linemode.rs src/lib.rs src/state.rs src/app.rs src/keymap.rs src/dispatch.rs src/ui/panes.rs src/ui/panes_render.rs
git commit -m "feat(tree): toggleable linemodes (size/mtime/perms) on L"
```

---

## Task 2: Opener rules (extension → command, fallback default app)

**Files:**
- Modify: `src/config.rs` (field + `SAMPLE_CONFIG`), `src/app.rs` (helper), `src/dispatch.rs` (two open sites)
- Test: `src/config.rs`, `src/app.rs`

**Interfaces:**
- Produces: `Config.openers: BTreeMap<String, String>` (lowercased ext → command template); `App::open_path(&mut self, path: &Path)` — runs the matching opener (expand `{file}`/`{paths}` against just `path`), else `open::that_detached`.

- [ ] **Step 1: Write the failing config test**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn openers_table_parses() {
        let (c, err) = Config::load_str("[openers]\nmd = \"glow {file}\"\n");
        assert!(err.is_none());
        assert_eq!(c.openers.get("md").map(String::as_str), Some("glow {file}"));
        // missing table -> empty
        let (c2, _) = Config::load_str("icons = true\n");
        assert!(c2.openers.is_empty());
    }
```

- [ ] **Step 2: Run it to verify failure**

Run: `cargo test --lib openers_table_parses`
Expected: FAIL — `no field 'openers' on type Config`.

- [ ] **Step 3: Add the field**

In `src/config.rs`, add to `struct Config` (after `theme`, line ~170):

```rust
    /// Extension (lowercased, no dot) -> shell command template, run when
    /// opening a file of that type (o / Enter). Same `{file}`/`{paths}`/…
    /// placeholders as `[commands]`. No rule -> the OS default app.
    pub openers: BTreeMap<String, String>,
```

- [ ] **Step 4: Document it in SAMPLE_CONFIG (keep the drift test green)**

In `src/config.rs` `SAMPLE_CONFIG` (before the `--- Theme ---` block, line ~64), add a fully-commented block (commented so the empty default still round-trips):

```rust
# --- Openers -----------------------------------------------------------
# Per-extension open commands (lowercase ext, no dot). Opening a file of a
# listed type (o / Enter) runs the command instead of the OS default app;
# any other type still uses the default. Same placeholders as [commands].
# [openers]
# md = "glow {file}"
# html = "firefox {file}"
# mp4 = "mpv {file}"

```

- [ ] **Step 5: Verify config tests pass**

Run: `cargo test --lib config::tests`
Expected: PASS (including `openers_table_parses` and `sample_config_parses_to_defaults`).

- [ ] **Step 6: Write the App::open_path test**

Add to the `tests` module in `src/app.rs` (it has an `app()` helper, line ~876). This test only checks the *routing decision*, so expose a pure helper `opener_for` and test that:

In `src/app.rs`, add a pure associated function near `run_command` (line ~718):

```rust
    /// The opener template for `path`'s extension, if any. Pure lookup.
    pub fn opener_for(&self, path: &std::path::Path) -> Option<String> {
        let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();
        self.config.openers.get(&ext).cloned()
    }

    /// Open `path`: run its extension opener (against just this path), or
    /// fall back to the OS default app. Used by `o` and Enter-on-file.
    pub fn open_path(&mut self, path: &std::path::Path) {
        if let Some(template) = self.opener_for(path) {
            let dir = path.parent().map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.arena.nodes[self.arena.root].path.clone());
            let subst = crate::command::Subst { targets: vec![path.to_path_buf()], dir };
            let cmd = crate::command::expand(&template, &subst);
            if let Err(e) = crate::shell::run_once(
                &self.arena.nodes[self.arena.root].path.clone(), &cmd) {
                self.set_err(format!("opener failed: {e}"));
            }
            self.force_redraw = true;
        } else if let Err(e) = open::that_detached(path) {
            self.set_err(format!("open failed: {e}"));
        }
    }
```

Test:

```rust
    #[test]
    fn opener_for_matches_extension() {
        let mut a = app();
        a.config.openers.insert("md".into(), "glow {file}".into());
        assert_eq!(a.opener_for(std::path::Path::new("/x/readme.md")).as_deref(), Some("glow {file}"));
        assert!(a.opener_for(std::path::Path::new("/x/photo.png")).is_none());
        assert!(a.opener_for(std::path::Path::new("/x/Makefile")).is_none());
    }
```

- [ ] **Step 7: Run it to verify it passes**

Run: `cargo test --lib opener_for_matches_extension`
Expected: PASS.

> ponytail: openers run suspended via `run_once` (like `!` and `[commands]`) — correct for terminal editors; a GUI app briefly suspends the TUI until it's closed. Add a non-blocking flag later if that feels laggy.

- [ ] **Step 8: Route the two open sites through open_path**

In `src/dispatch.rs`:
- `Action::OpenDefault` (line ~849-856): replace the `let _ = open::that_detached(&p);` body so it calls `app.open_path(&p)`:

```rust
        Action::OpenDefault => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if !app.arena.nodes[id].is_dir() {
                    app.open_path(&p);
                }
            }
        }
```

- `toggle_action` file/non-archive branch (line ~1305): replace `} else if let Err(e) = open::that_detached(&p) { app.set_err(...) }` with:

```rust
            } else {
                app.open_path(&p);
            }
```

- [ ] **Step 9: Build, test, clippy, commit**

Run: `cargo build && cargo test --lib && cargo clippy`

```bash
git add src/config.rs src/app.rs src/dispatch.rs
git commit -m "feat(open): per-extension opener rules, fallback to default app"
```

---

## Task 3: Filter mode (prune tree to matches, keep ancestors)

The biggest task. Reuses `Mode::Input` with `pending = Action::Filter` (no new `Mode`). Live fuzzy match over the whole subtree; the arena's `flatten` is pruned to matches + their ancestor dirs.

**Files:**
- Modify: `src/tree/arena.rs` (filter field + filtered flatten), `src/keymap.rs` (action), `src/app.rs` (filter state + methods), `src/dispatch.rs` (action handler + input wiring), `src/ui/overlays.rs` (Input title)
- Test: `src/tree/arena.rs`, `src/app.rs`

**Interfaces:**
- Produces: `Arena.filter: Option<std::collections::HashSet<NodeId>>` (None = no filter); `Arena::flatten` honors it. `App.filter_query: String`; `App::begin_filter()`, `App::live_filter()`, `App::clear_filter()`, `App::filter_active() -> bool`.

- [ ] **Step 1: Write the failing arena test (filtered flatten)**

Add to the `tests` module in `src/tree/arena.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib arena`
Expected: FAIL — `no field 'filter' on type Arena`.

- [ ] **Step 3: Add the filter field**

In `src/tree/arena.rs` `struct Arena` (line ~27):

```rust
pub struct Arena {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub show_hidden: bool,
    pub sort: SortState,
    /// When `Some`, `flatten` shows only these node ids (matches + their
    /// ancestor dirs). Descends through ancestor dirs regardless of their
    /// `expanded` flag. `None` = normal expansion-driven flatten.
    pub filter: Option<std::collections::HashSet<NodeId>>,
}
```

Initialize `filter: None` in `Arena::new` (line ~38-43).

- [ ] **Step 4: Branch `flatten`/`visit` on the filter**

In `src/tree/arena.rs`, replace `flatten`/`visit` (lines ~181-197):

```rust
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
```

- [ ] **Step 5: Run arena tests**

Run: `cargo test --lib arena`
Expected: PASS (new + existing).

- [ ] **Step 6: Add filter state + methods on App**

In `src/app.rs` `struct App`, add (after `search_cands`, line ~126):

```rust
    /// Active filter query (empty = no filter). When non-empty the tree
    /// is pruned to fuzzy matches + their ancestor dirs.
    pub filter_query: String,
    /// Cached candidate paths for the live filter (collected once on
    /// begin_filter, dropped on clear).
    pub filter_cands: Option<Vec<PathBuf>>,
```

Initialize both in `App::new` (line ~189 area): `filter_query: String::new(),` and `filter_cands: None,`.

Add the methods near the search methods (after `search_cycle`, line ~618):

```rust
    pub fn filter_active(&self) -> bool {
        !self.filter_query.is_empty()
    }

    /// Start a filter session: cache the candidate path list once.
    pub fn begin_filter(&mut self) {
        let root = self.arena.nodes[self.arena.root].path.clone();
        self.filter_cands = Some(crate::search::collect(&root, self.arena.show_hidden));
        self.filter_query.clear();
    }

    /// Recompute the allowed set from the current input and re-prune the
    /// tree. Empty query clears the prune (shows everything) but keeps the
    /// session candidate cache.
    ///
    /// ponytail: recomputes the allowed set (reveal each match) on every
    /// keystroke — fine for typical dirs; a 100k-node tree would lag.
    pub fn live_filter(&mut self) {
        self.filter_query = self.input.value();
        let Some(cands) = self.filter_cands.clone() else { return };
        if self.filter_query.is_empty() {
            self.arena.filter = None;
            self.visible = self.arena.flatten();
            self.selected = self.selected.min(self.visible.len().saturating_sub(1));
            self.set_status("filter: (all)");
            return;
        }
        let root = self.arena.nodes[self.arena.root].path.clone();
        let ranked = crate::search::rank(&cands, &root, &self.filter_query);
        let mut allowed: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (p, _, _) in &ranked {
            if let Ok(Some(id)) = self.arena.reveal(p) {
                let mut cur = Some(id);
                while let Some(c) = cur {
                    allowed.insert(c);
                    cur = self.arena.nodes[c].parent;
                }
            }
        }
        let n = ranked.len();
        self.arena.filter = Some(allowed);
        self.visible = self.arena.flatten();
        self.selected = 0;
        if n == 0 {
            self.set_status("filter: no matches");
        } else {
            self.set_status(format!("filter: {n} matches"));
        }
    }

    /// Clear an active filter and restore the full tree.
    pub fn clear_filter(&mut self) {
        self.filter_query.clear();
        self.filter_cands = None;
        if self.arena.filter.is_some() {
            self.arena.filter = None;
            let sel_path = self.visible.get(self.selected)
                .map(|&id| self.arena.nodes[id].path.clone());
            self.visible = self.arena.flatten();
            match sel_path {
                Some(p) if self.navigate_to(&p) => {}
                _ => self.selected = self.selected.min(self.visible.len().saturating_sub(1)),
            }
        }
    }
```

- [ ] **Step 7: Write the App filter test**

Add to the `tests` module in `src/app.rs`. The `app()` helper roots at `.`; use a dedicated tempdir so matches are deterministic:

```rust
    #[test]
    fn live_filter_prunes_then_clears() {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.txt"), "1").unwrap();
        std::fs::write(d.path().join("apple.rs"), "1").unwrap();
        std::fs::write(d.path().join("banana.rs"), "1").unwrap();
        let arena = Arena::new(d.path()).unwrap();
        let mut a = App::new(arena, Config::with_defaults(), State::default());
        a.begin_filter();
        a.input.set("apple");
        a.live_filter();
        let names: Vec<String> = a.visible.iter()
            .map(|&id| a.arena.nodes[id].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "apple.rs"));
        assert!(!names.iter().any(|n| n == "banana.rs"), "non-match pruned");
        a.clear_filter();
        assert!(a.arena.filter.is_none());
        let names2: Vec<String> = a.visible.iter()
            .map(|&id| a.arena.nodes[id].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names2.iter().any(|n| n == "banana.rs"), "restored after clear");
    }
```

Run: `cargo test --lib live_filter_prunes_then_clears` → PASS.

- [ ] **Step 8: Add the action + binding**

In `src/keymap.rs`:
- Add `Filter,` to `enum Action` (after `Search`/`SearchPrev` group, line ~48).
- Add to `from_name`: `"filter" => Filter,`
- Add default chord: `(Filter, vec!["f"]),`

- [ ] **Step 9: Wire the action handler**

In `src/dispatch.rs` `handle` (after the `Action::Search` arm, line ~919):

```rust
        Action::Filter => {
            app.begin_filter();
            app.pending = Some(Action::Filter);
            app.enter_mode(Mode::Input, "");
        }
```

> `enter_mode` clears `status`; the live status appears on the first keystroke. That's fine — an empty filter shows everything.

- [ ] **Step 10: Live-update + commit + cancel in the input handler**

In `src/dispatch.rs` `handle_text_input_key` (lines ~636-687): after each edit (Delete, Backspace, Char insert) the code currently calls `app.live_search()` when `mode == Search`. Extend each of those three branches to also handle filter. Replace each `if app.mode == Mode::Search { app.live_search(); }` with:

```rust
                if app.mode == Mode::Search {
                    app.live_search();
                } else if app.pending == Some(Action::Filter) {
                    app.live_filter();
                }
```

In the `KeyCode::Esc` arm (line ~643), the filter must be cleared on cancel. Replace:

```rust
        KeyCode::Esc => {
            if app.mode == Mode::Search {
                app.end_search();
            }
            app.mode = Mode::Normal;
            app.input.clear();
            app.pending = None;
        }
```

with:

```rust
        KeyCode::Esc => {
            if app.mode == Mode::Search {
                app.end_search();
            }
            if app.pending == Some(Action::Filter) {
                app.clear_filter();
            }
            app.mode = Mode::Normal;
            app.input.clear();
            app.pending = None;
        }
```

In `text_input_enter` `Mode::Input` match (line ~697), add a `Filter` arm that **keeps** the prune applied and just leaves the prompt:

```rust
            Some(Action::Filter) => {
                // Commit: filter stays applied, candidate cache freed.
                app.filter_cands = None;
                app.mode = Mode::Normal;
            }
```

- [ ] **Step 11: Clear the filter on navigation that rebuilds the arena**

A reroot/ascend/refresh builds a fresh `Arena` (filter `None`), but `app.filter_query` would stay stale. Clear the query string in those paths. In `src/dispatch.rs`:
- In `reroot` (after `app.arena = na;`, line ~61): `app.filter_query.clear(); app.filter_cands = None;`
- In `ascend` (after `app.arena = na;`, line ~213): same two lines.
- `refresh_in_place` and `refresh_dirs` rebuild/reload — if a filter is active, re-apply it. Simplest: at the **start** of `refresh_in_place` and `refresh_dirs`, if `app.filter_active()`, clear it first (`app.arena.filter = None; app.filter_query.clear(); app.filter_cands = None;`) so the refresh shows the full tree. (ponytail: filter doesn't survive an external-change refresh; re-press `f` — acceptable for v1.)

- [ ] **Step 12: Input overlay title**

In `src/ui/overlays.rs` `render_input` `title` match (line ~472), add:

```rust
            Some(Action::Filter) => "Filter",
```

- [ ] **Step 13: Build, test, clippy**

Run: `cargo build && cargo test && cargo clippy`
Manually: press `f`, type a fragment, confirm the tree prunes live; Enter keeps it; `f` then Esc restores.

- [ ] **Step 14: Commit**

```bash
git add src/tree/arena.rs src/keymap.rs src/app.rs src/dispatch.rs src/ui/overlays.rs
git commit -m "feat(tree): live filter mode (f) — prune to matches, keep ancestors"
```

---

## Task 4: Cancellable file ops

**Files:**
- Modify: `src/ops.rs` (cancel flag + worker checks), `src/app.rs` (cancel helper), `src/dispatch.rs` (intercept keys during op), `src/ui/overlays.rs` (hint)
- Test: `src/ops.rs`

**Interfaces:**
- Produces: `OpState.cancel: std::sync::Arc<std::sync::atomic::AtomicBool>`; `OpState::request_cancel(&self)`; workers honor the flag and finish with `Ok("cancelled")`.

- [ ] **Step 1: Add the cancel flag to OpState + constructors**

In `src/ops.rs`, add the import at top:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
```

Add the field to `struct OpState` (line ~38):

```rust
    /// Set true to ask the worker to stop at the next item boundary.
    pub cancel: Arc<AtomicBool>,
```

Add a method in `impl OpState`:

```rust
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
```

- [ ] **Step 2: Thread the flag into spawn_paste**

In `src/ops.rs` `spawn_paste` (line ~103): create the flag, clone it into the worker, store it on the returned `OpState`:

```rust
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
```

Inside the worker's `for src in &srcs_for_worker` loop, check at the top:

```rust
        for src in &srcs_for_worker {
            if cancel_worker.load(Ordering::Relaxed) {
                break;
            }
```

In both `fs_extra` progress closures (copy line ~145, move line ~174), abort when cancelled:

```rust
                        if cancel_worker.load(Ordering::Relaxed) {
                            return fs_extra::dir::TransitProcessResult::Abort;
                        }
                        fs_extra::dir::TransitProcessResult::ContinueOrAbort
```

After the loop, make the finished message cancel-aware (replace the `let msg = if failed.is_empty()` block's success branch tail so a cancel reads clearly). Simplest: before building `msg`, add:

```rust
        let was_cancelled = cancel_worker.load(Ordering::Relaxed);
```

and change the success arm to:

```rust
        let msg = if was_cancelled {
            Ok(format!("cancelled ({} done)", crate::status::human_bytes(done)))
        } else if failed.is_empty() {
            Ok(format!(/* unchanged */))
        } else { /* unchanged */ };
```

Add `cancel,` to the returned `OpState { ... }` literal (line ~224).

- [ ] **Step 3: Thread the flag into spawn_extract**

In `src/ops.rs` `spawn_extract` (line ~239): extract is a single shell-out with no progress stream, so cancel can't interrupt it mid-run; just carry a flag so the type is uniform. Add:

```rust
    let cancel = Arc::new(AtomicBool::new(false));
```

and `cancel,` in the returned `OpState { ... }` (line ~255). (ponytail: extract is one external process; cancel is a no-op until it returns. Killing the child is a future upgrade.)

- [ ] **Step 4: Cancel test**

Add to the `tests` module in `src/ops.rs`:

```rust
    #[test]
    fn request_cancel_sets_flag() {
        use std::sync::atomic::Ordering;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"hello").unwrap();
        let dst = tempfile::tempdir().unwrap();
        let op = spawn_paste(
            vec![d.path().join("a")],
            dst.path().to_path_buf(),
            crate::fs_ops::PasteMode::Copy,
        );
        op.request_cancel();
        assert!(op.cancel.load(Ordering::Relaxed));
    }
```

Run: `cargo test --lib ops` → PASS.

- [ ] **Step 5: App cancel helper**

In `src/app.rs`, add a method in `impl App`:

```rust
    /// Ask an in-flight op to cancel. No-op when idle.
    pub fn cancel_op(&mut self) {
        if let Some(op) = &self.op {
            op.request_cancel();
            self.set_status("cancelling…");
        }
    }
```

- [ ] **Step 6: Intercept keys while an op is in flight**

In `src/dispatch.rs` `handle_key` (line ~238), add at the very top of the function, before the `match app.mode`:

```rust
    // While a long op runs, the only input accepted is cancel (Esc /
    // Ctrl-c). Everything else is ignored so the user can't mutate the
    // tree mid-op. Mirrors the op modal taking render precedence.
    if app.op.is_some() {
        let ctrl_c = k.code == KeyCode::Char('c')
            && k.modifiers.contains(KeyModifiers::CONTROL);
        if k.code == KeyCode::Esc || ctrl_c {
            app.cancel_op();
        }
        return Ok(());
    }
```

- [ ] **Step 7: Overlay hint**

In `src/ui/overlays.rs` `render_op_modal` footer (line ~98-104), change the hint text to mention cancel:

```rust
        Paragraph::new(Span::styled(
            " esc / ctrl-c cancel · runs in background · tree refreshes when done",
            modal::dim_style(theme),
        )),
```

- [ ] **Step 8: Build, test, clippy, commit**

Run: `cargo build && cargo test && cargo clippy`

```bash
git add src/ops.rs src/app.rs src/dispatch.rs src/ui/overlays.rs
git commit -m "feat(ops): cancel an in-flight copy/move with esc/ctrl-c"
```

---

## Task 5: Bulk rename via $EDITOR

**Files:**
- Modify: `src/fs_ops.rs` (pure plan + apply), `src/shell.rs` (editor launch), `src/keymap.rs` (action), `src/dispatch.rs` (handler), `src/app.rs` (none — handler lives in dispatch)
- Test: `src/fs_ops.rs`

**Interfaces:**
- Produces: `fs_ops::plan_bulk_rename(olds: &[String], news: &[String]) -> Result<Vec<(String, String)>, String>` (validates count + collisions, returns only changed pairs); `shell::editor() -> String`; `shell::edit_lines(dir, lines: &[String]) -> io::Result<Vec<String>>` (write temp file, open `$EDITOR`, read back).

- [ ] **Step 1: Write the failing plan test (pure)**

Add to the `tests` module in `src/fs_ops.rs`:

```rust
    #[test]
    fn plan_bulk_rename_validates_and_diffs() {
        // count mismatch -> error
        assert!(plan_bulk_rename(&["a".into(), "b".into()], &["a".into()]).is_err());
        // only changed pairs returned
        let got = plan_bulk_rename(
            &["a.txt".into(), "b.txt".into()],
            &["a.txt".into(), "c.txt".into()],
        ).unwrap();
        assert_eq!(got, vec![("b.txt".to_string(), "c.txt".to_string())]);
        // duplicate new names -> error
        assert!(plan_bulk_rename(
            &["a".into(), "b".into()],
            &["same".into(), "same".into()],
        ).is_err());
        // empty new name -> error
        assert!(plan_bulk_rename(&["a".into()], &["".into()]).is_err());
        // path separator in new name -> error
        assert!(plan_bulk_rename(&["a".into()], &["x/y".into()]).is_err());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib plan_bulk_rename`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement the pure planner**

In `src/fs_ops.rs`, add:

```rust
/// Compute the rename plan from edited names. Validates: equal counts, no
/// empty/`.`/`..`/separator-bearing names, no duplicate targets. Returns
/// only the pairs whose name actually changed. Pure — no filesystem access.
pub fn plan_bulk_rename(
    olds: &[String],
    news: &[String],
) -> Result<Vec<(String, String)>, String> {
    if olds.len() != news.len() {
        return Err(format!(
            "line count mismatch: {} names, {} lines",
            olds.len(),
            news.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for n in news {
        let t = n.trim();
        if t.is_empty() || t == "." || t == ".." || t.contains('/')
            || t.contains(std::path::MAIN_SEPARATOR)
        {
            return Err(format!("invalid name: {n:?}"));
        }
        if !seen.insert(t.to_string()) {
            return Err(format!("duplicate target name: {t}"));
        }
    }
    Ok(olds
        .iter()
        .zip(news.iter())
        .filter(|(o, n)| o.trim() != n.trim())
        .map(|(o, n)| (o.clone(), n.trim().to_string()))
        .collect())
}
```

Run: `cargo test --lib plan_bulk_rename` → PASS.

- [ ] **Step 4: Add the editor helpers in shell.rs**

In `src/shell.rs`, add:

```rust
/// Resolve the user's editor: `$VISUAL`, `$EDITOR`, else `vi`.
pub fn editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

/// Write `lines` to a temp file, open it in `$EDITOR` (TUI suspended),
/// then read the edited lines back. The temp file lives under `dir` so
/// editors that show the path show a sensible one.
pub fn edit_lines(dir: &Path, lines: &[String]) -> io::Result<Vec<String>> {
    use std::io::Write as _;
    let tmp = dir.join(".explorenova-bulk-rename.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        for l in lines {
            writeln!(f, "{l}")?;
        }
    }
    let ed = editor();
    let quoted = crate::command::shell_quote(&tmp.to_string_lossy());
    let r = around_terminal(|| {
        let (sh, args) = shell_argv(&format!("{ed} {quoted}"));
        Command::new(sh).args(args).current_dir(dir).status().map(|_| ())
    });
    let out = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    r?;
    Ok(out.lines().map(|s| s.to_string()).collect())
}
```

- [ ] **Step 5: Add the action + binding**

In `src/keymap.rs`:
- Add `BulkRename,` to `enum Action` (near `Rename`, line ~42).
- `from_name`: `"bulk_rename" => BulkRename,`
- `defaults`: `(BulkRename, vec!["b"]),`

- [ ] **Step 6: Add the dispatch handler**

In `src/dispatch.rs` `handle` (after `Action::Rename` arm, line ~944), add:

```rust
        Action::BulkRename => bulk_rename(app, git_tx),
```

Add the handler function near `trash_targets` (line ~1103):

```rust
fn bulk_rename(app: &mut App, git_tx: &mpsc::Sender<std::path::PathBuf>) {
    let targets = app.targets();
    if targets.is_empty() {
        app.set_status("nothing to rename");
        return;
    }
    // All targets must share a parent dir (rename is in-place).
    let parent = match targets[0].parent() {
        Some(p) => p.to_path_buf(),
        None => {
            app.set_err("cannot rename: no parent dir");
            return;
        }
    };
    if targets.iter().any(|p| p.parent() != Some(parent.as_path())) {
        app.set_err("bulk rename needs all items in one directory");
        return;
    }
    let olds: Vec<String> = targets
        .iter()
        .map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let news = match crate::shell::edit_lines(&parent, &olds) {
        Ok(v) => v,
        Err(e) => {
            app.set_err(format!("editor failed: {e}"));
            app.force_redraw = true;
            return;
        }
    };
    app.force_redraw = true;
    let plan = match crate::fs_ops::plan_bulk_rename(&olds, &news) {
        Ok(p) => p,
        Err(e) => {
            app.set_err(format!("bulk rename aborted: {e}"));
            return;
        }
    };
    if plan.is_empty() {
        app.set_status("no names changed");
        return;
    }
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for (old, new) in &plan {
        match crate::fs_ops::rename(&parent.join(old), new) {
            Ok(_) => ok += 1,
            Err(e) => failed.push(format!("{old}: {e}")),
        }
    }
    app.marks.clear();
    app.mark_self_write([parent.clone()]);
    refresh_in_place(app, git_tx);
    if failed.is_empty() {
        app.set_status(format!("renamed {ok}"));
    } else {
        let preview = failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
        let tail = if failed.len() > 2 { ", …" } else { "" };
        app.set_err(format!("renamed {ok}, {} failed: {preview}{tail}", failed.len()));
    }
}
```

> `fs_ops::rename` already rejects existing targets, so an edit that collides with an untouched file fails that one pair rather than overwriting. ponytail: no two-phase temp renaming, so a *swap* (`a`→`b`, `b`→`a`) will partially fail; rare enough to defer.

- [ ] **Step 7: Build, test, clippy**

Run: `cargo build && cargo test && cargo clippy`
Manually: mark 2-3 files, press `b`, edit names in `$EDITOR`, save+quit, confirm renames + status.

- [ ] **Step 8: Commit**

```bash
git add src/fs_ops.rs src/shell.rs src/keymap.rs src/dispatch.rs
git commit -m "feat(rename): bulk rename marked files via \$EDITOR (b)"
```

---

## Task 6: Content search (ripgrep)

**Files:**
- Modify: `src/search.rs` (run + parse), `src/keymap.rs` (action), `src/dispatch.rs` (prompt + run)
- Test: `src/search.rs`

**Interfaces:**
- Produces: `search::parse_rg(stdout: &str, root: &Path) -> Vec<PathBuf>` (pure); `search::content_search(root: &Path, query: &str) -> Result<Vec<PathBuf>, String>` (spawns `rg`, maps absence/no-match to a message).

- [ ] **Step 1: Write the failing parse test (pure)**

Add to the `tests` module in `src/search.rs`:

```rust
    #[test]
    fn parse_rg_joins_relative_and_dedups_absolute() {
        let root = Path::new("/proj");
        let out = "src/main.rs\nsrc/main.rs\n/proj/README.md\n\n";
        let got = parse_rg(out, root);
        assert_eq!(got, vec![
            PathBuf::from("/proj/src/main.rs"),
            PathBuf::from("/proj/README.md"),
        ]);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib parse_rg`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement parse + run**

In `src/search.rs`, add:

```rust
use std::collections::HashSet;
use std::process::Command;

/// Turn `rg --files-with-matches` stdout into absolute, de-duplicated
/// paths. Relative lines are joined onto `root`; blank lines dropped.
pub fn parse_rg(stdout: &str, root: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let p = Path::new(line);
        let abs = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
        if seen.insert(abs.clone()) {
            out.push(abs);
        }
    }
    out
}

/// Run ripgrep for files containing `query` under `root`. Errors carry a
/// user-facing message (rg missing, or no matches).
pub fn content_search(root: &Path, query: &str) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("rg")
        .arg("--files-with-matches")
        .arg("--color=never")
        .arg("--")
        .arg(query)
        .current_dir(root)
        .output()
        .map_err(|e| format!("ripgrep (rg) not found: {e}"))?;
    // rg exit code 1 == no matches (not an error for us).
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "rg failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let paths = parse_rg(&String::from_utf8_lossy(&output.stdout), root);
    if paths.is_empty() {
        return Err("no matches".to_string());
    }
    Ok(paths)
}
```

> ponytail: `content_search` blocks the UI thread for the rg run. rg is fast; on a huge tree this is a brief freeze. Move to a worker thread + channel if it bites.

Run: `cargo test --lib parse_rg` → PASS.

- [ ] **Step 4: Add the action + binding**

In `src/keymap.rs`:
- `enum Action`: add `ContentSearch,` (near `Search`, line ~46).
- `from_name`: `"content_search" => ContentSearch,`
- `defaults`: `(ContentSearch, vec!["F"]),`

- [ ] **Step 5: Prompt + run in dispatch**

In `src/dispatch.rs` `handle` (after `Action::SearchPrev`, line ~921):

```rust
        Action::ContentSearch => {
            app.pending = Some(Action::ContentSearch);
            app.enter_mode(Mode::Input, "");
        }
```

In `text_input_enter` `Mode::Input` match (line ~697), add an arm that runs rg and loads the results into the existing jump-result set so `n`/`N` cycle:

```rust
            Some(Action::ContentSearch) => {
                let query = app.input.value();
                app.mode = Mode::Normal;
                if query.is_empty() {
                    return;
                }
                let root = app.arena.nodes[app.arena.root].path.clone();
                match crate::search::content_search(&root, &query) {
                    Ok(paths) => {
                        let n = paths.len();
                        app.search_results = paths.into_iter().map(|p| (p, Vec::new())).collect();
                        app.search_idx = 0;
                        app.jump_to_search();
                        app.set_status(format!("1/{n} matches (content)"));
                    }
                    Err(e) => app.set_err(e),
                }
            }
```

- [ ] **Step 6: Input overlay title**

In `src/ui/overlays.rs` `render_input` title match (line ~472), add:

```rust
            Some(Action::ContentSearch) => "Search contents (rg)",
```

- [ ] **Step 7: Build, test, clippy**

Run: `cargo build && cargo test && cargo clippy`
Manually (needs `rg` installed): press `F`, type a string in a code dir, confirm the cursor jumps to the first file containing it; `n`/`N` cycle.

- [ ] **Step 8: Commit**

```bash
git add src/search.rs src/keymap.rs src/dispatch.rs src/ui/overlays.rs
git commit -m "feat(search): ripgrep content search (F), reuses n/N jump cycle"
```

---

## Task 7: zoxide jump + cwd-on-exit

**Files:**
- Create: `src/zoxide.rs`
- Modify: `src/lib.rs`, `src/cli.rs` (`--cwd-file`), `src/main.rs` (write cwd on quit), `src/keymap.rs` (action), `src/dispatch.rs` (prompt + jump + add-on-cd)
- Test: `src/zoxide.rs`, `src/cli.rs`

**Interfaces:**
- Produces: `zoxide::query(q: &str) -> Option<PathBuf>` (best result, or None); `zoxide::add(path: &Path)` (fire-and-forget); `Args.cwd_file: Option<PathBuf>`.

- [ ] **Step 1: Create the zoxide module**

Create `src/zoxide.rs`:

```rust
//! Thin zoxide integration: query the database for a `z`-style jump and
//! record visited dirs so the database stays useful. Both degrade to a
//! no-op / None when the `zoxide` binary is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Best matching directory for `query`, or None (no match / zoxide absent).
pub fn query(query: &str) -> Option<PathBuf> {
    let out = Command::new("zoxide")
        .arg("query")
        .arg("--")
        .arg(query)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}

/// Record a visited directory (fire-and-forget; ignores all errors).
pub fn add(path: &Path) {
    let _ = Command::new("zoxide").arg("add").arg("--").arg(path).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_missing_binary_is_none_safe() {
        // Can't assume zoxide is installed in CI; just assert the call
        // never panics and returns an Option. (No DB match expected for a
        // random token.)
        let _ = super::query("zzz-nonexistent-token-zzz");
    }
}
```

Register in `src/lib.rs` (after `pub mod ui;` won't be alphabetical; put near `pub mod state;`):

```rust
pub mod zoxide;
```

- [ ] **Step 2: Add the `--cwd-file` flag**

In `src/cli.rs` `struct Args` (line ~6):

```rust
    /// On quit, write the final directory to this file (for a shell
    /// wrapper that cd's there). See README "Shell integration".
    #[arg(long = "cwd-file")]
    pub cwd_file: Option<PathBuf>,
```

Add a parse test in the `tests` module:

```rust
    #[test]
    fn parses_cwd_file_flag() {
        let a = Args::try_parse_from(["explorenova", "--cwd-file", "/tmp/x"]).unwrap();
        assert_eq!(a.cwd_file, Some(PathBuf::from("/tmp/x")));
    }
```

Run: `cargo test --lib cli` → PASS.

- [ ] **Step 3: Write the cwd file on quit**

In `src/main.rs`, after the state is written (after line ~420, before `Ok(())`):

```rust
    // Shell-integration: drop the final directory so a wrapper can cd there.
    if let Some(cwd_file) = args.cwd_file.as_ref() {
        let _ = std::fs::write(
            cwd_file,
            app.arena.nodes[app.arena.root].path.to_string_lossy().as_bytes(),
        );
    }
```

- [ ] **Step 4: Record visited dirs for zoxide on reroot**

In `src/dispatch.rs` `reroot` (in the `Ok(mut na)` arm after `app.set_status(...)`, line ~75): record the new root so `z` has data:

```rust
            crate::zoxide::add(&app.arena.nodes[app.arena.root].path.clone());
```

> ponytail: spawns `zoxide add` per cd (detached, errors ignored). That's how zoxide's own shell hooks work; the cost is one short-lived process per directory change.

- [ ] **Step 5: Add the action + binding**

In `src/keymap.rs`:
- `enum Action`: `ZoxideJump,` (near `JumpHome`, line ~59).
- `from_name`: `"zoxide_jump" => ZoxideJump,`
- `defaults`: `(ZoxideJump, vec!["z"]),`

- [ ] **Step 6: Prompt + jump in dispatch**

In `src/dispatch.rs` `handle` (after `Action::JumpHome`, line ~885):

```rust
        Action::ZoxideJump => {
            app.pending = Some(Action::ZoxideJump);
            app.enter_mode(Mode::Input, "");
        }
```

In `text_input_enter` `Mode::Input` match, add:

```rust
            Some(Action::ZoxideJump) => {
                let q = app.input.value();
                app.mode = Mode::Normal;
                if q.is_empty() {
                    return;
                }
                match crate::zoxide::query(&q) {
                    Some(dir) if dir.is_dir() => reroot(app, &dir, git_tx),
                    Some(_) => app.set_err("zoxide: result is not a directory"),
                    None => app.set_err("zoxide: no match (is zoxide installed?)"),
                }
            }
```

- [ ] **Step 7: Input overlay title**

In `src/ui/overlays.rs` `render_input` title match:

```rust
            Some(Action::ZoxideJump) => "Jump (zoxide)",
```

- [ ] **Step 8: Build, test, clippy**

Run: `cargo build && cargo test && cargo clippy`

- [ ] **Step 9: Commit**

```bash
git add src/zoxide.rs src/lib.rs src/cli.rs src/main.rs src/keymap.rs src/dispatch.rs src/ui/overlays.rs
git commit -m "feat(nav): zoxide jump (z) + --cwd-file for shell cd-on-exit"
```

---

## Task 8: Documentation (README + help)

**Files:**
- Modify: `README.md`

**Interfaces:** none.

- [ ] **Step 1: Document keys + config + shell wrapper**

In `README.md`, add to the keybindings/usage section:

```markdown
### Power features

- `f` — **filter** the tree to fuzzy matches (ancestors kept). Enter keeps it, Esc clears.
- `F` — **content search** with ripgrep (`rg`); jumps to the first file containing the query, `n`/`N` cycle.
- `z` — **zoxide jump**: type a query, jump to the best-ranked directory (needs `zoxide`).
- `b` — **bulk rename** the marked files in `$EDITOR` (one name per line).
- `L` — cycle the **linemode** column (none → size → mtime → perms).
- During a copy/move, `esc` or `ctrl-c` **cancels** the op.

### Openers

    [openers]
    md = "glow {file}"
    html = "firefox {file}"

A file whose extension has no rule opens with the OS default app. Same
`{file}`/`{paths}`/`{dir}`/`{name}`/`{ext}` placeholders as `[commands]`.

### Shell integration (cd on exit)

Launch with `--cwd-file` and have a wrapper cd to the directory you quit in.
Add to `~/.bashrc` / `~/.zshrc`:

    en() {
      local tmp; tmp="$(mktemp)"
      explorenova --cwd-file "$tmp" "$@"
      local dir; dir="$(cat "$tmp")"; rm -f "$tmp"
      [ -n "$dir" ] && cd "$dir"
    }

fish (`~/.config/fish/functions/en.fish`):

    function en
      set -l tmp (mktemp)
      explorenova --cwd-file $tmp $argv
      set -l dir (cat $tmp); rm -f $tmp
      test -n "$dir" && cd "$dir"
    end

Now `en` opens explorenova and drops you in the last directory on quit.
```

- [ ] **Step 2: Verify + commit**

Run: `rg -n "Power features" README.md` → present.

```bash
git add README.md
git commit -m "docs: filter, content search, zoxide, bulk rename, openers, linemodes, cd-on-exit"
```

---

## Final verification

- [ ] **Full suite**

Run: `cargo test`
Expected: all unit + integration tests pass.

- [ ] **Clippy clean**

Run: `cargo clippy --all-targets`
Expected: no warnings.

- [ ] **Help overlay sanity**

`?` lists the new keys (it builds from the keymap via `help::build_entries`, so they appear automatically). Confirm `f`/`F`/`z`/`b`/`L` show.

- [ ] **Manual dogfood**

`cargo run -- .` — exercise: `f` filter live + Esc; `F` rg jump (in a code dir); `L` cycle column; mark files + `b` bulk rename; start a big copy + `esc` cancel; add `[openers] md = "..."` and open a `.md`; build the `en` wrapper and confirm cd-on-exit.

---

## Self-review notes

- **Spec coverage:** safety fixes C1/C2 (Task 0), linemodes (Task 1), openers (Task 2), filter (Task 3), op-cancel UX (Task 4), bulk rename (Task 5), content search (Task 6), zoxide+cwd (Task 7), docs (Task 8). All seven requested features plus the two audit bugs covered; still deferred per "leave rest": N-tabs, mouse, plugins, search-highlight paint.
- **No new Mode:** filter/content-search/zoxide all reuse `Mode::Input` + `app.pending`, so `keymap::resolve`, the main-loop dialog-derive, and overlay routing need no new arms — only `render_input` gains titles.
- **Key collisions:** `f F z b L` are all currently unbound (verified against `Keymap::defaults`); `b`/`z` are not prefixes of any chord. All rebindable via `[keys]`.
- **Type consistency:** `LineMode` used identically in `state.rs`/`app.rs`/`panes.rs`; `OpState.cancel` Arc shared by worker + `request_cancel`; `search::parse_rg`/`content_search` and `zoxide::query`/`add` signatures match their call sites.
