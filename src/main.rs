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

/// Collect the dirs the fs-watcher should currently cover: the active
/// arena root + every expanded directory node. Each is watched
/// NonRecursive so events arrive only for direct children — which is
/// what the visible tree actually shows.
fn watch_targets(app: &App) -> HashSet<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::new();
    out.insert(app.arena.nodes[app.arena.root].path.clone());
    for n in &app.arena.nodes {
        if n.is_dir() && n.expanded {
            out.insert(n.path.clone());
        }
    }
    out
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
    for p in &to_add {
        let _ = watcher.watch(p.as_path(), RecursiveMode::NonRecursive);
    }
    for p in &to_drop {
        let _ = watcher.unwatch(p.as_path());
    }
    *watched = desired;
}

fn load_config() -> (Config, Option<String>) {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => Config::load_str(&s),
        Err(_) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, explorenova::config::SAMPLE_CONFIG);
            (Config::with_defaults(), None)
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let sp = state_path();
    let state = std::fs::read_to_string(&sp)
        .map(|s| State::parse(&s))
        .unwrap_or_default();

    let (mut config, config_err) = load_config();
    state.apply_to(&mut config.panes);
    if args.tree_only {
        config.panes.bookmarks.visible = false;
        config.panes.preview.visible = false;
        config.panes.tree.visible = true;
    }

    let pwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let start = resolve_start_dir(args.path.clone(), state.last_cwd.clone(), pwd);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    let arena = Arena::new(&start)?;
    let mut app = App::new(arena, config, state.clone());
    if let Some(e) = config_err {
        app.set_err(format!("config error: {e} (using defaults)"));
    }
    let sharesp = shares_path();
    app.shares = explorenova::smb::load(&sharesp);
    app.shares_path = sharesp;
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

    let (git_tx, git_rx) = mpsc::channel::<PathBuf>();
    let (git_res_tx, git_res_rx) = mpsc::channel();
    {
        let start_root = start.clone();
        std::thread::spawn(move || {
            let send = |r| { let _ = git_res_tx.send(r); };
            send(scan_status(&start_root).unwrap_or_default());
            while let Ok(root) = git_rx.recv() {
                // Coalesce a burst of triggers into one scan of the latest root.
                let root = explorenova::git::coalesce_latest(root, git_rx.try_iter());
                send(scan_status(&root).unwrap_or_default());
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

    loop {
        // Pull in any preview that finished rendering off-thread; a fresh
        // one means the preview pane has new content to show.
        let preview_ready = preview_cache.drain();

        while let Ok(map) = git_res_rx.try_recv() {
            for n in app.arena.nodes.iter_mut() {
                if let Some(s) = map.get(&n.path) {
                    n.git = *s;
                }
            }
        }

        while let Ok(paths) = fs_rx.try_recv() {
            for p in paths {
                let dir = if watched.contains(&p) {
                    p.clone()
                } else {
                    p.parent().map(Path::to_path_buf).unwrap_or(p)
                };
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
        if !pending_dirs.is_empty() && fs_dirty_since.is_none() {
            fs_dirty_since = Some(Instant::now());
        }
        if let Some(t0) = fs_dirty_since {
            if t0.elapsed() >= Duration::from_millis(app.config.fs_refresh_debounce_ms) {
                let dirs: Vec<PathBuf> = pending_dirs.drain().collect();
                if !dirs.is_empty() {
                    refresh_dirs(&mut app, &dirs, &git_tx);
                }
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
                let res = state.finished.unwrap();
                let mut dirs = vec![app.arena.nodes[app.arena.root].path.clone()];
                if let Some(dst) = reveal.as_ref().and_then(|p| p.parent()) {
                    dirs.push(dst.to_path_buf());
                }
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
                        // Move failed — sources still exist; restore the cut
                        // clipboard so the user can retry.
                        if let Some(c) = app.cut_restore.take() {
                            app.clipboard = Some(c);
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
            continue;
        }
        if let Event::Key(k) = event::read()? {
            let prev_selected_outer = app.selected;
            // `key_token` only maps chord-relevant keys; it returns None for
            // Backspace/Home/End/etc. That gate belongs to Normal-mode chord
            // building only — the Search/Input/Confirm/AppPick arms consume
            // `k.code` directly and must see every key (esp. Backspace).
            handle_key(&mut app, &k, &keymap, &mut chord, &start, &git_tx)?;
            if app.selected != prev_selected_outer {
                app.cursor_moved_at = std::time::Instant::now();
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
                _ => explorenova::app::Dialog::Input,
            },
            Mode::SortMenu | Mode::Help => explorenova::app::Dialog::None,
        };

        // Reconcile fs-watches against the current expanded set. Cheap
        // when the diff is empty (most frames). Runs after dispatch so
        // newly-expanded dirs from this key event are watched before
        // the next render.
        sync_watches(&mut watcher, &mut watched, watch_targets(&app));

        if app.should_quit {
            break;
        }
    }

    // Persist the directory the user is actually in at quit, so the next
    // launch resumes there — not the directory we happened to start in.
    app.state.last_cwd = Some(app.arena.nodes[app.arena.root].path.clone());
    if !args.tree_only {
        app.state.capture_panes(&app.config.panes);
    }
    if let Some(parent) = sp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&sp, app.state.to_toml());
    // Shell-integration: drop the final directory so a wrapper can cd there.
    if let Some(cwd_file) = args.cwd_file.as_ref() {
        let _ = std::fs::write(
            cwd_file,
            app.arena.nodes[app.arena.root].path.to_string_lossy().as_bytes(),
        );
    }
    Ok(())
}
