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
`[keys]`, preview size limits. Written with defaults on first run.

Runtime state (bookmarks, last cwd):
`$XDG_DATA_HOME/explorenova/state.toml` (default
`~/.local/share/explorenova/state.toml`).

## Keys

| Action | Keys |
|---|---|
| Move | `j`/`k`/`↑`/`↓`, `gg`/`G`, `Ctrl-d`/`Ctrl-u` |
| Expand / collapse | `l`/`→`, `h`/`←`, `Enter` toggle/open |
| Open default / with | `o` / `O` |
| Pane focus / toggle | `Tab`, `B` bookmarks, `P` preview |
| Pane resize | `<` / `>` |
| Create | `a` (trailing `/` → directory) |
| Copy / cut / paste | `yy` / `dd` / `p` |
| Rename | `r` |
| Trash / permanent delete | `x` or `Del` / `X` (typed y/N confirm) |
| Toggle bookmark | `m` |
| Fuzzy search | `/`, `n` / `N`, `Esc` clear |
| Path navigation | `:` (Tab completion) |
| Shell | `!` one-shot in cwd, `S` drop to `$SHELL` |
| Toggle hidden / refresh / quit | `.` / `R` / `q` |

## Notes

- **PDF preview** requires `poppler-utils` (`pdftoppm`). Without it,
  PDFs show file metadata instead of a rendered page.
- **Image preview** uses the terminal graphics protocol
  (kitty / sixel / iTerm2), falling back to halfblocks.
- The tree auto-refreshes on filesystem changes (≈150 ms debounce).
  Under sustained external filesystem churn on a very large tree the
  refresh rebuilds on the main thread and may briefly hitch; this is a
  known tradeoff. Press `R` to force a git/tree refresh anytime.

## Tests

    cargo test
