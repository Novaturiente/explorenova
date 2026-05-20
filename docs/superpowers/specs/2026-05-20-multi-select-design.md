# explorenova — Multi-Select & Batch Operations

**Date:** 2026-05-20
**Status:** Approved (pending written-spec review)
**Builds on:** shipped explorenova on `master`.

## Problem

Today every destructive/clipboard action (`yy`/`dd`/`x`/`X`/`p`) operates
on exactly the cursor row. Users cannot pick out several files or
folders and act on them as a batch — a basic file-manager affordance
that yazi, ranger, and pcmanfm all expose.

## Decisions (locked during brainstorming)

- **Semantic rule:** existing single-item bindings act on the **mark
  set** when it is non-empty, else on the cursor row. No new copy/cut/
  delete/trash keys.
- **Toggle:** `Space` toggles selection on the cursor row. Cursor and
  selection are independent — `j`/`k` never modify the set.
- **Visual mark:** a leading `*` span and the name in an accent colour
  (`theme.mark`). Selection bar (cursor) and mark are independent and
  may visually combine on the same row.
- **Selection management:** `Ctrl-a` marks every visible row in the
  tree; `Esc` (= `Action::Cancel`) in Normal mode clears the set.
- **Out of scope (YAGNI):** visual-mode range select (`V`+j/k), per-pane
  marks, mark counts/sizes in the footer, undo, mark persistence across
  restart, mixed copy/cut in one clipboard.

## Architecture

| Unit | Responsibility |
|---|---|
| `src/app.rs` | New `pub marks: std::collections::BTreeSet<PathBuf>` keyed by path (paths are stable across arena rebuilds; NodeIds are not). New methods: `toggle_mark`, `clear_marks`, `select_all_visible`, `targets(&self) -> Vec<PathBuf>`. Existing `App::apply(Action::Cancel)` gains a `marks.clear()` side effect. `clipboard` becomes `Option<(Vec<PathBuf>, bool)>`. |
| `src/keymap.rs` | New `Action::ToggleMark` (default `space`), `Action::SelectAllVisible` (default `ctrl-a`). `Cancel`'s default binding (`esc`) is unchanged. |
| `src/main.rs::handle()` | `ToggleMark` / `SelectAllVisible` arms route to the App methods. Copy / Cut / Paste / Trash / PermanentDelete arms iterate `app.targets()` and collect per-entry results. |
| `src/app.rs::confirm_yes` | Capture `targets()` into a local snapshot **before** any mutation, iterate the snapshot for `permanent_delete`, surface a "deleted N (M failed: …)" status, clear marks at the end. |
| `src/ui/theme.rs` | New `pub mark: Color::Yellow` with a fixed default. |
| `src/ui/panes.rs` | `tree_row` gains `marked: bool`. When `marked`: prepend a `*` span styled `fg(theme.mark)`; **override** the name span to `fg(theme.mark)` (regardless of classified type colour). Marker / git / icon styling unchanged. |
| `src/ui/mod.rs` | While building tree `ListItem`s, compute `marked = app.marks.contains(&app.arena.nodes[id].path)` and forward. Footer format becomes `"{sel+1}/{total}  {marks_segment}{status}"` where `marks_segment = format!("{} selected  ", app.marks.len())` when `marks.len() > 0`, else `""`. |

### Key types

```rust
// app.rs
pub struct App {
    // …existing fields…
    pub marks: std::collections::BTreeSet<std::path::PathBuf>,
    pub clipboard: Option<(Vec<std::path::PathBuf>, bool)>, // (paths, is_cut)
}

impl App {
    pub fn toggle_mark(&mut self) { /* sel path: in→remove, out→insert */ }
    pub fn clear_marks(&mut self) { self.marks.clear(); }
    pub fn select_all_visible(&mut self) { /* every visible path in */ }
    /// Single chokepoint for the "selection-if-any-else-cursor" rule.
    pub fn targets(&self) -> Vec<std::path::PathBuf> {
        if !self.marks.is_empty() {
            self.marks.iter().cloned().collect()
        } else if let Some(&id) = self.visible.get(self.selected) {
            vec![self.arena.nodes[id].path.clone()]
        } else {
            Vec::new()
        }
    }
}
```

## Behaviours

### Toggle / clear / select-all

- `Space` (Normal): toggle the cursor row's path in `marks`. No-op if
  `visible` is empty. Cursor unchanged.
- `Ctrl-a` (Normal): insert every visible row's path into `marks`
  (additive; existing marks remain).
- `Esc` (Normal → `Action::Cancel` via the existing default binding):
  clears `marks`. (Non-Normal modes handle `Esc` directly in `main.rs`
  and never reach `App::apply` — only Normal-mode `Cancel` clears
  marks.)
- Marks survive arena rebuilds (ascend, fs-watch refresh, ToggleHidden)
  because they are keyed by `PathBuf`. After a rebuild, any mark whose
  path is no longer in `visible` is still in `marks` but invisible — if
  the user runs a batch op, `targets()` includes it; if the path no
  longer exists on disk the per-entry op records that as a failure.

### Batch operations (in `main.rs::handle`)

For each of `Copy` / `Cut` / `Paste` / `Trash` / `PermanentDelete`:

1. Snapshot `targets = app.targets()`. If empty, status `"nothing to <verb>"`, return.
2. Iterate. For destructive ops collect failures into a `Vec<String>` of
   `format!("{}", p.display())`.
3. After the loop, set `app.status`:
   - All ok → `"<verb> {n}"` (n=1 → singular form, e.g. `"deleted README.md"`).
   - Some failed → `"<verb> {ok}/{n} ({m} failed: <first 2 paths>{, …}?)"`.
4. Clear marks for destructive batches (`Trash`, `PermanentDelete`) and
   after a successful `Cut` paste (the `Cut` itself does not clear
   marks; the *paste* of a cut clipboard does, and additionally drops
   the clipboard). `Copy` does not clear marks; `Paste` does not clear
   marks (the source marks are independent of the destination).

Specifics:

- **Copy** (`yy`): `app.clipboard = Some((targets, false))`. Status
  `"copied N"`. Marks **not** cleared (yazi behaviour: yy retains so the
  user can also delete or move the same set after).
- **Cut** (`dd`): `app.clipboard = Some((targets, true))`. Status
  `"cut N"`. Marks **not** cleared.
- **Paste** (`p`): if clipboard is `None` → status `"nothing to paste"`.
  Else compute `dst_dir` exactly as today (cursor row's dir if dir,
  else parent, else cwd). For each `src` in the clipboard list call
  `fs_ops::paste(&src, &dst_dir, mode)`. If `is_cut`: the clipboard is
  cleared after the loop (one paste consumes a cut), AND the marks
  whose path equals a successfully-moved source are removed (those
  paths no longer exist). Copy-paste leaves marks and clipboard alone.
- **Trash** (`x`/`Del`): per path `fs_ops::trash_path`. Marks cleared
  unconditionally after the loop (whether or not every entry trashed
  successfully — failed paths remain on disk, user can re-mark).
- **PermanentDelete** (`X`): set `app.pending = Some(PermanentDelete)`;
  `app.mode = Mode::Confirm`; `app.status = "permanently delete N? y/N"`
  (n=1 → `permanently delete <name>? y/N`). `app.confirm_yes` captures
  `targets()` as its first action, iterates the snapshot with
  `fs_ops::permanent_delete`, builds the result message, clears marks
  + pending + mode.

### Render

- `tree_row(arena, id, selected, theme, icons, marked)`:
  - If `marked`: `spans.push(Span::styled("* ", Style::default().fg(theme.mark)))` **before** the indent of the icon block (so the mark sits at the left edge regardless of tree depth). The name span uses `fg(theme.mark)` (override). Marker and git glyph rendering are unchanged.
  - If `!marked`: identical to today's render.
- The `List`'s `highlight_style = bg(theme.selected_bg)` continues to
  paint the cursor bar across the full row; the `*` and accent fg are
  drawn on top of it for a row that is both marked AND selected.
- Footer: `format!("{}/{}  {}{}", sel+1, total.max(1), marks_segment, status)` where `marks_segment = format!("{} selected  ", n)` when `n > 0` else `""`.

## Error Handling

- Batch ops never panic on per-entry failure; failures are collected
  and surfaced in the status line.
- A mark whose path has disappeared on disk between toggle-time and
  batch-time is treated as a normal per-entry failure (the underlying
  fs_ops call returns the appropriate `io::Error`).
- `targets()` returns an empty `Vec` when nothing is selectable; every
  caller checks for empty first and short-circuits.
- `tree_row` with `marked == true` on a row whose path is no longer in
  `arena.nodes` is impossible: `marked` is derived from
  `arena.nodes[id].path`, so the path read for the check and the path
  rendered come from the same node.
- `Action::Cancel` in Normal mode is idempotent — calling it with an
  empty mark set is a no-op (no status change).

## Testing

- **Unit (`app.rs`):**
  - `toggle_mark_inserts_then_removes` — start empty, `toggle_mark`,
    assert in `marks`, `toggle_mark` again, assert removed.
  - `clear_marks_empties` — populate, `clear_marks`, assert empty.
  - `select_all_visible_marks_every_visible_path` — assert
    `marks.len() == app.visible.len()` and every visible path present.
  - `targets_returns_marks_when_nonempty_else_cursor` — mark two paths,
    assert `targets()` length 2 in alphabetical (BTreeSet) order; clear
    marks, assert `targets()` == `[cursor_path]`.
  - `cancel_clears_marks` — populate, `apply(Action::Cancel)`, assert
    empty.
  - `targets_survive_arena_rebuild_by_path` — mark a path, replace
    `app.arena` with a fresh `Arena::new` of the same root, assert
    `targets()` still contains the marked path (paths stable, ids not).
- **Manual:** mark/clear UI, select-all, render of `*` + accent +
  selection-bar combination, footer count, batch yy/dd/p/x/X across two
  marked entries (one of which is a directory).

## Out of Scope (YAGNI)

- Visual-mode (`V`) range selection.
- Mark counts as bytes / dir-recursive-size in the footer.
- Persistence of `marks` across restart.
- Per-pane marks (bookmarks pane never has marks).
- Mixed copy/cut in one clipboard slot, multi-clipboard slots.
- Undo for destructive batches.
