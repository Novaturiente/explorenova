use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use explorenova::app::App;
use explorenova::cli::{resolve_start_dir, Args};
use explorenova::config::Config;
use explorenova::dispatch::{handle_key, refresh_in_place, refresh_dirs};
use explorenova::git::scan_status;
use explorenova::keymap::{Keymap, Mode};
use explorenova::preview::PreviewCache;
use explorenova::state::State;
use explorenova::tree::Arena;
use explorenova::ui;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the loop may rewrite `state.toml` when no key was pressed.
/// Key-driven changes are saved right away (see the loop).
const STATE_SAVE_INTERVAL: Duration = Duration::from_secs(5);

/// How often a keyless (timeout) tick may reconcile the fs-watch set.
const IDLE_WATCH_SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// RAII guard: always restores the terminal, even on panic/early return.
struct TermGuard;
impl TermGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TermGuard)
    }
}
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("explorenova").join("config.toml")
}

fn shares_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".config")
        });
    base.join("explorenova").join("shares.toml")
}

fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".local")
                .join("share")
        });
    base.join("explorenova").join("state.toml")
}

/// Capture the parts of `App` that belong in `state.toml` and serialize.
/// Returns `None` when the state cannot be represented (see
/// [`State::to_toml`]), in which case the caller must NOT write.
fn state_snapshot(app: &mut App, tree_only: bool) -> Option<String> {
    // Persist the directory the user is actually in, not the one we
    // happened to start in.
    app.state.last_cwd = Some(app.arena.nodes[app.arena.root].path.clone());
    if !tree_only {
        app.state.capture_panes(&app.config.panes);
    }
    app.state.to_toml()
}

/// Collect the dirs the fs-watcher should currently cover: every on-screen
/// pane's root + its expanded directory nodes. Each is watched
/// NonRecursive so events arrive only for direct children — which is
/// what the visible tree actually shows.
fn watch_targets(app: &App) -> HashSet<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::new();
    // Walk `visible`, the flattened reachable tree, not `arena.nodes`, the
    // flat store. Nodes orphaned by `reload_dir` keep `expanded == true` in
    // the store until a compaction, so the store version kept asking for
    // watches on directories that are no longer in the tree at all.
    let collect = |out: &mut HashSet<PathBuf>, arena: &explorenova::tree::Arena, visible: &[usize]| {
        out.insert(arena.nodes[arena.root].path.clone());
        for &id in visible {
            let n = &arena.nodes[id];
            if n.is_dir() && n.expanded {
                out.insert(n.path.clone());
            }
        }
    };
    collect(&mut out, &app.arena, &app.visible);
    // In split mode the other pane is on screen too, so it needs watching
    // as much as the focused one; without this it sat stale until you
    // swapped to it and pressed R.
    for snap in &app.tabs {
        collect(&mut out, &snap.arena, &snap.visible);
    }
    // Watch the enclosing repo's .git dir so external `git commit/add/
    // checkout` refresh the status glyphs without a manual R. HEAD and
    // index both live directly in .git (NonRecursive is enough).
    if let Some(gd) = git_dir_for(&app.arena.nodes[app.arena.root].path) {
        out.insert(gd);
    }
    out
}

/// Nearest `.git` directory at or above `root`, if any. A handful of
/// stats per call — cheap enough to run per keypress from watch_targets.
fn git_dir_for(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|a| a.join(".git"))
        .find(|g| g.is_dir())
}

/// Reconcile installed watches against `desired`. Adds new ones,
/// drops stale ones. Errors (typically EACCES on root-owned dirs)
/// are swallowed — the caller still gets coverage for everything
/// else and the user sees a missing-refresh symptom only on the
/// specific blocked path.
fn sync_watches(
    watcher: &mut notify::RecommendedWatcher,
    watched: &mut HashSet<PathBuf>,
    desired: HashSet<PathBuf>,
) {
    use notify::{RecursiveMode, Watcher};
    let to_add: Vec<PathBuf> = desired.difference(watched).cloned().collect();
    let to_drop: Vec<PathBuf> = watched.difference(&desired).cloned().collect();
    // Record only what actually installed. Assigning `desired` wholesale
    // marked a failed watch as present, and since `to_add` is computed as
    // `desired - watched`, that path was never passed to `watch()` again:
    // one EACCES, or one delete-and-recreate that made the kernel drop the
    // inotify watch, left that directory blind for the rest of the session.
    let mut installed: HashSet<PathBuf> = desired.intersection(watched).cloned().collect();
    for p in &to_add {
        if watcher.watch(p.as_path(), RecursiveMode::NonRecursive).is_ok() {
            installed.insert(p.clone());
        }
    }
    for p in &to_drop {
        let _ = watcher.unwatch(p.as_path());
    }
    *watched = installed;
}

/// Returns the parsed config, any parse error, and the pane keys the user
/// pinned explicitly (which `State::apply_to` must not override).
fn load_config() -> (Config, Option<String>, HashSet<String>) {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            let (c, err) = Config::load_str(&s);
            (c, err, explorenova::config::pinned_pane_keys(&s))
        }
        Err(_) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, explorenova::config::SAMPLE_CONFIG);
            // Every line in the sample is commented out, so nothing is
            // pinned on a first run — state persists the whole layout.
            (Config::with_defaults(), None, HashSet::new())
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let sp = state_path();
    let mut state = std::fs::read_to_string(&sp)
        .map(|s| State::parse(&s))
        .unwrap_or_default();
    state.prune_missing_pins();

    let (mut config, config_err, pinned_panes) = load_config();
    state.apply_to(&mut config.panes, &pinned_panes);
    if args.tree_only {
        config.panes.bookmarks.visible = false;
        config.panes.preview.visible = false;
        config.panes.tree.visible = true;
    }

    let pwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (start, start_warning) = resolve_start_dir(args.path.clone(), state.last_cwd.clone(), pwd);

    let default_hook = std::panic::take_hook();
    let ui_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        // Only the UI thread's panic ends the session. The preview worker
        // wraps its render in `catch_unwind` and carries on, but a panic
        // hook runs BEFORE unwinding, so tearing down unconditionally
        // dropped a perfectly healthy app out of the alternate screen and
        // painted a backtrace over it when syntect or `image` choked on a
        // malformed file.
        if std::thread::current().id() == ui_thread {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            default_hook(info);
        }
    }));

    let arena = Arena::new(&start)?;
    let mut app = App::new(arena, config, state.clone());
    if let Some(e) = config_err {
        app.set_err(format!("config error: {e} (using defaults)"));
    }
    if let Some(w) = start_warning {
        app.set_err(format!("{w} — opened {} instead", start.display()));
    }
    let sharesp = shares_path();
    app.shares = explorenova::smb::load(&sharesp);
    app.shares_path = sharesp;
    app.well_known = explorenova::sidebar::well_known_dirs();
    app.disks = explorenova::devices::build_rows(&app.shares);
    // Background auto-mount: fire `gio mount` per saved share off-thread
    // so the first draw is never blocked by network I/O. An unreachable
    // server / locked keyring fails quietly; the share just shows
    // unmounted. NOTE: gio mount has no connect timeout, so a worker
    // thread can block for the OS TCP timeout on a dead host — acceptable
    // for a detached thread that owns no resources.
    {
        let urls: Vec<String> =
            app.shares.iter().map(|s| s.url.clone()).collect();
        std::thread::spawn(move || {
            for url in urls {
                explorenova::smb::mount_background(&url);
            }
        });
    }
    let keymap = Keymap::build(&app.config.keys);
    // Validate [commands] against the FINAL keymap (after user [keys] overrides),
    // then store the filtered map. Dropped-key warnings join the startup status so
    // they aren't silently lost.
    let (commands, cmd_warnings) =
        explorenova::command::validate_commands(&app.config.commands, &keymap);
    app.commands = commands;
    if !cmd_warnings.is_empty() {
        let msg = format!("config: {}", cmd_warnings.join("; "));
        if app.status.is_empty() {
            app.set_status(msg);
        } else {
            let prev = app.status.clone();
            app.set_status(format!("{prev}; {msg}"));
        }
    }
    let mut preview_cache = PreviewCache::with_cap(app.config.preview.cache_cap);
    // Sweep day-old rasterized thumbnails from /tmp off-thread.
    std::thread::spawn(explorenova::preview::media::cleanup_stale_temps);

    let (git_tx, git_rx) = mpsc::channel::<PathBuf>();
    let (git_res_tx, git_res_rx) = mpsc::channel();
    {
        let start_root = start.clone();
        std::thread::spawn(move || {
            // Send the root each result describes, and send the failure as a
            // failure. `unwrap_or_default()` turned any git2 error — an index
            // lock held during a rebase, a permissions problem — into an empty
            // map, which the consumer reads as "every file is clean".
            let send = |root: PathBuf, r| { let _ = git_res_tx.send((root, r)); };
            send(start_root.clone(), scan_status(&start_root).ok());
            while let Ok(root) = git_rx.recv() {
                // Coalesce a burst of triggers into one scan of the latest root.
                let root = explorenova::git::coalesce_latest(root, git_rx.try_iter());
                send(root.clone(), scan_status(&root).ok());
            }
        });
    }

    let (fs_tx, fs_rx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(ev) = res {
            let ev: notify::Event = ev;
            if !ev.paths.is_empty() {
                let _ = fs_tx.send(ev.paths);
            }
        }
    })?;
    // Lazy per-dir watching (yazi-style). The set of watched dirs
    // mirrors the set of currently-expanded dirs in the arena. New
    // expansions get a NonRecursive watch installed; collapses /
    // reroots drop the now-irrelevant ones. Startup pays for exactly
    // one watch (the root) and never blocks on a subtree walk.
    let mut watched: HashSet<PathBuf> = HashSet::new();
    sync_watches(&mut watcher, &mut watched, watch_targets(&app));

    let mut picker = ui::make_picker();
    let _guard = TermGuard::new()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut chord = String::new();
    let mut img_cache = ui::ImageCache::default();
    let mut fs_dirty_since: Option<Instant> = None;
    let mut pending_dirs: HashSet<PathBuf> = HashSet::new();
    let mut git_dirty = false;
    // State used to be written once, after the loop, so a panic, a SIGTERM
    // or a kill lost the session's pins, pane layout and sort settings
    // outright. Now it is written whenever it actually changes, throttled
    // so a held key cannot turn into a write per frame.
    let mut last_saved: Option<String> = None;
    let mut last_save_at = Instant::now();
    let mut last_idle_sync = Instant::now();

    loop {
        // Pull in any preview that finished rendering off-thread; a fresh
        // one means the preview pane has new content to show.
        let preview_ready = preview_cache.drain() | app.meta.drain();

        while let Ok((scanned_root, map)) = git_res_rx.try_recv() {
            // A scan that started before a re-root describes a different
            // tree; applying it would blank every glyph in the new one, and
            // nothing would re-trigger a scan until the next fs event. The
            // re-root already queued its own scan, so dropping this one
            // costs nothing.
            if scanned_root != app.arena.nodes[app.arena.root].path {
                continue;
            }
            let Some(map) = map else { continue };
            // Absent-from-map means clean: resetting is what clears a
            // stale Modified/Added glyph after an external commit/stage.
            for n in app.arena.nodes.iter_mut() {
                n.git = map
                    .get(&n.path)
                    .copied()
                    .unwrap_or(explorenova::tree::GitStatus::Clean);
            }
        }

        while let Ok(paths) = fs_rx.try_recv() {
            for p in paths {
                // Events inside .git are git-metadata churn (commit,
                // stage, checkout) — they re-trigger the status scan,
                // not a tree refresh.
                if p.components().any(|c| c.as_os_str() == ".git") {
                    git_dirty = true;
                    continue;
                }
                // `watched.contains` alone sent a deleted directory's own
                // removal event to itself; `reload_dir` then hit its
                // `Err(_) => return true` arm, reported success, and left
                // the gone directory on screen as a ghost row. A path that
                // is no longer a directory belongs to its parent.
                let dir = if watched.contains(&p) && p.is_dir() {
                    p.clone()
                } else {
                    p.parent().map(Path::to_path_buf).unwrap_or(p.clone())
                };
                // Only `p`'s own preview went stale, plus `dir`'s (its
                // preview is an entry count, which a create/delete moves).
                // Invalidating the whole dir would re-render the file the
                // user is *looking at* every time an unrelated sibling
                // changes — a log file or download churning in the same
                // directory made the preview flash back to "(loading…)".
                preview_cache.invalidate_path(&p);
                preview_cache.invalidate_path(&dir);
                pending_dirs.insert(dir);
            }
        }
        if let Some(until) = app.self_write_until {
            if Instant::now() < until {
                for d in &app.self_write_dirs {
                    pending_dirs.remove(d);
                }
            } else {
                app.self_write_until = None;
                app.self_write_dirs.clear();
            }
        }
        if (!pending_dirs.is_empty() || git_dirty) && fs_dirty_since.is_none() {
            fs_dirty_since = Some(Instant::now());
        }
        if let Some(t0) = fs_dirty_since {
            if t0.elapsed() >= Duration::from_millis(app.config.fs_refresh_debounce_ms) {
                let dirs: Vec<PathBuf> = pending_dirs.drain().collect();
                if !dirs.is_empty() {
                    app.meta.invalidate();
                    refresh_dirs(&mut app, &dirs, &git_tx);
                } else if git_dirty {
                    // Pure .git churn: rescan status only (refresh_dirs
                    // already re-triggers the scan when it runs).
                    let _ = git_tx.send(app.arena.nodes[app.arena.root].path.clone());
                }
                git_dirty = false;
                fs_dirty_since = None;
            }
        }

        // Poll any in-flight background op (copy/move/extract). On
        // terminal status, refresh the tree, reveal the destination,
        // and surface the worker's status text.
        if let Some(op) = app.op.as_mut() {
            op.poll();
            if op.finished.is_some() {
                let state = app.op.take().unwrap();
                let reveal = state.reveal_hint.clone();
                let kind = state.kind;
                let res = state.finished.unwrap();
                app.op_history.push(explorenova::ops::OpRecord {
                    kind,
                    label: state.label.clone(),
                    result: res.clone(),
                });
                // shortcut: session history capped at 50, never persisted.
                if app.op_history.len() > 50 {
                    app.op_history.remove(0);
                }
                // A background mount or unmount changes what the sidebar's
                // Disks section should say. `app.disks` was otherwise only
                // assigned at startup and on the R key, so the share stayed
                // listed as unmounted until the user noticed and refreshed.
                if matches!(kind, explorenova::ops::OpKind::Mount | explorenova::ops::OpKind::Unmount) {
                    app.disks = explorenova::devices::build_rows(&app.shares);
                }
                let mut dirs = vec![app.arena.nodes[app.arena.root].path.clone()];
                if let Some(dst) = reveal.as_ref().and_then(|p| p.parent()) {
                    dirs.push(dst.to_path_buf());
                }
                for d in &dirs {
                    preview_cache.invalidate_dir(d);
                }
                app.meta.invalidate();
                app.mark_self_write(dirs);
                refresh_in_place(&mut app, &git_tx);
                if let Some(target) = reveal {
                    app.navigate_to(&target);
                }
                match res {
                    Ok(s) => {
                        app.cut_restore = None; // move succeeded; drop the stash
                        app.set_status(s);
                    }
                    Err(e) => {
                        // Move failed — restore the cut clipboard so the user
                        // can retry, but only for sources that are still
                        // there. Restoring the whole stash meant a five-item
                        // cut where four moved handed back all five, and the
                        // retry reported four spurious failures for files
                        // that had already arrived.
                        if let Some((paths, is_cut)) = app.cut_restore.take() {
                            let remaining: Vec<PathBuf> = paths
                                .into_iter()
                                .filter(|p| std::fs::symlink_metadata(p).is_ok())
                                .collect();
                            if !remaining.is_empty() {
                                app.clipboard = Some((remaining, is_cut));
                            }
                        }
                        app.set_err(format!("op failed: {e}"));
                    }
                };
            }
        }

        app.refresh_disk_free();
        // Lazily probe empty-dir markers only for the on-screen window,
        // so the marker survives without an O(tree) read_dir storm.
        if let Ok(sz) = term.size() {
            let win = sz.height as usize;
            let start = app.selected.saturating_sub(win);
            let end = (app.selected + win).min(app.visible.len());
            let ids: Vec<usize> = app.visible[start..end].to_vec();
            app.arena.probe_empty(&ids);
            // Tree pane visible rows ≈ height minus header, status, borders.
            // Drives the half-page jump (Ctrl-d/u).
            app.viewport_h = win.saturating_sub(4);
        }
        // A child program (editor via `!`, a `[commands]` key, the `\` picker,
        // drop-to-shell, SMB mount) clobbered the alternate screen. ratatui's
        // cached frame still matches the new one, so a plain draw would diff to
        // nothing and leave the child's leftover content on screen. Force a full
        // repaint once.
        if app.force_redraw {
            term.clear()?;
            // A child process (editor, `!`, a `[commands]` key, drop-to-
            // shell) had the terminal and may have rewritten anything —
            // cheaper and safer to drop the whole preview cache than try
            // to enumerate what it touched.
            preview_cache.clear();
            app.force_redraw = false;
        }
        term.draw(|f| {
            ui::render_frame(f, &mut app, &mut preview_cache, &mut picker, &mut img_cache, &keymap)
        })?;

        // Wake the loop exactly when the current row's preview settle
        // expires, so the (debounced) preview renders without an extra
        // keypress. Per-type so heavy previews wait their longer settle.
        let preview_debounce_ms = app
            .visible
            .get(app.selected)
            .map(|&id| explorenova::preview::settle_ms(
                &app.arena.nodes[id].path,
                app.config.preview.settle_cheap_ms,
                app.config.preview.settle_heavy_ms,
            ))
            .unwrap_or(150);
        let poll_ms = {
            let elapsed = app.cursor_moved_at.elapsed().as_millis() as u64;
            let remaining = preview_debounce_ms.saturating_sub(elapsed);
            // 200ms is the existing default; never wait longer than that
            // (so fs/git channels still get drained promptly).
            // When debounce is already past, remaining == 0 and we use 200.
            let base = if remaining == 0 { 200 } else { remaining.min(200) };
            // During a long op, poll on a short tick so the progress
            // gauge / spinner refreshes smoothly even when no key event
            // arrives.
            if app.op.is_some() { base.min(80) } else if preview_ready { 0 } else { base }
        };
        if !event::poll(Duration::from_millis(poll_ms))? {
            // A timeout tick still has to reconcile: an op completion
            // expands every ancestor of its destination with no key event
            // involved, and those directories stayed unwatched until the
            // user next pressed something. Throttled, because the expansions
            // that reach here are rare and `watch_targets` stats every
            // ancestor looking for a .git dir.
            if last_idle_sync.elapsed() >= IDLE_WATCH_SYNC_INTERVAL {
                last_idle_sync = Instant::now();
                sync_watches(&mut watcher, &mut watched, watch_targets(&app));
            }
            continue;
        }
        let mut key_handled = false;
        if let Event::Key(k) = event::read()? {
            if k.kind == crossterm::event::KeyEventKind::Release {
                continue;
            }
            key_handled = true;
            let prev_selected_outer = app.selected;
            // `key_token` only maps chord-relevant keys; it returns None for
            // Backspace/Home/End/etc. That gate belongs to Normal-mode chord
            // building only — the Search/Input/Confirm/AppPick arms consume
            // `k.code` directly and must see every key (esp. Backspace).
            handle_key(&mut app, &k, &keymap, &mut chord, &start, &git_tx)?;
            // Mirror the chord buffer so the status bar can render a
            // pending multi-key sequence (vim showcmd).
            app.pending_chord = chord.clone();
            if app.selected != prev_selected_outer {
                app.cursor_moved_at = std::time::Instant::now();
                app.preview_scroll = 0;
                if app.visual_mode {
                    // Mark every row crossed during the jump (gg, G,
                    // Ctrl-d, Ctrl-u all skip multiple rows). Single-
                    // step j/k still works because lo == hi - 1.
                    let (lo, hi) = if app.selected < prev_selected_outer {
                        (app.selected, prev_selected_outer)
                    } else {
                        (prev_selected_outer, app.selected)
                    };
                    app.mark_range(lo, hi);
                }
            }
        }

        app.dialog = match app.mode {
            Mode::Normal => explorenova::app::Dialog::None,
            Mode::Confirm => explorenova::app::Dialog::Confirm,
            Mode::Search | Mode::Input => match std::mem::take(&mut app.dialog) {
                explorenova::app::Dialog::AppPick { items, sel, filter } => {
                    explorenova::app::Dialog::AppPick { items, sel, filter }
                }
                explorenova::app::Dialog::Devices { items, sel } => {
                    explorenova::app::Dialog::Devices { items, sel }
                }
                explorenova::app::Dialog::ScriptPick { items, sel, filter } => {
                    explorenova::app::Dialog::ScriptPick { items, sel, filter }
                }
                _ => explorenova::app::Dialog::Input,
            },
            Mode::SortMenu | Mode::Help => explorenova::app::Dialog::None,
        };

        // Reconcile fs-watches against the current expanded set. Cheap
        // when the diff is empty (most frames). Runs after dispatch so
        // newly-expanded dirs from this key event are watched before
        // the next render.
        sync_watches(&mut watcher, &mut watched, watch_targets(&app));

        // Save right after a key that changed state: closing the terminal
        // window kills the process with SIGHUP, so waiting for the interval
        // (or for `q`) lost a sort/pin change made just before closing.
        // The snapshot compare below keeps unchanged keys write-free.
        if key_handled || last_save_at.elapsed() >= STATE_SAVE_INTERVAL {
            last_save_at = Instant::now();
            if let Some(toml) = state_snapshot(&mut app, args.tree_only) {
                if last_saved.as_deref() != Some(toml.as_str()) {
                    if let Some(parent) = sp.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = explorenova::fs_ops::write_atomic(&sp, &toml);
                    last_saved = Some(toml);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    if let Some(toml) = state_snapshot(&mut app, args.tree_only) {
        if let Some(parent) = sp.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = explorenova::fs_ops::write_atomic(&sp, &toml);
    }
    // Shell-integration: drop the final directory so a wrapper can cd there.
    if let Some(cwd_file) = args.cwd_file.as_ref() {
        let _ = std::fs::write(
            cwd_file,
            app.arena.nodes[app.arena.root].path.to_string_lossy().as_bytes(),
        );
    }
    Ok(())
}
