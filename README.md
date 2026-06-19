# explorenova

A daily-driver terminal file manager: nvim-tree-style collapsible
git-annotated tree, toggleable bookmarks pane, yazi-style preview
(text / image / PDF), fuzzy search, safe file operations, vim
keybindings, and configurable panes.

## Build

    cargo build --release

The binary is `target/release/explorenova`.

## Run

    explorenova [PATH] [--tree-only|-t]

- `PATH` — directory to start in (default: last cwd, else `$PWD`)
- `--tree-only` / `-t` — launch with only the tree pane (bookmarks +
  preview hidden for this session; re-enable at runtime with `B` / `P`)

## Configuration

`$XDG_CONFIG_HOME/explorenova/config.toml` (default
`~/.config/explorenova/config.toml`) — pane order/visibility/width,
`[keys]`, preview size limits, `icons` (bool), and `confirm_trash`
(bool, default `false`: prompt y/N before trashing). Written with
defaults on first run.

Runtime state (bookmarks, last cwd, and the runtime pane
layout — visibility, width/weight, order):
`$XDG_DATA_HOME/explorenova/state.toml` (default
`~/.local/share/explorenova/state.toml`). Runtime pane
changes persist across restarts, except under `--tree-only`
(a non-persisted session override).

## Keys

| Action | Keys |
|---|---|
| Move | `j`/`k`/`↑`/`↓`, `gg`/`G`, `Ctrl-d`/`Ctrl-u` |
| Expand / collapse | `l`/`→`, `h`/`←`, `Enter` toggle/open |
| Open default / with | `o` / `O` |
| Pane focus / toggle | `Tab`, `B` bookmarks, `P` preview |
| Pane resize | `<` / `>` |
| Pane reorder | `[` / `]` (move focused pane left/right) |
| Create | `a` (trailing `/` → directory) |
| Copy / cut / paste | `yy` / `dd` / `p` |
| Rename | `r` |
| Trash / permanent delete | `x` or `Del` (optional confirm via `confirm_trash`) / `X` (always y/N confirm) |
| Toggle bookmark | `m` |
| Fuzzy search | `/`, `n` / `N`, `Esc` clear |
| Path navigation | `:` (Tab completion) |
| Shell | `!` one-shot in cwd, `S` drop to `$SHELL` |
| Toggle hidden / refresh / quit | `.` / `R` / `q` |
| Help overlay | `?` (any key dismisses) |
| Sort menu | `s` → `1`/`n` name, `2`/`s` size, `3`/`m` mtime, `4`/`e` ext, `r` reverse, `d` dir-first, `esc` close |
| Tabs (split-pane) | `t` new tab, `w` close tab, `Tab` swap focus |

The status bar at the bottom is split into three segments: cursor path /
perms / size / mtime on the left, `N sel • X.X MB` (selection summary) in
the middle, and `N items • Y free` (visible count + free disk) on the
right. Transient status messages overlay the left segment for ~3 seconds.

Sort settings (mode, reverse, dir-first) persist in `state.toml` under
`[sort]` and apply to both tabs in split-pane mode.

## Notes

- **PDF preview** requires `poppler-utils` (`pdftoppm`). Without it,
  PDFs show file metadata instead of a rendered page.
- **Image preview** uses the terminal graphics protocol
  (kitty / sixel / iTerm2), falling back to halfblocks.
- The tree auto-refreshes on filesystem changes (≈150 ms debounce),
  reloading only the directories that actually changed. Preview
  generation runs on a worker thread, so resting on a large archive or
  PDF never blocks scrolling. Press `R` to force a tree + git refresh.

## Tests

    cargo test
