# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`explorenova` — a single-binary terminal file manager (ratatui + crossterm TUI).
nvim-tree-style collapsible git-annotated tree, bookmarks pane, yazi-style
preview (text/image/PDF), fuzzy search, safe file ops, vim keybindings,
split-pane tabs, SMB share mounting. ~10k lines of Rust across `src/`.

## Commands

```
cargo build --release        # binary at target/release/explorenova
cargo test                   # all unit + integration tests
cargo test <name>            # single test by substring (e.g. cargo test sort_reverse)
cargo test --test git_int    # one integration file (tests/{apps,fs_ops,git}_int.rs)
cargo clippy                 # lints (CI-clean; keep it that way)
cargo run -- [PATH] [-t]     # run; -t / --tree-only hides bookmarks+preview for the session
```

Runtime deps (optional, degrade gracefully): `poppler-utils` (`pdftoppm`) for PDF
preview; a graphics-capable terminal (kitty/sixel/iTerm2) for images, else halfblocks.

## Build-artifact noise

`src/cargo-home/` and `src/explorenova/` are **makepkg build artifacts** (the
Arch `PKGBUILD` unpacks here), untracked and gitignored. They are NOT source.
Always scope greps/searches with `--exclude-dir=cargo-home --exclude-dir=explorenova`
or use `git ls-files 'src/*.rs'` — a naive recursive grep reads the whole vendored
crate tree and returns hundreds of false hits.

## Architecture

### The event loop (`main.rs`)

Everything hangs off one synchronous loop in `main()`. Per iteration it: drains
finished previews, applies git-status results, folds filesystem-watch events into
`pending_dirs`, runs the 150ms fs-debounce refresh, polls any in-flight background
op, draws the frame, then blocks on `event::poll` with an **adaptive timeout** —
short during ops (smooth spinner), zero when a preview just finished, otherwise the
remaining per-file preview settle time capped at 200ms. Three worker threads feed it
over `mpsc` channels: git scan, fs notify watcher, and (detached) SMB auto-mount.
Preview rendering is its own thread pool inside `PreviewCache`.

Key invariant — **lazy per-directory fs-watching**: the watched set mirrors the
currently-expanded dirs (`watch_targets` / `sync_watches`, NonRecursive). Startup
watches only the root. Reconciled after every key event so newly-expanded dirs are
watched before the next render.

`self_write_until` / `self_write_dirs` (set via `app.mark_self_write`) suppress the
fs-watcher for dirs the app just mutated itself, so a copy/move/create doesn't
trigger a redundant self-induced refresh.

### Tree model (`tree/arena.rs`, `tree/node.rs`)

The tree is an **arena** (`Vec<Node>` indexed by `NodeId = usize`, not pointers).
`arena.nodes` is the flat store; `app.visible` is the flattened list of currently-
visible node ids (depth-first over expanded dirs), and `app.selected` indexes into
`visible`. Mutations go through arena methods (`expand`/`collapse`/`reload_dir`/
`reveal`/`resort_all`), then the caller rebuilds `visible` via `flatten`. `reload_dir`
reloads a single changed directory in place (used by the debounced fs refresh) rather
than rebuilding the whole tree. `probe_empty` lazily marks empty dirs for only the
on-screen window to avoid an O(tree) `read_dir` storm.

### Input dispatch (`keymap.rs`, `dispatch.rs`, `app.rs`)

- `Mode` (Normal/Search/Input/Confirm/SortMenu/Help) is the current input context;
  `Dialog` is the matching overlay variant. `main.rs` re-derives `app.dialog` from
  `app.mode` after each key.
- `Keymap::build` merges user `[keys]` config over defaults; `resolve(mode, buffer)`
  maps a chord buffer to an `Action`. Normal-mode chord building uses `key_token`
  (only chord-relevant keys); Search/Input/Confirm/AppPick arms read `k.code`
  directly so they see Backspace/Home/etc.
- `dispatch::handle_key` is the entry point from the loop; `App::apply(Action)` is the
  big action executor. Tree-mutation handlers and refresh logic (`reroot`,
  `refresh_in_place`, `refresh_dirs`, `ascend`) live in `dispatch.rs`.

### State vs config (`config.rs`, `state.rs`)

Two separate persisted files, do not conflate:
- **config** (`$XDG_CONFIG_HOME/explorenova/config.toml`) — user-authored defaults:
  pane layout, `[keys]`, `preview_max_bytes`, `icons`, `confirm_trash`. Written with
  defaults on first run.
- **state** (`$XDG_DATA_HOME/explorenova/state.toml`) — runtime-captured: bookmarks,
  last cwd, live pane layout (visibility/width/weight/order), sort settings. Saved on
  quit. `--tree-only` is a non-persisted session override.

SMB shares live in a third file, `shares.toml` (`smb.rs`).

### Async operations (`ops.rs`)

Long file ops (copy/move/extract) run off-thread as an `OpState` polled each loop
iteration (`op.poll()`). On completion the loop refreshes, reveals the destination,
and on move-failure restores the cut clipboard (`cut_restore`) so the user can retry.

### Preview (`preview/`)

`PreviewCache::request` returns a cached `PreviewKind` or spawns a render; `drain`
pulls finished ones. `settle_ms(path)` gives a per-type debounce (heavy types wait
longer) that the loop uses to schedule its wake-up. Submodules: `text` (syntect
highlight), `media` (image/PDF), `archive`, `xlsx` (calamine).

## Conventions

- Errors: `anyhow` everywhere (`Result`, `bail!`, `anyhow!`). There is no custom
  error enum — `thiserror` was removed as unused. Don't reintroduce it for a one-off.
- Terminal teardown is RAII (`TermGuard`) plus a panic hook, so raw mode is always
  restored. Don't add manual restore paths.
- Mark intentional simplifications/known ceilings with a `ponytail:` comment.
