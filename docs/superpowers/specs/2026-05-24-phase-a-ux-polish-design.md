# Phase A — UX Polish Design

**Date:** 2026-05-24
**Branch:** `phase-a-ux-polish`
**Status:** Approved

## Goal

Three low-risk daily-use polish features for explorenova:
1. `?` help overlay listing all active keybindings.
2. Richer 3-segment status bar (cursor info, selection summary, disk/count).
3. Sort controls (name / size / mtime / ext, reverse, dir-first), persisted.

## Non-Goals

- Recursive directory size in selection bytes (file sizes only).
- Custom theming for overlay/menu (use existing palette).
- Help search-within-help.
- Per-tab or per-directory sort (global only).
- Content search, archive ops, paste-conflict prompts, undo (later phases).

## Architecture

### New modules

- `src/help.rs`
  - `pub fn build_entries(km: &Keymap) -> Vec<HelpCategory>`
  - `pub struct HelpCategory { name: &'static str, rows: Vec<(String /* label */, String /* keys */)> }`
  - `pub fn render(area: Rect, frame: &mut Frame, entries: &[HelpCategory])`
  - Hardcoded `ACTION_META: &[(Action, &'static str /* category */, &'static str /* label */)]` table.

- `src/sort.rs`
  - `pub enum SortMode { Name, Size, Mtime, Ext }`
  - `pub struct SortState { mode: SortMode, reverse: bool, dir_first: bool }`
  - `pub fn sort_children(nodes: &mut [usize], arena: &Arena, sort: &SortState)`
  - `pub fn render_menu(area: Rect, frame: &mut Frame, sort: &SortState)`

- `src/status.rs`
  - `pub fn render(area: Rect, frame: &mut Frame, app: &App)`
  - `pub fn disk_free_bytes(path: &Path) -> Option<u64>` (libc::statvfs, no new dep)
  - 3-segment layout via `Layout::horizontal([Constraint::Percentage(60), 20, 20])`.

### Modified files

- `src/app.rs`
  - `App.sort: SortState`
  - `App.help_open: bool` (true when in `Mode::Help`; derived from `mode` — kept as field-free check OR add to Mode enum; see Modes)
  - `App.marks_bytes: u64` (incremental sum of file sizes in marks; updated on mark/unmark/clear)
  - `App.disk_free: Option<(PathBuf, u64, Instant)>` (cache key = cwd path)
  - `App.status_set_at: Instant` (for 3s fade of transient status)
  - Methods:
    - `apply_sort(&mut self)` — re-flatten all tabs using current sort
    - `toggle_mark(...)` updates `marks_bytes`
    - `refresh_disk_free(&mut self)` — recompute if cache stale or cwd changed
    - `set_sort_mode(&mut self, SortMode)`, `toggle_sort_reverse`, `toggle_sort_dir_first`

- `src/state.rs`
  - Add `[sort]` section: `mode: String`, `reverse: bool`, `dir_first: bool`. Round-trip via existing TOML.

- `src/keymap.rs`
  - New actions: `Help`, `SortMenu`, `SortByName`, `SortBySize`, `SortByMtime`, `SortByExt`, `SortToggleReverse`, `SortToggleDirFirst`.
  - Defaults: `(Help, vec!["?"])`, `(SortMenu, vec!["s"])`.
  - `Action::from_name` adds: `"help"`, `"sort_menu"`, `"sort_name"`, `"sort_size"`, `"sort_mtime"`, `"sort_ext"`, `"sort_reverse"`, `"sort_dir_first"`.

- `src/main.rs` (dispatch)
  - `Action::Help => app.mode = Mode::Help`
  - `Action::SortMenu => app.mode = Mode::SortMenu`
  - `Action::SortBy* => { app.set_sort_mode(_); app.apply_sort(); app.mode = Mode::Normal; }`
  - `Action::SortToggleReverse => { app.toggle_sort_reverse(); app.apply_sort(); }`
  - `Action::SortToggleDirFirst => { app.toggle_sort_dir_first(); app.apply_sort(); }`
  - In `Mode::Help`: any key → `app.mode = Mode::Normal`
  - In `Mode::SortMenu`: route digit/letter to corresponding action; `esc` → Normal

- `src/ui/mod.rs`
  - Replace existing status row with `status::render`.
  - After all panes drawn, if `app.mode == Mode::Help`: `help::render(centered_rect(80, content_h, frame.size()), frame, &entries)`.
  - If `app.mode == Mode::SortMenu`: `sort::render_menu(centered_rect(40, 12, frame.size()), frame, &app.sort)`.
  - Helper: `centered_rect(w, h, area) -> Rect`.

- `src/tree/mod.rs` (or wherever `flatten`/refresh lives)
  - `flatten` accepts (or App passes) the current `SortState` and uses `sort::sort_children` when ordering each expanded directory's children.

### Mode enum extension

```rust
pub enum Mode {
    Normal,
    Search,
    Input,
    Confirm,
    Help,      // NEW
    SortMenu,  // NEW
}
```

`Keymap::resolve` for `Mode::Help` → any non-esc returns `Action::Cancel`; `esc` also returns `Cancel`.
For `Mode::SortMenu` → map `1`/`n` → SortByName, `2`/`s` → SortBySize, `3`/`m` → SortByMtime, `4`/`e` → SortByExt, `r` → SortToggleReverse, `d` → SortToggleDirFirst, `esc` → Cancel.

## Behavior details

### Help overlay

- Modal centered popup. Block with border + title `" explorenova keys "`.
- Two columns per row: `label (left-aligned, padded)` `keys (right-aligned)`.
- Categories rendered as header rows (bold, no keys column).
- Width: `min(80, term_width - 4)`. Height: `min(rows + 4, term_height - 4)`.
- If content overflows height: truncate with last row `"… more (resize terminal)"`. No scroll in v1.
- Dismiss: any key in `Mode::Help` returns to `Mode::Normal`.

### Status bar

- Single row at bottom (existing row, restyled).
- Left segment (60%): `{path}  {perms}  {size}  {mtime}`.
  - `path` = full path of cursor item, truncated from left to fit segment width with `…/` prefix.
  - `perms` = unix mode string e.g. `-rw-r--r--` from `Metadata::permissions().mode()`.
  - `size` = human-readable bytes (1.2K, 32K, 5.4M, 2.1G). 0 for dirs.
  - `mtime` = `YYYY-MM-DD` (omit time to save space).
- Center segment (20%): when `app.marks.len() > 0`: `"{n} sel • {bytes_human}"`. Else empty.
- Right segment (20%): `"{n} items • {free} free"`.
  - `n` = `app.visible.len()`.
  - `free` = `app.disk_free` value, human-readable.
- Transient status overlay: if `app.status_set_at.elapsed() < 3s`, render `app.status` text overlaying Left segment, replacing it for that duration.
- Setting `app.status = X` updates `status_set_at = Instant::now()`. Centralize via `app.set_status(s)` helper.

### Selection bytes

- `App.marks_bytes: u64`.
- On `toggle_mark(path)`:
  - If path now in marks AND `!is_dir`: `marks_bytes += node.size`.
  - If path was in marks AND `!is_dir`: `marks_bytes -= node.size`.
- On clear (`Cancel` from visual mode, etc.): `marks_bytes = 0`.
- On select-all-visible: recompute as sum.

### Disk free

- Cached at `App.disk_free: Option<(PathBuf cwd, u64 bytes, Instant ts)>`.
- Recompute when:
  - cache is `None`, OR
  - `cache.0 != current_cwd`, OR
  - `cache.2.elapsed() > Duration::from_secs(30)`.
- Implementation: call `libc::statvfs` via `unsafe { libc::statvfs(...) }`; compute `f_bavail * f_frsize`. On failure return previous cached value (or `None` first time).

### Sort

- `SortState::default() = { mode: Name, reverse: false, dir_first: true }`.
- `sort_children` algorithm:
  1. If `dir_first`: partition into (dirs, files), sort each by `mode`, concat (dirs first); apply `reverse` to whole sequence after partition.
  2. Else: sort whole slice by `mode`, apply `reverse`.
- Sort keys:
  - Name: case-insensitive `to_lowercase().cmp(...)`, ties → case-sensitive.
  - Size: `node.size` numeric, ties → name asc.
  - Mtime: `node.mtime` (SystemTime → Duration since epoch), ties → name asc.
  - Ext: extension (lowercase) cmp, ties → name asc. Dotfiles (no ext) sort last in ext mode.

### Sort menu

- Centered modal, 40 wide × 12 tall.
- Body:
  ```
   Sort
   ────
   1  Name    [n]   (active)
   2  Size    [s]
   3  Mtime   [m]
   4  Ext     [e]

   r  Reverse: off
   d  Dir first: on
   ```
- Active mode shown with `(active)` suffix. `reverse`/`dir_first` show on/off state.
- Selecting 1-4 (or n/s/m/e) sets mode, applies, closes.
- `r`/`d` toggle and keep menu open (immediate visual feedback).
- `esc` closes without further changes.

### Persistence

- `state.toml` gains:
  ```toml
  [sort]
  mode = "name"        # "name" | "size" | "mtime" | "ext"
  reverse = false
  dir_first = true
  ```
- Loaded on startup, written on change.

## Edge cases

- Help overlay opened in `Mode::Search` or `Mode::Input`: ignore `?` in those modes (already gated by `keymap::resolve`). Only triggerable from Normal.
- Sort menu opened while marks active: marks persist; sort re-order does not clear marks. Marks keyed on `PathBuf`, paths unchanged.
- Sort applied to large tree: re-flatten cost O(n log n) per expanded dir. Acceptable for typical sizes; not optimized.
- statvfs on non-existent path: return cached value or `None`.
- Tree refresh during help overlay: rebuild is fine; overlay re-renders on next frame.
- Transient status during help overlay: status hidden by overlay; resumes on dismiss.
- Empty marks set: marks_bytes stays 0; center segment hidden.
- Free-disk on network mount that hangs: `statvfs` could block briefly. Accept for v1; if observed, move to background thread later.

## Testing strategy

### Unit tests

- `sort::sort_children`:
  - Name ascending, dir_first on.
  - Name descending, dir_first off.
  - Size ordering with ties broken by name.
  - Mtime ordering.
  - Ext ordering with dotfiles last.
- `state.rs`:
  - Round-trip `SortState` through TOML.
  - Default values when section missing.
- `keymap.rs`:
  - `?` resolves to `Action::Help`.
  - `s` resolves to `Action::SortMenu`.
  - In `Mode::SortMenu`: `1` → SortByName, `r` → SortToggleReverse, etc.
- `help::build_entries`:
  - Returns at least one row per category present in `ACTION_META`.
  - User remap (e.g. `quit → Q`) reflected in keys column.
- `status` helpers:
  - `human_bytes` formats correctly across K/M/G boundaries.
  - Path truncation with `…/` prefix preserves last components.

### Integration tests

- `app.rs`:
  - Open help → `mode == Help`; any key → `mode == Normal`.
  - `set_sort_mode` followed by `apply_sort` re-flattens tabs[0] in expected order.
  - `toggle_mark` increments/decrements `marks_bytes` correctly.
  - `marks_bytes = 0` after clear.
  - `disk_free` cache invalidates on cwd change.

### Manual verification

- Run binary; press `?` → modal shows; press `j` → modal closes, cursor moves (verify any-key dismiss).
- Press `s` → menu; press `2` → list reorders by size, menu closes.
- Press `s r` → list reverses, menu stays open showing `Reverse: on`; `esc` → close.
- Mark 3 files via `space`; verify center segment shows `3 sel • X.X MB`.
- `cd` into different mount; verify right segment updates free-disk within 30s tick.

## Implementation order (informs plan)

1. **Task 1: SortState + sort module + state persistence + tests.** (foundational; no UI yet)
2. **Task 2: Wire sort into flatten + Action variants + dispatch (no menu UI).** (verifies sort works end-to-end before adding menu chrome)
3. **Task 3: Sort menu modal + Mode::SortMenu + keymap routing.** (UI for Task 2)
4. **Task 4: Help action + Mode::Help + help::build_entries + render.** (independent module; can land before or after sort)
5. **Task 5: Status module + 3-segment renderer + disk_free + selection bytes + transient fade.** (largest UI change; uses no Sort/Help APIs)
6. **Task 6: Manual smoke pass + README key-table update.** (verify all three features interact correctly)

Each task is independent enough to commit on its own. Tasks 1-3 form the sort track; Task 4 is help; Task 5 is status; Task 6 is wrap-up.
