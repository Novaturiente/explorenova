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
`[keys]`, `[commands]`, preview size limits, `icons` (bool), and
`confirm_trash` (bool, default `false`: prompt y/N before trashing).
Written with defaults on first run.

Runtime state (bookmarks, last cwd, and the runtime pane
layout — visibility, width/weight, order):
`$XDG_DATA_HOME/explorenova/state.toml` (default
`~/.local/share/explorenova/state.toml`). Runtime pane
changes persist across restarts, except under `--tree-only`
(a non-persisted session override).

### Theme

Colors default to the Catppuccin Mocha palette (truecolor, looks the same in
every terminal). To follow your terminal's color scheme instead, opt in:

    [theme]
    preset = "terminal"

Override any individual color with an ANSI name (`blue`, `light_cyan`) or
hex (`#50C8FA`):

    [theme]
    dir = "blue"
    git_modified = "#FFC33C"

Fields: `dir`, `file`, `selected_bg`, `git_modified`, `git_added`,
`git_untracked`, `git_conflict`, `match_hl`, `mark`, `executable`,
`symlink`, `media`, `archive`, `header`, `dim`. A bad value warns in the
status bar and keeps the default — it never discards the rest of your config.

### Preview tuning

    [preview]
    settle_cheap_ms = 150   # delay before text/dir/image previews
    settle_heavy_ms = 400   # delay before pdf/video/xlsx/archive previews
    cache_cap = 128         # max cached previews before eviction

    fs_refresh_debounce_ms = 150   # filesystem-watch refresh debounce

### Scripts and commands

Run shell commands and your own scripts against the selected files (marked
files, or the file under the cursor if none are marked). Three ways:

**Ad-hoc** — press `!`, type any command. These placeholders expand, each
path shell-quoted:

| Token | Expands to |
|---|---|
| `{paths}` | every selected path, space-joined |
| `{file}` | the first selected path |
| `{dir}` | its directory |
| `{name}` | its file name |
| `{ext}` | its extension |

e.g. `!tar czf out.tgz {paths}` or `!~/script.sh {paths}`. A command with no
placeholder (`!mkdir foo`) still runs as a plain shell escape.

**Bound keys** — map a single key to a command template:

    [commands]
    c = "tar czf archive.tgz {paths}"
    e = "$EDITOR {file}"

The key is one char or one `ctrl`/`alt` combo. A key that clashes with a
builtin binding is ignored with a startup warning (builtin wins).

**Scripts dir** — drop executables in
`$XDG_CONFIG_HOME/explorenova/scripts/`; press `\` to pick one from a list.
The chosen script receives the selected paths as arguments.

The first run writes a fully commented `config.toml` documenting every option.

### Openers

Map file extensions to custom open commands:

    [openers]
    md = "glow {file}"
    html = "firefox {file}"

A file whose extension has no rule opens with the OS default app. Same
`{file}`/`{paths}`/`{dir}`/`{name}`/`{ext}` placeholders as `[commands]`.

## Keys

| Action | Keys |
|---|---|
| Move | `j`/`k`/`↑`/`↓`, `gg`/`G`, `Ctrl-d`/`Ctrl-u` |
| Expand / collapse | `l`/`→`, `h`/`←`, `Enter` toggle/open |
| Open default / with | `o` / `O` |
| Pane focus / toggle | `Tab`, `Shift-Tab` sidebar section, `B` bookmarks, `P` preview |
| Pane resize | `<` / `>` |
| Pane reorder | `[` / `]` (move focused pane left/right) |
| Create | `a` (trailing `/` → directory) |
| Copy / cut / paste | `yy` / `dd` / `p` |
| Rename | `r` |
| Trash / permanent delete | `x` or `Del` (optional confirm via `confirm_trash`) / `X` (always y/N confirm) |
| Toggle pin | `m` |
| Fuzzy search | `/`, `n` / `N`, `Esc` clear |
| Path navigation | `:` (Tab completion) |
| Shell | `!` one-shot (with `{paths}`/`{file}`/… expansion), `S` drop to `$SHELL` |
| Scripts | `\` pick a script from the scripts dir; `[commands]` keys run templates |
| Toggle hidden / refresh / quit | `.` / `R` / `q` |
| Help overlay | `?` (any key dismisses) |
| Sort menu | `s` → `1`/`n` name, `2`/`s` size, `3`/`m` mtime, `4`/`e` ext, `r` reverse, `d` dir-first, `esc` close |
| Tabs (split-pane) | `t` new tab, `w` close tab, `Tab` swap focus |

The status bar at the bottom is split into three segments: cursor path /
perms / size / mtime on the left, `N sel • X.X MB` (selection summary) in
the middle, and `N items • Y free` (visible count + free disk) on the
right. Transient status messages overlay the left segment for ~3 seconds.

### Power features

- `f` — **filter** the tree to fuzzy matches (ancestors kept). Enter keeps it, Esc clears.
- `F` — **content search** with ripgrep (`rg`); jumps to the first file containing the query, `n`/`N` cycle.
- `z` — **zoxide jump**: type a query, jump to the best-ranked directory (needs `zoxide`).
- `b` — **bulk rename** the marked files in `$EDITOR` (one name per line).
- `L` — cycle the **linemode** column (none → size → mtime → perms).
- During a copy/move, `esc` or `ctrl-c` **cancels** the op.

Sort settings (mode, reverse, dir-first) persist in `state.toml` under
`[sort]` and apply to both tabs in split-pane mode.

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
