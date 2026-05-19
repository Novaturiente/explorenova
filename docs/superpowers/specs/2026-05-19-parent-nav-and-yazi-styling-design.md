# explorenova — Parent Navigation + Yazi-style Icons & Styling

**Date:** 2026-05-19
**Status:** Approved (pending written-spec review)
**Builds on:** shipped explorenova on `master`.

## Problem

1. The tree root is fixed to the start directory; there is no way to
   navigate above it (the user cannot reach the parent folder).
2. The UI is plain: no filetype icons, text-only selection highlight,
   flat colours, square borders, no path context. It should look modern
   like `yazi`.

## Decisions (locked during brainstorming)

- **Parent nav:** `-` ascends from anywhere; `h`/`←` ascends only when
  the cursor is on the root row (else normal collapse). After ascending,
  the directory just left is selected. Filesystem `/` is the lower bound
  (no-op).
- **Icons:** curated Nerd Font extension→glyph map (~70 entries) plus
  dir (collapsed/open), symlink specials. `config.icons: bool`, default
  `true`; `false` restores the icon-less look.
- **Styling:** full-row selection bar, richer type colours, path header
  + status footer, rounded borders, dimmed secondary text.
- **Tree render:** refactor the tree pane from `Paragraph` to
  `List` + `ListState` (gives a true full-width selection bar and
  viewport scrolling; also fixes the pre-existing long-tree overflow).
- **Theme is a fixed tasteful default** (not config-exposed). No
  size/permission columns, no alternate icon sets (YAGNI).

## Architecture

| Unit | Responsibility |
|---|---|
| `src/icons.rs` (new) | `icon_for(kind: NodeKind, expanded: bool, name: &str) -> &'static str` and `classify(...) -> Kind` for type colouring. Pure, unit-tested. |
| `src/config.rs` | Add `pub icons: bool` with a serde default of `true`. |
| `src/tree/node.rs` | Add `pub exec: bool` to `Node` (default `false`). |
| `src/tree/arena.rs` | In `load_children`, set `exec` from the already-fetched metadata (Unix mode `& 0o111 != 0`; non-Unix → `false`). |
| `src/keymap.rs` | Add `Action::Parent`; `from_name` `"parent"`; default binding `["-"]`. Nothing else changes. |
| `src/main.rs` | `handle()`: new `Action::Parent` arm (ascend); intercept `Action::Collapse` — if the selected row is the root node, ascend, else `app.apply(Action::Collapse)`. |
| `src/ui/theme.rs` | Add `executable, symlink, media, archive, selection_bg, header, dim` colours with fixed defaults. |
| `src/ui/panes.rs` | `tree_row` builds a `ratatui::text::Line` (icon? + name) coloured by classified type + git glyph; secondary glyphs use `dim`. No per-row background (the `List` applies the selection bar). |
| `src/ui/mod.rs` | Tree pane → `List` + `ListState`; `render_frame` layout becomes vertical `[header(1), body, footer(1)]`; rounded borders on panes + modal. |

### Type classification

`icons::classify(kind, exec, name) -> Kind` where
`Kind ∈ { Dir, Symlink, Exec, Media, Archive, File }`:

- `kind == Dir` → `Dir`
- `kind == Symlink` → `Symlink`
- `exec == true` (and not dir/symlink) → `Exec`
- extension in media set (png jpg jpeg gif webp bmp svg mp4 mkv mov mp3
  flac wav) → `Media`
- extension in archive set (zip tar gz bz2 xz zst 7z rar) → `Archive`
- else → `File`

Both the icon and the row colour derive from this single function so
they never disagree.

## Behaviour

### Parent navigation

- `Action::Parent` (key `-`, any row): let `root = arena.nodes[root].path`.
  `parent = root.parent()`. If `parent` is `None` or `parent == root`
  (i.e. `root` is `/`), no-op (optionally `status = "at filesystem root"`).
  Otherwise rebuild the arena rooted at `parent` using the existing
  rebuild pattern (preserve `show_hidden`: if it was on, clear the new
  root's children, reload, expand), then `reveal` the old root path and
  select it so the cursor sits on the folder just left.
- `Action::Collapse` (keys `h`/`←`): in `handle()`, if the selected
  visible row is the root node (`app.visible[app.selected] == arena.root`),
  perform the same ascend as `Action::Parent`; otherwise
  `app.apply(Action::Collapse)` (unchanged collapse/parent-select
  behaviour within the tree).
- Ascend is filesystem IO (`Arena::new`) so it lives in `main::handle()`,
  consistent with the existing `ToggleHidden` rebuild.

### Icons

- `tree_row` prepends `icons::icon_for(...) + " "` only when
  `app.config.icons` is `true`.
- `icon_for`: `Dir` → collapsed `` / expanded `` folder glyph;
  `Symlink` → ``; else lowercase extension looked up in a curated
  static map (~70 common types: rs py js ts tsx jsx go c h cpp rb java
  kt swift php sh md txt json toml yaml yml lock xml html css scss png
  jpg jpeg gif webp svg pdf zip tar gz 7z mp3 mp4 mkv wav csv sql db
  lua vim el ex exs hs ml nim zig dart …) → its glyph; unknown → a
  default file glyph ``.
- Glyphs are Nerd Font private-use codepoints (string literals in the
  source). No icon when `config.icons == false`.

### Theme (fixed defaults)

`Theme` gains: `executable` (green), `symlink` (cyan), `media`
(magenta), `archive` (red), `selection_bg` (a dark highlight bg used as
the `List` highlight style), `header` (bold accent), `dim` (dark grey).
Existing `dir`, `file`, git-status colours retained. Values are fixed in
`Theme::default()`; not read from config.

### Render

- `render_frame` splits `f.area()` vertically into
  `[Length(1) header, Min(0) body, Length(1) footer]`. The horizontal
  pane layout uses `body` (as before). The modal overlay is centred over
  `body`.
- **Header**: the current root path, left-truncated to fit, styled
  `header`.
- **Footer**: `"{selected+1}/{visible_len}  {status}"` (selection index
  + count, then any transient message), replacing the old prompt row
  (the prompt is already a modal since the previous feature).
- **Tree pane**: a `List` of items (one per `app.visible` entry) built
  via `tree_row`; `ListState` selected = `app.selected`;
  `highlight_style = bg(selection_bg)` → full-width selection bar; the
  `List` scrolls so the selected row stays visible (fixes long-tree
  overflow).
- **Borders**: panes (bookmarks/tree/preview) and the modal use
  `BorderType::Rounded`.
- **Dim**: on non-selected rows, the tree expand markers and the git
  glyph (when status is `Clean`) use the `dim` style. The selected row
  renders all of its spans at full strength (no `dim`) so the text stays
  readable on the selection bar; the bar background comes solely from
  the `List` `highlight_style`, not from per-span styling in `tree_row`.

## Error Handling

- Ascend at `/` is a no-op (no panic, optional status note).
- `Arena::new(parent)` failure: keep the current arena, set
  `status = "cannot open <parent>"`. Never panic.
- Missing/empty extension → default file icon; unknown extension →
  default file icon. `classify` never panics.
- Non-Unix builds: `exec` is always `false` (no executable colour);
  everything else unaffected.

## Testing

- **Unit (`icons`)**: extension→glyph for several known types; unknown→
  default; dir collapsed vs expanded glyph; symlink glyph; `classify`
  returns Dir/Symlink/Exec/Media/Archive/File for representative inputs.
- **Unit (`config`)**: `icons` defaults to `true`; explicit
  `icons = false` parses.
- **Unit (`tree`/`arena`)**: a file created with mode `0o755` loads with
  `exec == true`; a `0o644` file loads `exec == false` (Unix-gated test).
- **Manual**: parent ascend via `-` and `h` at root (cursor lands on the
  came-from dir; `/` is a no-op); icons render with a Nerd Font; full-row
  selection bar; header/footer; rounded borders; `icons = false`
  restores the plain look; long tree scrolls.

## Out of Scope (YAGNI)

- Configurable colour theme / multiple palettes.
- Size, permission, mtime columns.
- Alternate icon sets (Unicode/ASCII) or per-extension config overrides.
- Mouse, breadcrumb segments as clickable, animated transitions.
