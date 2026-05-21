# explorenova — Preview Debounce + Visual Mode

**Date:** 2026-05-21
**Status:** Approved (decisions locked via brainstorming)
**Builds on:** shipped multi-select on `master`.

## Problem

1. **Scroll lag on image folders.** When `j`/`k` lands on an image, the
   preview pane decodes it immediately, blocking the next frame. Holding
   `j` through a folder of photos stutters.
2. **Non-contiguous batch selection is awkward.** Today the only way to
   build a non-contiguous mark set is `Space` on every individual row.
   yazi/ranger users expect a `v` "visual" mode that auto-marks rows the
   cursor crosses, so they can pick a block at a time. Toggling `v` off
   leaves marks intact so the user can move elsewhere and grab another
   block.

## Decisions (locked)

- **Image-preview debounce:** 150 ms. Cursor must sit on a new path for
  ≥ 150 ms before the image is decoded. Text/Meta previews remain
  immediate (their cost is negligible).
- **Visual mode semantics:** `v` toggles. On enter, the cursor row is
  marked (anchors). While on, every successful navigation action
  (Down/Up/Top/Bottom/HalfDown/HalfUp/PageDown/PageUp/Open/Collapse/
  Toggle/Parent) **marks** the new cursor row — never unmarks. Mode is
  additive (existing marks survive entry and exit). `v` again exits;
  marks stay. `Esc` (Cancel) clears marks AND exits visual.
- **Out of scope (YAGNI):** debouncing text/meta previews, persisting
  visual-mode state across restart, range-reverse un-marking, a
  separate Visual `Mode` enum variant (use a bool flag — keeps the
  modal dispatch in main.rs unchanged).

## Architecture

| Unit | Responsibility |
|---|---|
| `src/app.rs` | New `pub visual_mode: bool` (default `false`). New `pub cursor_moved_at: std::time::Instant` (default `Instant::now()` in `App::new`). New methods: `toggle_visual_mode(&mut self)`, `mark_cursor(&mut self)`. `Action::Cancel` arm gains `self.visual_mode = false;` (after the existing `marks.clear()`). |
| `src/keymap.rs` | New `Action::ToggleVisualMode` (default chord `v`, name `"toggle_visual_mode"`). |
| `src/main.rs::handle()` | New `Action::ToggleVisualMode` arm: calls `app.toggle_visual_mode()` which (a) flips the flag and (b) if entering, marks the cursor row. |
| `src/main.rs` event loop | Around each Normal-mode action dispatch, snapshot `prev_selected = app.selected`. After `handle()` returns, if `app.selected != prev_selected` then set `app.cursor_moved_at = Instant::now()`. **And** if `app.visual_mode && app.selected != prev_selected`, call `app.mark_cursor()`. |
| `src/main.rs` event poll | Clamp `event::poll` timeout to `min(200ms, debounce_remaining)` when the preview is image-bearing and debounce hasn't elapsed, so the deferred image draw fires within ~150 ms even with no input. |
| `src/ui/mod.rs::render_frame` preview arm | For `PreviewKind::Image`, only call `render_image()` if **either** the path equals `img_cache.key.path` (already decoded; cheap redraw) **or** `app.cursor_moved_at.elapsed() >= 150 ms`. Otherwise render the preview block with body text `"(loading…)"`. Text/Meta paths render immediately as today. |
| `src/ui/mod.rs::render_frame` footer | When `app.visual_mode`, prepend `"-- VISUAL --  "` styled `fg(theme.mark) + BOLD` to the footer line, before `marks_segment`. |

### Key types

```rust
// app.rs
pub struct App {
    // …existing fields…
    pub visual_mode: bool,
    pub cursor_moved_at: std::time::Instant,
}

impl App {
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
}
```

```rust
// main.rs event loop sketch
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

let prev_selected = app.selected;
// … dispatch keystroke through Mode::Normal or modal arms …
if app.selected != prev_selected {
    app.cursor_moved_at = Instant::now();
    if app.visual_mode {
        app.mark_cursor();
    }
}
```

```rust
// ui/mod.rs render_frame preview arm sketch
PreviewKind::Image(p) => {
    let block = Block::default().borders(Borders::ALL)
        .border_type(BorderType::Rounded).title("preview");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let already_decoded = img_cache.key.as_ref()
        .map(|(k, _)| k == &p).unwrap_or(false);
    let elapsed = app.cursor_moved_at.elapsed() >= PREVIEW_DEBOUNCE;
    if already_decoded || elapsed {
        render_image(f, inner, picker, img_cache, &p);
    } else {
        f.render_widget(Paragraph::new("(loading…)"), inner);
    }
}
```

## Behaviours

### Image preview debounce

- `cursor_moved_at` is set whenever the visible cursor index changes
  (any cause: keystroke, search-jump, picker-jump, ascend, fs-watch
  rebuild that shifts the index, etc. — the post-dispatch hook covers
  them all because it runs once per loop iteration).
- A path that's already in the image cache is drawn immediately (no
  decode cost — just blits cached protocol cells). This means
  navigating *back* to a previously-viewed image feels instant.
- A path that's NOT in the cache shows `"(loading…)"` until
  `cursor_moved_at.elapsed() >= 150ms`, at which point the next frame
  decodes and renders.
- The event poll timeout is clamped to the remaining debounce so the
  loop wakes up at ~150 ms even with no key/fs/git events. Without
  this, the user could see "(loading…)" for up to 200 ms longer than
  necessary.
- Text/Meta previews are NOT debounced — they hit `PreviewCache`
  (mtime-keyed) and cost is microseconds.

### Visual mode

- **Enter:** `v` in Normal mode → `visual_mode = true`, mark current
  cursor row. Footer shows `-- VISUAL --` indicator.
- **Move while on:** each successful navigation action runs through
  `handle()` (or `App::apply` for pure-state actions); the post-dispatch
  hook detects `selected` changed, then `mark_cursor()` inserts the new
  cursor path. Cursor is unaffected. Existing marks remain.
- **Exit:** `v` again → `visual_mode = false`. Marks remain. Footer
  indicator gone.
- **Cancel:** `Esc` (`Action::Cancel`) in Normal mode → clears marks
  AND turns visual_mode off (the existing `apply(Cancel)` arm extended
  with `self.visual_mode = false`).
- **Batch ops while on:** `yy`/`dd`/`p`/`x`/`X` continue to use
  `app.targets()` (marks-if-any-else-cursor). They do NOT auto-exit
  visual mode. After `Trash`/`PermanentDelete` (which clear marks) the
  user is still in visual mode with an empty mark set — fine, next
  movement re-marks.
- **No new mode enum variant.** Visual is a flag inside `Mode::Normal`.
  All modal dispatch (Search/Input/Confirm/AppPick) is untouched.

### Render

- Preview pane behaviour above.
- Footer (`src/ui/mod.rs` near `marks_segment`):
  ```rust
  let visual_segment = if app.visual_mode {
      // styled span inserted into a Line, not embedded in the format!
      // (the existing footer is a Paragraph::new(String); split into
      // a Line<'_> with spans so we can colour the indicator).
      Some(Span::styled("-- VISUAL --  ",
          Style::default().fg(theme.mark).add_modifier(Modifier::BOLD)))
  } else { None };
  // Compose: optional visual_segment + sel+1/total + marks_segment + status.
  ```

## Error Handling

- `mark_cursor()` is a no-op when `visible` is empty (matches existing
  `toggle_mark`).
- `toggle_visual_mode()` never fails; flipping the flag with no items
  visible enters visual mode with no anchor (next movement marks).
- `cursor_moved_at` is monotonic (`Instant::now()`); clock skew is
  irrelevant.
- Image-preview "(loading…)" placeholder is text; if the path becomes
  invalid between debounce-start and decode, the existing `render_image`
  fallback `"[unreadable image] <path>"` covers it.
- The event-poll clamp uses `saturating_sub` so a debounce that's
  already past returns a 0 ms poll (single tight pass), not a negative
  duration.

## Testing

- **Unit (`app.rs`):**
  - `toggle_visual_mode_marks_cursor_on_enter` — empty marks, call
    `toggle_visual_mode`; assert `visual_mode == true && marks.len() == 1
    && marks.contains(cursor_path)`.
  - `toggle_visual_mode_leaves_marks_on_exit` — enter, populate via two
    `mark_cursor` calls (or move-then-mark), exit; assert `visual_mode
    == false && marks.len() == 2`.
  - `cancel_exits_visual_and_clears_marks` — enter visual, populate
    marks, `apply(Action::Cancel)`; assert `visual_mode == false &&
    marks.is_empty()`.
  - `mark_cursor_is_idempotent` — call twice on the same cursor; assert
    `marks.len() == 1`.
  - `mark_cursor_no_op_when_visible_empty` — clear `visible`, call;
    assert `marks.is_empty()` and no panic.
- **Unit (`keymap.rs`):**
  - `v_resolves_to_toggle_visual_mode` — default keymap; assert
    `resolve(Normal, "v") == Some(Action::ToggleVisualMode)` and
    `Action::from_name("toggle_visual_mode") ==
    Some(Action::ToggleVisualMode)`.
- **Manual:**
  - Hold `j` through a folder of 30 images — scroll smooth, last image
    decodes ~150 ms after release.
  - Move back to a previously-viewed image — instant (cache hit, no
    debounce wait).
  - `v` at row A, j, j, j → A,B,C,D marked, `* ` prefix visible, footer
    shows `-- VISUAL --  4 selected`. `v` → indicator off, marks stay.
    `Esc` → all clear.

## Out of Scope (YAGNI)

- Debouncing text/meta previews.
- A separate `Mode::Visual` enum variant.
- Persisting `visual_mode` across restart.
- Reverse-direction un-marking inside visual.
- Visual-block / visual-line distinction.
- Configurable debounce duration (hard-code 150 ms; a constant in
  `main.rs` is easy to tweak later).
