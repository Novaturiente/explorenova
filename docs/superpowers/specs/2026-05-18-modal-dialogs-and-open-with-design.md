# explorenova — Modal Dialogs, Caret Line Editor & Open-With App Picker

**Date:** 2026-05-18
**Status:** Approved (pending written-spec review)
**Builds on:** the shipped explorenova file manager (branch `master`).

## Problem

Three deficiencies in the current interactive layer:

1. **Open-with (`O`)** requires typing a raw shell command. There is no
   list of available applications (unlike pcmanfm).
2. **Input mode** (rename / search / path / shell-once) uses a primitive
   append-only `String` rendered as a single status-bar line: no visible
   cursor, no caret navigation, and Backspace is unreliable across
   terminals.
3. **Prompts** (rename, search, shell, delete confirmation) appear as
   status-bar text rather than a focused dialog, which is easy to miss.

## Decisions (locked during brainstorming)

- **App discovery:** parse freedesktop `.desktop` files and rank by MIME.
- **Line editor:** a basic caret editor (cursor + Left/Right/Home/End +
  Backspace/Delete + robust DEL/Ctrl-H), insert at caret.
- **Presentation:** one reusable centered modal overlay for input,
  confirm, and the app picker. The status row is kept only for transient
  result/error messages.
- **App picker** includes a `Custom command…` entry that falls back to
  the existing typed-command path (pcmanfm parity).

## Architecture

Each new unit is small, single-purpose, and independently testable.

| Unit | Responsibility | Depends on |
|---|---|---|
| `src/textinput.rs` (new) | `TextInput`: owned `String` + caret `usize`. Pure methods `set/insert/backspace/delete/left/right/home/end/value`. | std only |
| `src/apps.rs` (new) | `apps_for(&Path) -> Vec<AppEntry>`: scan XDG application dirs, parse `[Desktop Entry]`, MIME-rank via `mime_guess`. `AppEntry { name, exec, terminal }`. | `mime_guess` |
| `src/ui/modal.rs` (new) | Centered overlay renderer (`Clear` + bordered `Block`) + a list-selection helper with wrapping navigation. | `ratatui` |
| `src/app.rs` (modified) | `input: String` → `input: TextInput`. New `dialog: Dialog` field. | textinput, apps |
| `src/ui/mod.rs` (modified) | `render_frame` draws the active modal over the panes and sets the terminal cursor inside the input box. Status row shows only `app.status`. | modal |
| `src/main.rs` (modified) | Route Input/Confirm/AppPick keys to the editor / list / picker. | — |
| `src/lib.rs` (modified) | Declare `textinput`, `apps`. | — |
| `src/keymap.rs` | Unchanged — no new actions or bindings. | — |

### State model

`App` gains:

```rust
pub enum Dialog {
    None,
    Input,                       // kind disambiguated by existing `pending`
    Confirm,                     // existing PermanentDelete confirm
    AppPick { items: Vec<apps::AppEntry>, sel: usize },
}
```

`Mode` (Normal/Search/Input/Confirm) is retained for key routing.
`App::input` becomes a `TextInput`. The modal is rendered whenever
`mode != Normal`; `AppPick` is entered from the `OpenWith` flow.

## Behaviors

### Caret line editor (`TextInput`)

- Internal: `value: String`, `caret: usize` (char index, clamped).
- `insert(c)` inserts at caret, caret += 1.
- `backspace()` removes the char before caret (no-op at 0).
- `delete()` removes the char at caret (no-op at end).
- `left/right` move caret by one, clamped; `home/end` jump to 0/len.
- `set(seed)` replaces value, caret = end (used by rename pre-fill).
- `value()` returns `&str`.
- Caret is a **char index**; all ops operate on `char` boundaries
  (UTF-8 safe), not bytes.

### Key routing (Input/Search modes, `main.rs`)

`KeyCode` → action:

- `Left/Right` → `left/right`; `Home/End` → `home/end`.
- `Backspace` → `backspace`. Also map `Char('\u{7f}')` (DEL) and
  `Char('\u{8}')` (Ctrl-H) to `backspace` — fixes the cross-terminal
  Backspace bug.
- `Delete` → `delete`.
- `Char(c)` (printable, not the two control chars above) → `insert(c)`.
- `Enter` → commit (existing per-`pending` logic). `Esc` → cancel
  (clear input + pending, dialog None, mode Normal).

### Modal overlay (`ui/modal.rs` + `render_frame`)

- A centered rect ≈ 55% width, height = content + borders, clamped to
  the frame. `Clear` the rect, draw a bordered `Block` with a title.
- Input dialog body: the `TextInput` value as one line; title is
  `Rename` / `Search` / `Go to path` / `Shell`. The terminal cursor is
  positioned with `f.set_cursor_position`. **Scroll rule (v1):** let
  `w = inner width`. If `value.chars().count() < w`, render the whole
  value from `inner.x` and place the cursor at `inner.x + caret`.
  Otherwise render the substring ending at `caret` whose length is
  `w-1` (i.e. the last `w-1` chars up to the caret) and place the
  cursor at `inner.x + (w-1)`. No other horizontal scrolling.
- Confirm dialog body: `"<name>"` + `[y] yes  [n] no`.
- App-picker body: scrollable list; selected row highlighted; a visual
  separator line precedes the final `Custom command…` row.
- Panes still render underneath (not cleared); only the modal rect is
  cleared, giving an overlay effect.

### Open-with app picker (`O`)

1. `O` on the selected node → `apps::apps_for(path)`:
   - Directories scanned: `$XDG_DATA_HOME/applications` (default
     `~/.local/share/applications`), `/usr/share/applications`, and each
     `$XDG_DATA_DIRS/applications` (default
     `/usr/share:/usr/local/share`). De-duplicate by desktop-file id;
     first occurrence wins (user dir overrides system).
   - Parse the `[Desktop Entry]` group: `Name`, `Exec`, `MimeType`,
     `NoDisplay`, `Terminal`, `Type`. Skip entries where
     `Type != Application`, `NoDisplay=true`, or `Exec` missing.
   - File MIME via `mime_guess::from_path(path).first()`.
   - `MimeType` is `;`-separated. An entry "matches" if any token,
     compared case-insensitively, equals the file's MIME essence
     string (e.g. `image/png`). Order: matching entries first, then
     the rest; each group sorted by `Name` case-insensitively. If the
     file MIME is unknown, no entry matches → all listed unranked
     (alpha).
   - Append a synthetic `Custom command…` entry last.
2. Result → `Dialog::AppPick { items, sel: 0 }`.
3. Keys: `↑/↓` move `sel` (wrapping), `Enter` activate, `Esc` close
   (dialog None, mode Normal).
4. Activate:
   - `Custom command…` → set `pending = OpenWith`, switch to
     `Dialog::Input` / `Mode::Input` (the existing typed-command path,
     now with the caret editor).
   - An app → build argv from `Exec`: strip field codes
     `%f %F %u %U %i %c %k` (replace `%f/%F/%u/%U` with the file path,
     drop the rest), split on whitespace (simple split; quoted args in
     Exec are uncommon for the picker and out of v1 scope). If
     `Terminal=true`, wrap as `$TERM -e <argv>` (best-effort; falls back
     to direct spawn if `$TERM` unset). Spawn detached.
5. Spawn failure → `app.status = "open failed: …"`; dialog closes.

## Error Handling

- `.desktop` parse: malformed lines/entries are skipped, never panic.
- Empty/unknown MIME: all apps listed unranked (still usable).
- No applications found: list still contains `Custom command…`.
- Spawn errors surface on the status line; never crash the TUI.
- Caret operations are saturating/clamped; never index out of bounds.

## Testing

- **Unit (`textinput`):** insert/backspace/delete/left/right/home/end at
  bounds (caret 0, caret = len, empty string); UTF-8 multi-byte char
  insert+backspace; DEL/Ctrl-H map to backspace (tested at the routing
  layer or via a small mapper fn).
- **Unit (`apps`):** parse a sample `.desktop` string — Name/Exec
  extracted, `NoDisplay=true` skipped, `Type=Link` skipped, field codes
  stripped; MIME ranking puts a matching entry before a non-matching
  one; alpha order within group.
- **Unit (`modal`):** list wrap-navigation (down past end → 0, up past 0
  → last); centered-rect geometry within a known frame size.
- **Integration:** `apps_for` over a tempdir of fixture `.desktop`
  files with a known file extension produces the expected ordered names.
- **Manual:** modal rendering, cursor placement, real app launch,
  terminal-app wrapping.

## Out of Scope (YAGNI)

- Word motions / emacs bindings / kill-ring in the editor.
- Fuzzy filtering within the app-picker list.
- Desktop-entry icons, localized `Name[xx]`, `DBusActivatable`,
  `TryExec`, action sub-entries.
- Quoted/escaped argument parsing in `Exec` beyond whitespace split.
- Horizontal input scrolling beyond the simple tail view.
- Mouse support.
