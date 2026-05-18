# explorenova — Terminal File Manager Design

**Date:** 2026-05-18
**Status:** Approved (pending written-spec review)
**Crate / package name:** `explorenova`
**Project directory:** `/home/nova/personal/explorer`

## Purpose

A daily-driver terminal file manager. An nvim-tree-style collapsible tree with
git status, a toggleable bookmarks pane, and a yazi-style preview pane (text,
image, PDF). Vim-first keybindings with conventional explorer aliases, fuzzy
search, safe file operations, configurable panes, path navigation with tab
completion, and an in-place shell runner. Priority: robustness and speed for
everyday use.

## Language Decision: Rust (not C)

The tool is a long-running interactive process whose hard parts — a
git-annotated lazy tree, gitignore-aware recursive walking, fzf-grade fuzzy
matching, terminal graphics protocols, trash semantics, mime/xdg integration —
are mature, audited Rust crates. C would require hand-rolling all of these with
manual memory management in a process where use-after-free is easy and costly.
Rust delivers a production-grade result with far less code and no GC. C would
only win if the goal were learning C; it is not. **Decision: Rust.**

## Architecture

Approach A: `ratatui` + `crossterm` with a synchronous render loop and
background worker threads. No async runtime.

- **Main thread:** input handling and rendering only. Never blocks on IO.
- **Worker threads**, results returned via `mpsc` channels:
  1. Git status scan
  2. Recursive walk for search
  3. Filesystem watch (`notify`, ~150ms debounce → targeted tree refresh)
  4. Preview generation (text highlight / image decode / PDF rasterize)
- App state is reduced on the main thread between frames as worker messages
  and input events arrive.

Rejected: tokio async loop (needless complexity for this IO profile); Cursive
(fights a custom git-annotated collapsible tree).

### Crate stack

| Concern | Crate / approach |
|---|---|
| TUI / input | `ratatui` + `crossterm` |
| Git status | `git2` (libgit2 safe bindings) |
| FS walk / search | `ignore` (ripgrep gitignore walker) + `nucleo` (fzf-grade matcher) |
| FS events | `notify` (debounced) |
| File ops | `std::fs` + `fs_extra` (recursive copy/move) |
| Delete | `trash` crate (default); explicit permanent path with confirm |
| Open / open-with | `open` crate (xdg defaults); command prompt for "open with" |
| Image preview | `ratatui-image` + `image` (kitty/sixel/iTerm2 auto, halfblock fallback) |
| PDF preview | shell `pdftoppm` (poppler-utils) → PNG → `ratatui-image`; fallback to metadata |
| Text preview | `syntect` syntax highlighting (lazy, size-capped) |
| CLI args | `clap` (derive) |
| Config / state | `serde` + `toml` |
| Errors | `anyhow` internal, `thiserror` at boundaries |

External runtime dependency: `poppler-utils` (`pdftoppm`) for PDF preview only;
absence degrades gracefully to metadata.

## Command-Line Interface

Parsed with `clap` (derive) in `main.rs`.

| Flag | Effect |
|---|---|
| `[PATH]` (positional) | Start in this directory (default: last cwd from state, else `$PWD`) |
| `--tree-only` / `-t` | Launch with the tree pane only — bookmarks and preview forced hidden for this session |

`--tree-only` is a **session override**: it overrides config/state pane
visibility at startup but is not written back to the state file, so the next
normal launch restores the saved layout. Panes can still be re-enabled at
runtime (`B`/`P`) within the session.

## UI & Layout

Three panes, left to right: **bookmarks** (toggleable), **tree** (main),
**preview**. Layout is rebuilt every frame from config: pane order,
per-pane visibility, and width/weight. Runtime changes (toggle, resize,
reorder, focus) are persisted to the state file and survive restart;
`config.toml` provides the defaults.

- Focus cycles among visible panes with `Tab`.
- Bookmarks pane is hidden by default, toggled with `B`.
- Preview pane visible by default, toggled with `P`.
- Resize focused pane with `<` / `>`. Reorder moves the focused pane left/right.
- **Sizing rule:** a pane with `width` set takes that many fixed columns; panes
  with `weight` split the remaining width proportionally. Runtime resize
  adjusts `width` (fixed pane) or `weight` (proportional pane).

### Tree rendering

- Indented tree with `▾`/`▸` expand markers and a git-status glyph/color per
  row (`M`, `A`, `D`, `R`, `?`, `!`, conflict).
- Directories show an aggregated status = worst child state (nvim-tree
  behavior).
- Hidden files toggled with `.`.

## Data Model

- **Arena tree:** `Vec<Node>` indexed by `NodeId`; no `Rc`/`RefCell`.
  `Node { path, kind: File|Dir|Symlink, expanded: bool,
  children: Option<Vec<NodeId>> (lazily loaded), git: GitStatus, depth }`.
- **Visible list:** flattened DFS over expanded nodes, recomputed on structural
  change. Selection is an index into this list.
- **`GitStatus`:** `Clean | Modified | Added | Deleted | Renamed | Untracked |
  Ignored | Conflicted`. Directory status is the aggregated worst child state.
- **Bookmarks:** `Vec<PathBuf>`, persisted to state file.
- **Search state:** `{ query: String, candidates: Vec<(PathBuf, Vec<usize>)> }`
  where the index vector drives match highlighting.

## Keybindings

Vim-first with conventional explorer aliases. Mode-aware
(Normal / Search / Input / Confirm). Every binding remappable under `[keys]`
in config; defaults below.

| Action | Default keys |
|---|---|
| Move | `j`/`k`/`↑`/`↓`, `gg`/`G`, `Ctrl-d`/`Ctrl-u` |
| Expand / collapse | `l`/`→` open, `h`/`←` collapse, `Enter` toggle dir / open file |
| Open default / with | `o` / `O` |
| Pane focus / toggle | `Tab`, `B` bookmarks, `P` preview |
| Pane resize | `<` / `>` |
| Create | `a` (trailing `/` → directory, nested mkdir -p) |
| Copy / cut / paste | `yy` / `dd` / `p` |
| Rename | `r` (inline, prefilled) |
| Trash / permanent delete | `x` or `Del` / `X` (typed y/N confirm) |
| Toggle bookmark | `m` |
| Fuzzy search | `/`, `n` / `N` next/prev, `Esc` clear |
| Path navigation | `:` (input with Tab completion) |
| Shell | `!` one-shot in cwd, `S` drop to `$SHELL` |
| Toggle hidden / refresh / quit | `.` / `R` / `q` |

## Behaviors

### File operations & safety

- **Create (`a`):** trailing `/` creates a directory, otherwise a file; nested
  paths create intermediate directories.
- **Copy/cut/paste:** `yy`/`dd` register the selection; `p` pastes via
  `fs_extra` (recursive). Name collisions get a ` (1)` suffix — never a silent
  overwrite.
- **Rename (`r`):** inline edit prefilled with the current name.
- **Delete:** `x`/`Del` → system trash via `trash`. `X` → permanent delete,
  gated by a blocking typed y/N confirm modal. Permanent deletion is
  irreversible and is never applied in bulk without a per-batch confirm.
- All operations run on a worker thread; failures surface on the status line
  and never panic.

### Search

`/` streams paths from `ignore::WalkBuilder` (respects `.gitignore` and hidden
files by default) on a worker thread into an incremental `nucleo` matcher,
producing a ranked list with match-index highlighting. `.` toggles inclusion of
hidden/ignored entries. `Enter` jumps the tree to the match, auto-expanding
ancestor directories. A new keystroke cancels the in-flight walk cleanly.

### Path navigation

`:` opens a path input. `Tab` completes against the filesystem
(longest-common-prefix, cycling on repeat). `~` and environment variables are
expanded. `Enter` jumps the tree to the path (expanding ancestors) or reports
an error if it does not exist.

### Open / open-with / shell

- `o` opens with the xdg default via the `open` crate.
- `O` prompts for a command and runs `<cmd> <path>` detached.
- `!` runs a one-shot command in the current directory: TUI suspends, command
  runs, "press a key" resumes, tree refreshes.
- `S` execs `$SHELL` with the current working directory; the terminal is fully
  restored on the shell's exit.

### Preview pipeline

Mime-dispatched, runs on a worker thread, cached by `(path, mtime)`, debounced
on selection movement:

- **Text:** `syntect` syntax highlighting, lazily loaded, size-capped (~1 MB).
- **Image** (png/jpg/webp/gif/…): `image` decode → `ratatui-image`
  (kitty/sixel/iTerm2 auto-detected, halfblock fallback).
- **PDF:** `pdftoppm` renders page 1 to PNG → `ratatui-image`; if poppler is
  absent, falls back to metadata.
- **Binary / oversized / unknown:** metadata (size, permissions, mime, mtime).

## Configuration & Persistence

- **`$XDG_CONFIG_HOME/explorenova/config.toml`** (default
  `~/.config/explorenova/config.toml`): pane order/visibility/width, `[keys]`,
  theme, preview size limits, behavior options. Missing on first run → written
  with commented defaults.
- **`$XDG_DATA_HOME/explorenova/state.toml`** (default
  `~/.local/share/explorenova/state.toml`): bookmarks, last working directory,
  runtime pane overrides.
- **Load order:** built-in defaults → merge user `config.toml` → merge state
  → apply CLI session overrides (e.g. `--tree-only`). Session overrides are not
  persisted back to the state file. A malformed config loads defaults and emits
  a non-fatal warning rather than failing to start.

Example pane config:

```toml
[panes]
order = ["bookmarks", "tree", "preview"]

[panes.bookmarks]
visible = false
width   = 24

[panes.tree]
weight = 2

[panes.preview]
visible = true
weight  = 3
```

## Error Handling

`anyhow` internally, `thiserror` at module boundaries. No `unwrap` on IO or
user-supplied paths. Terminal raw-mode and the alternate screen are restored
through an RAII guard, including on panic: a panic hook returns the terminal to
cooked mode before printing. Worker-thread errors travel back over their channel
and render on the status line.

## Module Layout

Each module has a single purpose and a channel/struct interface; dependencies
point inward toward `app`/`tree`.

```
main.rs      CLI parse (clap), bootstrap, terminal RAII, event loop
app.rs       App state, Mode enum, action reducer
tree/        arena, node, flatten, lazy load, expand/collapse
git.rs       worker: repo scan, path→status map, directory aggregation
fs_ops.rs    create / copy / move / rename / delete (trash | permanent)
search.rs    ignore-walk producer + nucleo matcher
nav.rs       path input + filesystem tab completion
preview/     mime dispatch: text / image / pdf / metadata + cache
ui/          layout-from-config, per-pane render, theme
keymap.rs    Action enum, mode-aware config-bound keymap
config.rs    TOML load/merge of defaults + state persistence
shell.rs     suspend / spawn / restore
```

## Testing Strategy

- **Unit:** tree flatten/expand/collapse, git status aggregation, keymap
  resolution, config merge, path completion, collision-rename suffixing.
- **Integration:** filesystem operations in `tempfile` directories
  (create/copy/move/trash).
- **Git:** fixture repository; assert the path→status map.
- **Manual (not unit-tested):** preview rendering and terminal graphics
  protocols.

## Out of Scope (YAGNI)

- Async runtime.
- Plugin/scripting system.
- Remote/SSH filesystems.
- Multi-tab or split sessions.
- Configurable themes beyond a single `[theme]` table.
