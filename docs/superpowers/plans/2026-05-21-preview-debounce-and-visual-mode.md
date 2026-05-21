# Preview Debounce + Visual Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop image-preview decode from stuttering scroll, and add a yazi-style `v` visual mode that marks rows the cursor crosses.

**Architecture:** A `cursor_moved_at: Instant` on `App` plus a UI gate skips image decode within 150 ms of any cursor change. A `visual_mode: bool` plus an `Action::ToggleVisualMode` (bound `v`) auto-marks the new cursor row after each navigation. `Esc` clears marks and exits visual.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, ratatui-image 4.2, image 0.25.

---

## File Map

| File | Change |
|---|---|
| `src/app.rs` | Add `visual_mode: bool`, `cursor_moved_at: Instant`; init in `App::new`; new `toggle_visual_mode`, `mark_cursor`; extend `Action::Cancel` arm to flip visual_mode off. |
| `src/keymap.rs` | Add `Action::ToggleVisualMode`, default chord `"v"`, name `"toggle_visual_mode"`. |
| `src/main.rs` | `Action::ToggleVisualMode` arm in `handle()`; post-dispatch hook that detects `selected` changed → `cursor_moved_at = now` + `mark_cursor()` if visual; clamp `event::poll` to remaining debounce. |
| `src/ui/mod.rs` | Gate `render_image` in the `PreviewKind::Image` arm on `cache-hit OR debounce-elapsed`; otherwise draw `"(loading…)"`. Footer prepends `-- VISUAL --` span when `app.visual_mode`. |

---

## Task 1: Add `visual_mode` + `cursor_moved_at` fields and helper methods on App

**Files:**
- Modify: `src/app.rs` (struct fields + `App::new` + new methods + extend `Cancel` arm + tests)

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `src/app.rs`:

```rust
#[test]
fn toggle_visual_mode_marks_cursor_on_enter() {
    let mut a = app();
    assert!(!a.visual_mode);
    let p = a.arena.nodes[a.visible[a.selected]].path.clone();
    a.toggle_visual_mode();
    assert!(a.visual_mode);
    assert!(a.marks.contains(&p));
    assert_eq!(a.marks.len(), 1);
}

#[test]
fn toggle_visual_mode_leaves_marks_on_exit() {
    let mut a = app();
    a.toggle_visual_mode();              // enter, marks 1
    a.apply(Action::Down);
    a.mark_cursor();                     // simulate post-dispatch hook
    assert_eq!(a.marks.len(), 2);
    a.toggle_visual_mode();              // exit
    assert!(!a.visual_mode);
    assert_eq!(a.marks.len(), 2, "marks must survive exit");
}

#[test]
fn cancel_exits_visual_and_clears_marks() {
    let mut a = app();
    a.toggle_visual_mode();
    a.apply(Action::Down);
    a.mark_cursor();
    assert!(a.visual_mode && !a.marks.is_empty());
    a.apply(Action::Cancel);
    assert!(!a.visual_mode);
    assert!(a.marks.is_empty());
}

#[test]
fn mark_cursor_is_idempotent() {
    let mut a = app();
    a.mark_cursor();
    a.mark_cursor();
    assert_eq!(a.marks.len(), 1);
}

#[test]
fn mark_cursor_no_op_when_visible_empty() {
    let mut a = app();
    a.visible.clear();
    a.mark_cursor();
    assert!(a.marks.is_empty());
}
```

- [ ] **Step 2: Run tests to verify failures**

Run: `cargo test --lib app::tests::toggle_visual_mode_marks_cursor_on_enter app::tests::toggle_visual_mode_leaves_marks_on_exit app::tests::cancel_exits_visual_and_clears_marks app::tests::mark_cursor_is_idempotent app::tests::mark_cursor_no_op_when_visible_empty`
Expected: 5 failures (compile error: `visual_mode` / `toggle_visual_mode` / `mark_cursor` not found).

- [ ] **Step 3: Add fields to App struct**

In `src/app.rs`, add to `pub struct App { … }` (after existing fields, before closing `}`):

```rust
    pub visual_mode: bool,
    pub cursor_moved_at: std::time::Instant,
```

- [ ] **Step 4: Initialise in `App::new`**

In `App::new`, add to the struct literal (anywhere among the field initialisers):

```rust
            visual_mode: false,
            cursor_moved_at: std::time::Instant::now(),
```

- [ ] **Step 5: Add `toggle_visual_mode` and `mark_cursor` methods**

In `impl App`, add (alongside `toggle_mark`, `clear_marks`, etc.):

```rust
    /// Flip visual mode. When entering, mark the cursor row so a bare
    /// `v` press selects the current item even without movement.
    pub fn toggle_visual_mode(&mut self) {
        self.visual_mode = !self.visual_mode;
        if self.visual_mode {
            self.mark_cursor();
        }
    }

    /// Insert the cursor row's path into `marks` (idempotent; never
    /// removes). Used by visual-mode auto-mark on movement.
    pub fn mark_cursor(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            self.marks.insert(self.arena.nodes[id].path.clone());
        }
    }
```

- [ ] **Step 6: Extend `Action::Cancel` arm to exit visual mode**

In `App::apply`'s `Action::Cancel` arm, change:

```rust
            Action::Cancel => {
                self.mode = Mode::Normal;
                self.marks.clear();
            }
```

to:

```rust
            Action::Cancel => {
                self.mode = Mode::Normal;
                self.marks.clear();
                self.visual_mode = false;
            }
```

- [ ] **Step 7: Run tests to verify pass**

Run: `cargo test --lib app::tests`
Expected: all `app::tests` pass (including the 5 new ones), no other test regressions.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs
git commit -m "feat: App.visual_mode + cursor_moved_at + toggle/mark helpers"
```

---

## Task 2: Add `Action::ToggleVisualMode` keybinding

**Files:**
- Modify: `src/keymap.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/keymap.rs`:

```rust
#[test]
fn v_resolves_to_toggle_visual_mode() {
    let km = Keymap::build(&std::collections::BTreeMap::new());
    assert_eq!(km.resolve(Mode::Normal, "v"), Some(Action::ToggleVisualMode));
    assert_eq!(
        Action::from_name("toggle_visual_mode"),
        Some(Action::ToggleVisualMode)
    );
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --lib keymap::tests::v_resolves_to_toggle_visual_mode`
Expected: compile error (`Action::ToggleVisualMode` not found).

- [ ] **Step 3: Add variant to `Action` enum**

In `src/keymap.rs`, add to `pub enum Action { … }` (e.g. just after `SelectAllVisible,`):

```rust
    ToggleVisualMode,
```

- [ ] **Step 4: Register name in `Action::from_name`**

Add to the `match s` block in `from_name` (with the other `_` cases):

```rust
            "toggle_visual_mode" => ToggleVisualMode,
```

- [ ] **Step 5: Add default binding**

In `Keymap::defaults()`, add (e.g. just after `(SelectAllVisible, vec!["ctrl-a"]),`):

```rust
            (ToggleVisualMode, vec!["v"]),
```

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test --lib keymap::tests`
Expected: all `keymap::tests` pass.

- [ ] **Step 7: Commit**

```bash
git add src/keymap.rs
git commit -m "feat: Action::ToggleVisualMode bound to v"
```

---

## Task 3: Wire `ToggleVisualMode` action + post-dispatch hook in main.rs

**Files:**
- Modify: `src/main.rs` (handle() match + main loop dispatch wrapper)

**Context:** The Normal-mode dispatch path is in `src/main.rs` around line 193. Pure-state actions go through `app.apply(action)` via `Keymap::resolve`; the `handle()` function (line ~428) routes IO-bearing actions. We need to (a) add a `ToggleVisualMode` arm in `handle()` (since `App::apply` already covers pure state but `handle()` is the actual dispatch entry — `apply` is called from inside the catch-all `_` of `handle()`), and (b) wrap the dispatch with a `prev_selected` snapshot + post-dispatch hook for `cursor_moved_at` and visual auto-mark.

- [ ] **Step 1: Read current handle() routing for Normal-mode actions**

Run: `grep -n "fn handle\|app.apply\|action =>" src/main.rs | head -30`

Inspect the `match action` block in `handle()`. Confirm how pure-state actions reach `app.apply`. (Expected: an explicit `_ => app.apply(action)` catch-all or per-arm calls.)

- [ ] **Step 2: Add `ToggleVisualMode` arm in handle()**

In `fn handle()` in `src/main.rs`, add a new arm (alongside `Action::Trash`, before the catch-all):

```rust
        Action::ToggleVisualMode => app.toggle_visual_mode(),
```

- [ ] **Step 3: Add post-dispatch hook around the Normal-mode action call**

Locate the Normal-mode chord-resolution block in the event loop (around line 193). The relevant lines look like:

```rust
                Mode::Normal => {
                    let Some(tok) = key_token(&k) else { continue };
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
```

Change the `if let Some(action) = …` arm to snapshot and post-update:

```rust
                    if let Some(action) = keymap.resolve(Mode::Normal, &chord) {
                        chord.clear();
                        let prev_selected = app.selected;
                        handle(&mut app, action, &start, &git_tx)?;
                        if app.selected != prev_selected {
                            app.cursor_moved_at = std::time::Instant::now();
                            if app.visual_mode {
                                app.mark_cursor();
                            }
                        }
                    } else if keymap.is_prefix(&chord) {
```

- [ ] **Step 4: Apply the same hook to the other dispatch site (search/PathInput jumps)**

The `Mode::Search | Mode::Input` arm's `KeyCode::Enter` handler also moves the selected index (via `run_search`, `PathInput` reveal, etc.). To keep the debounce honest for jumps, wrap the entire modal-dispatch `if let Event::Key(k) = event::read()? { … }` block as follows:

Locate the `if let Event::Key(k) = event::read()? {` line. Replace the block with:

```rust
        if let Event::Key(k) = event::read()? {
            let prev_selected_outer = app.selected;
            // … existing match app.mode { … } block, UNCHANGED in body …
            if app.selected != prev_selected_outer {
                app.cursor_moved_at = std::time::Instant::now();
                if app.visual_mode {
                    app.mark_cursor();
                }
            }
        }
```

Then **remove** the inner `prev_selected` / cursor_moved_at update from the Normal arm you added in Step 3 — the outer wrapper now covers it. (Double-counting is harmless but redundant; remove the inner copy to keep one code path.)

Final Normal arm (after this cleanup):

```rust
                    if let Some(action) = keymap.resolve(Mode::Normal, &chord) {
                        chord.clear();
                        handle(&mut app, action, &start, &git_tx)?;
                    } else if keymap.is_prefix(&chord) {
```

- [ ] **Step 5: Clamp `event::poll` to the remaining debounce**

Locate this line (around line 185):

```rust
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
```

Replace with:

```rust
        const PREVIEW_DEBOUNCE_MS: u64 = 150;
        let poll_ms = {
            let elapsed = app.cursor_moved_at.elapsed().as_millis() as u64;
            let remaining = PREVIEW_DEBOUNCE_MS.saturating_sub(elapsed);
            // 200ms is the existing default; never wait longer than that
            // (so fs/git channels still get drained promptly).
            // When debounce is already past, remaining == 0 and we use 200.
            if remaining == 0 { 200 } else { remaining.min(200) }
        };
        if !event::poll(Duration::from_millis(poll_ms))? {
            continue;
        }
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --release 2>&1 | tail -20`
Expected: clean build, no warnings introduced by these edits.

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: full suite passes (88 + 6 new = 94 expected).

- [ ] **Step 8: Commit**

```bash
git add src/main.rs
git commit -m "feat: ToggleVisualMode dispatch + cursor_moved_at hook + debounce poll clamp"
```

---

## Task 4: Gate image-preview decode + add visual-mode footer indicator in UI

**Files:**
- Modify: `src/ui/mod.rs` (preview arm gate + footer span)

- [ ] **Step 1: Inspect current footer composition**

Run: `grep -n "marks_segment\|prompt_area\|footer" src/ui/mod.rs`

The footer Paragraph builds a single `String` today. To prepend a coloured `-- VISUAL --` span we need a `Line` with spans.

- [ ] **Step 2: Add debounce gate in the `PreviewKind::Image` arm**

Locate this block in `render_frame` (around line 231):

```rust
                        PreviewKind::Image(p) => {
                            let block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(ratatui::widgets::BorderType::Rounded)
                                .title("preview");
                            let inner = block.inner(area);
                            f.render_widget(block, area);
                            render_image(f, inner, picker, img_cache, &p);
                        }
```

Replace with:

```rust
                        PreviewKind::Image(p) => {
                            const PREVIEW_DEBOUNCE_MS: u128 = 150;
                            let block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(ratatui::widgets::BorderType::Rounded)
                                .title("preview");
                            let inner = block.inner(area);
                            f.render_widget(block, area);
                            let already_decoded = img_cache
                                .key
                                .as_ref()
                                .map(|(k, _)| k == &p)
                                .unwrap_or(false);
                            let elapsed =
                                app.cursor_moved_at.elapsed().as_millis()
                                    >= PREVIEW_DEBOUNCE_MS;
                            if already_decoded || elapsed {
                                render_image(f, inner, picker, img_cache, &p);
                            } else {
                                f.render_widget(
                                    Paragraph::new("(loading…)"),
                                    inner,
                                );
                            }
                        }
```

Note: this requires `ImageCache.key` to be readable from `ui/mod.rs`. It's already `pub` (`pub struct ImageCache { key: …, proto: … }` — fields are private). Make `key` accessible by either:

- (a) Adding `pub` to the `key` field in the existing `ImageCache` struct (preferred — keeps the gate in one place), or
- (b) Adding a `pub fn matches(&self, p: &Path) -> bool` accessor.

Pick **(a)**: change `key: Option<…>` to `pub key: Option<…>` in the `ImageCache` struct definition (same file, around line 93).

- [ ] **Step 3: Replace footer Paragraph with a Line of spans**

Locate the footer block (around line 272-onwards). The existing code is roughly:

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
        app.status,
    );
    f.render_widget(Paragraph::new(footer), prompt_area);
```

Replace with:

```rust
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let mut spans: Vec<Span> = Vec::new();
    if app.visual_mode {
        spans.push(Span::styled(
            "-- VISUAL --  ",
            Style::default()
                .fg(theme.mark)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(format!(
        "{}/{}  ",
        app.selected + 1,
        app.visible.len().max(1),
    )));
    if !app.marks.is_empty() {
        spans.push(Span::raw(format!("{} selected  ", app.marks.len())));
    }
    spans.push(Span::raw(app.status.clone()));
    f.render_widget(Paragraph::new(Line::from(spans)), prompt_area);
```

(If the existing footer code uses a slightly different shape — e.g. `format!` already inlined or `marks_segment` named differently — adapt the replacement to wrap whatever the current composition is. The key changes are: leading `-- VISUAL --` span when `visual_mode`, and rendering via `Line::from(spans)` not a plain `String`.)

- [ ] **Step 4: Verify compile + clippy**

Run: `cargo build --release 2>&1 | tail -20`
Then: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -30`
Expected: clean build, no new clippy warnings.

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: all pre-existing tests + the 6 new app/keymap tests pass.

- [ ] **Step 6: Manual smoke test**

```bash
cargo build --release
./target/release/explorenova ~/Pictures
```

Verify:
- Hold `j` through 10+ images — scroll is smooth, preview shows `(loading…)` briefly then renders ~150 ms after release.
- Navigate back to a previously-viewed image — instant (cache hit).
- Press `v` → footer shows `-- VISUAL --  1 selected`, current row has `*` prefix in accent colour.
- `j j j` → 4 marked, footer `-- VISUAL --  4 selected`.
- `v` again → `-- VISUAL --` disappears, marks stay (`4 selected`).
- Move elsewhere, `v`, mark more → adds to existing set.
- `Esc` → all marks cleared, footer back to `N/M`.
- `dd` then move + `p` → confirms cut/paste still respects the mark set (regression check).

Quit with `q`.

- [ ] **Step 7: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: debounce image-preview decode 150ms; -- VISUAL -- footer indicator"
```

---

## Task 5: Final whole-branch review (no code changes unless review finds defects)

**Files:** none (verification only).

- [ ] **Step 1: Run full test suite once more**

Run: `cargo test 2>&1 | tail -5`
Expected: all green.

- [ ] **Step 2: Run clippy + release build**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`
Run: `cargo build --release 2>&1 | tail -5`
Expected: zero errors, zero new warnings.

- [ ] **Step 3: Inspect branch commits**

Run: `git log --oneline master..HEAD`
Expected: 4 feature commits (one per Task 1–4) plus the earlier spec commit.

- [ ] **Step 4: Hand off to finishing-a-development-branch**

Tests green + clippy clean + manual smoke OK → invoke
`superpowers:finishing-a-development-branch` to present merge options.
