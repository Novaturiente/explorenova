use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use explorenova::app::App;
use explorenova::cli::{resolve_start_dir, Args};
use explorenova::config::Config;
use explorenova::git::scan_status;
use explorenova::keymap::{key_token, Action, Keymap, Mode};
use explorenova::preview::PreviewCache;
use explorenova::state::State;
use explorenova::tree::Arena;
use explorenova::ui;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
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

/// Case-insensitive substring filter. Returns indices into `items`
/// whose `.name` contains every space-separated token in `filter`.
fn filter_app_indices(
    items: &[explorenova::apps::AppEntry],
    filter: &str,
) -> Vec<usize> {
    if filter.is_empty() {
        return (0..items.len()).collect();
    }
    let needle = filter.to_lowercase();
    let tokens: Vec<&str> = needle.split_whitespace().collect();
    items
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let hay = a.name.to_lowercase();
            if tokens.iter().all(|t| hay.contains(t)) {
                Some(i)
            } else {
                None
            }
        })
        .collect()
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

fn load_config() -> Config {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => Config::load_str(&s).0,
        Err(_) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let c = Config::with_defaults();
            if let Ok(s) = toml::to_string_pretty(&c) {
                let _ = std::fs::write(&p, format!("# explorenova config\n{s}"));
            }
            c
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let sp = state_path();
    let state = std::fs::read_to_string(&sp)
        .map(|s| State::parse(&s))
        .unwrap_or_default();

    let mut config = load_config();
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
    let keymap = Keymap::build(&app.config.keys);
    let mut preview_cache = PreviewCache::default();

    let (git_tx, git_rx) = mpsc::channel::<()>();
    let (git_res_tx, git_res_rx) = mpsc::channel();
    {
        let root = start.clone();
        std::thread::spawn(move || {
            let send = |r| { let _ = git_res_tx.send(r); };
            send(scan_status(&root).unwrap_or_default());
            while git_rx.recv().is_ok() {
                send(scan_status(&root).unwrap_or_default());
            }
        });
    }

    use notify::{RecursiveMode, Watcher};
    let (fs_tx, fs_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(ev) = res {
            let _ = fs_tx.send(ev);
        }
    })?;
    watcher.watch(&start, RecursiveMode::Recursive)?;

    let mut picker = ui::make_picker();
    let _guard = TermGuard::new()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut chord = String::new();
    let mut prev_status = String::new();
    let mut img_cache = ui::ImageCache::default();
    let mut fs_dirty_since: Option<Instant> = None;

    loop {
        while let Ok(map) = git_res_rx.try_recv() {
            for n in app.arena.nodes.iter_mut() {
                if let Some(s) = map.get(&n.path) {
                    n.git = *s;
                }
            }
        }

        let mut fs_changed = false;
        while fs_rx.try_recv().is_ok() {
            fs_changed = true;
        }
        if fs_changed && fs_dirty_since.is_none() {
            fs_dirty_since = Some(Instant::now());
        }
        if let Some(t0) = fs_dirty_since {
            if t0.elapsed() >= Duration::from_millis(150) {
                // Rebuild at the CURRENT arena root, not the startup
                // `start` arg — after H/T/devices/ascend the user may
                // be rooted elsewhere and we'd otherwise yank them back.
                refresh_in_place(&mut app, &git_tx);
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
                refresh_in_place(&mut app, &git_tx);
                if let Some(target) = reveal {
                    app.navigate_to(&target);
                }
                app.status = match res {
                    Ok(s) => s,
                    Err(e) => format!("op failed: {e}"),
                };
            }
        }

        // Auto-stamp status timestamp on any change so the 3s fade in the
        // status bar Left segment behaves whether the assignment went
        // through `set_status` or a direct `app.status = X`.
        if app.status != prev_status {
            app.status_set_at = std::time::Instant::now();
            prev_status = app.status.clone();
        }
        app.refresh_disk_free();
        term.draw(|f| {
            ui::render_frame(f, &mut app, &mut preview_cache, &mut picker, &mut img_cache, &keymap)
        })?;

        const PREVIEW_DEBOUNCE_MS: u64 = 150;
        let poll_ms = {
            let elapsed = app.cursor_moved_at.elapsed().as_millis() as u64;
            let remaining = PREVIEW_DEBOUNCE_MS.saturating_sub(elapsed);
            // 200ms is the existing default; never wait longer than that
            // (so fs/git channels still get drained promptly).
            // When debounce is already past, remaining == 0 and we use 200.
            let base = if remaining == 0 { 200 } else { remaining.min(200) };
            // During a long op, poll on a short tick so the progress
            // gauge / spinner refreshes smoothly even when no key event
            // arrives.
            if app.op.is_some() { base.min(80) } else { base }
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
            match app.mode {
                Mode::Normal => {
                    let Some(tok) = key_token(&k) else { continue };
                    chord.push_str(&tok);
                    if let Some(action) = keymap.resolve(Mode::Normal, &chord) {
                        chord.clear();
                        handle(&mut app, action, &start, &git_tx)?;
                    } else if keymap.is_prefix(&chord) {
                        // wait for more keys
                    } else {
                        chord.clear();
                    }
                }
                Mode::SortMenu => {
                    let Some(tok) = key_token(&k) else { continue };
                    if let Some(action) = keymap.resolve(Mode::SortMenu, &tok) {
                        if action == Action::Cancel {
                            app.mode = Mode::Normal;
                        } else {
                            handle(&mut app, action, &start, &git_tx)?;
                        }
                    }
                }
                Mode::Help => {
                    // Arrow keys + Ctrl-d/u scroll; Backspace trims
                    // filter; Esc closes (or clears filter first when
                    // non-empty); printable chars append to filter.
                    let scroll_step = |app: &mut explorenova::app::App, by: i32| {
                        if by < 0 {
                            app.help_scroll =
                                app.help_scroll.saturating_sub((-by) as usize);
                        } else {
                            app.help_scroll =
                                app.help_scroll.saturating_add(by as usize);
                        }
                    };
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    match (k.code, ctrl) {
                        (KeyCode::Down, _) => scroll_step(&mut app, 1),
                        (KeyCode::Up, _) => scroll_step(&mut app, -1),
                        (KeyCode::PageDown, _) => scroll_step(&mut app, 8),
                        (KeyCode::PageUp, _) => scroll_step(&mut app, -8),
                        (KeyCode::Char('d'), true) => scroll_step(&mut app, 8),
                        (KeyCode::Char('u'), true) => scroll_step(&mut app, -8),
                        (KeyCode::Home, _) => app.help_scroll = 0,
                        (KeyCode::End, _) => app.help_scroll = usize::MAX / 2,
                        (KeyCode::Esc, _) => {
                            if app.help_filter.is_empty() {
                                app.mode = Mode::Normal;
                                app.help_scroll = 0;
                            } else {
                                app.help_filter.clear();
                                app.help_scroll = 0;
                            }
                        }
                        (KeyCode::Backspace, _) => {
                            app.help_filter.pop();
                            app.help_scroll = 0;
                        }
                        (KeyCode::Char(c), false) => {
                            // Allow any printable char for the filter.
                            // Space, punctuation, digits all useful for
                            // multi-token search (e.g. "ctrl t").
                            app.help_filter.push(c);
                            app.help_scroll = 0;
                        }
                        _ => {
                            // Enter / unknown key: close.
                            app.mode = Mode::Normal;
                            app.help_scroll = 0;
                            app.help_filter.clear();
                        }
                    }
                }
                Mode::Search | Mode::Input
                    if matches!(
                        app.dialog,
                        explorenova::app::Dialog::Devices { .. }
                    ) =>
                {
                    use explorenova::app::Dialog;
                    use crossterm::event::KeyCode;
                    let len = match &app.dialog {
                        Dialog::Devices { items, .. } => items.len(),
                        _ => 0,
                    };
                    match k.code {
                        KeyCode::Esc | KeyCode::Char('M') => {
                            app.mode = Mode::Normal;
                            app.dialog = Dialog::None;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if let Dialog::Devices { sel, .. } = &mut app.dialog {
                                *sel = explorenova::ui::modal::wrap_nav(
                                    *sel, len, true,
                                );
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if let Dialog::Devices { sel, .. } = &mut app.dialog {
                                *sel = explorenova::ui::modal::wrap_nav(
                                    *sel, len, false,
                                );
                            }
                        }
                        KeyCode::Char('r') => {
                            match explorenova::devices::list() {
                                Ok(items) => {
                                    let new_len = items.len();
                                    if let Dialog::Devices { items: it, sel } =
                                        &mut app.dialog
                                    {
                                        *it = items;
                                        if *sel >= new_len {
                                            *sel = new_len.saturating_sub(1);
                                        }
                                    }
                                }
                                Err(e) => {
                                    app.status = format!("devices: {e}");
                                }
                            }
                        }
                        KeyCode::Char('u') => {
                            let dev_opt = if let Dialog::Devices {
                                items, sel,
                            } = &app.dialog
                            {
                                items.get(*sel).cloned()
                            } else {
                                None
                            };
                            if let Some(d) = dev_opt {
                                if d.is_mounted() {
                                    match explorenova::devices::unmount(&d.device) {
                                        Ok(_) => {
                                            app.status = format!(
                                                "unmounted {}",
                                                d.device
                                            );
                                        }
                                        Err(e) => {
                                            app.status =
                                                format!("umount failed: {e}");
                                        }
                                    }
                                    // refresh
                                    if let Ok(items) = explorenova::devices::list()
                                    {
                                        let new_len = items.len();
                                        if let Dialog::Devices {
                                            items: it,
                                            sel,
                                        } = &mut app.dialog
                                        {
                                            *it = items;
                                            if *sel >= new_len {
                                                *sel = new_len.saturating_sub(1);
                                            }
                                        }
                                    }
                                } else {
                                    app.status = "not mounted".into();
                                }
                            }
                        }
                        KeyCode::Enter => {
                            let dev_opt = if let Dialog::Devices {
                                items, sel,
                            } = &app.dialog
                            {
                                items.get(*sel).cloned()
                            } else {
                                None
                            };
                            if let Some(d) = dev_opt {
                                if let Some(mp) = d.mountpoint.clone() {
                                    // mounted → cd then close. Try reveal
                                    // within current arena first; if the
                                    // mountpoint is outside (typical: /run/
                                    // media/...), re-root the arena at the
                                    // mountpoint.
                                    if !app.navigate_to(&mp) {
                                        let prev_hidden = app.arena.show_hidden;
                                        match Arena::new(&mp) {
                                            Ok(mut na) => {
                                                if prev_hidden {
                                                    na.show_hidden = true;
                                                    na.nodes[na.root].children = None;
                                                    let _ = na.load_children(na.root);
                                                    na.nodes[na.root].expanded = true;
                                                }
                                                app.arena = na;
                                                app.visible = app.arena.flatten();
                                                app.selected = 0;
                                                app.status = format!(
                                                    "rooted at {}",
                                                    mp.display()
                                                );
                                            }
                                            Err(e) => {
                                                app.status = format!(
                                                    "cannot open {}: {e}",
                                                    mp.display()
                                                );
                                            }
                                        }
                                    }
                                    app.mode = Mode::Normal;
                                    app.dialog = Dialog::None;
                                } else {
                                    // unmounted → mount + refresh, stay
                                    match explorenova::devices::mount(&d.device) {
                                        Ok(_) => {
                                            app.status =
                                                format!("mounted {}", d.device);
                                        }
                                        Err(e) => {
                                            app.status =
                                                format!("mount failed: {e}");
                                        }
                                    }
                                    if let Ok(items) =
                                        explorenova::devices::list()
                                    {
                                        let new_len = items.len();
                                        if let Dialog::Devices {
                                            items: it,
                                            sel,
                                        } = &mut app.dialog
                                        {
                                            *it = items;
                                            if *sel >= new_len {
                                                *sel = new_len.saturating_sub(1);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Mode::Search | Mode::Input
                    if matches!(
                        app.dialog,
                        explorenova::app::Dialog::AppPick { .. }
                    ) =>
                {
                    use explorenova::app::Dialog;
                    // `len` = filtered apps + 1 (custom-command row).
                    // `sel` is the index into that filtered+custom view.
                    let (len, sel, filtered_idx) = match &app.dialog {
                        Dialog::AppPick { items, sel, filter } => {
                            let fi = filter_app_indices(items, filter);
                            (fi.len() + 1, *sel, fi)
                        }
                        _ => (1, 0, Vec::new()),
                    };
                    match k.code {
                        KeyCode::Esc => {
                            app.mode = Mode::Normal;
                            app.pending = None;
                            app.dialog = Dialog::None;
                        }
                        KeyCode::Up => {
                            if let Dialog::AppPick { sel, .. } = &mut app.dialog {
                                *sel = explorenova::ui::modal::wrap_nav(
                                    *sel, len, false,
                                );
                            }
                        }
                        KeyCode::Down => {
                            if let Dialog::AppPick { sel, .. } = &mut app.dialog {
                                *sel = explorenova::ui::modal::wrap_nav(
                                    *sel, len, true,
                                );
                            }
                        }
                        KeyCode::Backspace => {
                            if let Dialog::AppPick { sel, filter, .. } =
                                &mut app.dialog
                            {
                                filter.pop();
                                *sel = 0;
                            }
                        }
                        KeyCode::Char(c) => {
                            if let Dialog::AppPick { sel, filter, .. } =
                                &mut app.dialog
                            {
                                filter.push(c);
                                *sel = 0;
                            }
                        }
                        KeyCode::Enter => {
                            if sel == len - 1 {
                                // "Custom command…" → typed-command path
                                app.input.clear();
                                app.pending = Some(Action::OpenWith);
                                app.mode = Mode::Input;
                                app.dialog = Dialog::Input;
                            } else {
                                let entry = if let Dialog::AppPick {
                                    items,
                                    ..
                                } = &app.dialog
                                {
                                    filtered_idx
                                        .get(sel)
                                        .and_then(|&i| items.get(i).cloned())
                                } else {
                                    None
                                };
                                if let (Some(entry), Some(&id)) =
                                    (entry, app.visible.get(app.selected))
                                {
                                    let path =
                                        app.arena.nodes[id].path.clone();
                                    if let Err(e) =
                                        explorenova::apps::spawn(&entry, &path)
                                    {
                                        app.status =
                                            format!("open failed: {e}");
                                    }
                                }
                                app.mode = Mode::Normal;
                                app.pending = None;
                                app.dialog = Dialog::None;
                            }
                        }
                        _ => {}
                    }
                }
                Mode::Search | Mode::Input => match k.code {
                    KeyCode::Esc => {
                        if app.mode == Mode::Search {
                            app.end_search();
                        }
                        app.mode = Mode::Normal;
                        app.input.clear();
                        app.pending = None;
                    }
                    KeyCode::Enter => match app.mode {
                        Mode::Search => {
                            // Cursor is already on the top live match;
                            // Enter just dismisses the prompt. Cached
                            // candidates can be freed now.
                            app.end_search();
                            app.mode = Mode::Normal;
                        }
                        Mode::Input => match app.pending.take() {
                            Some(Action::Rename) => {
                                let np = app.commit_rename();
                                refresh_in_place(&mut app, &git_tx);
                                if let Some(p) = np {
                                    app.navigate_to(&p);
                                }
                            }
                            Some(Action::Create) => {
                                let np = app.commit_create();
                                refresh_in_place(&mut app, &git_tx);
                                if let Some(p) = np {
                                    app.navigate_to(&p);
                                }
                            }
                            Some(Action::PathInput) => {
                                let target = explorenova::nav::expand(&app.input.value());
                                if !app.navigate_to(std::path::Path::new(&target)) {
                                    app.status = "path not found".into();
                                }
                                app.mode = Mode::Normal;
                            }
                            Some(Action::OpenWith) => {
                                if let Some(&id) = app.visible.get(app.selected) {
                                    let p = app.arena.nodes[id].path.clone();
                                    if let Err(e) =
                                        std::process::Command::new(app.input.value())
                                            .arg(&p)
                                            .spawn()
                                    {
                                        app.status = format!("open failed: {e}");
                                    }
                                }
                                app.mode = Mode::Normal;
                            }
                            Some(Action::ShellOnce) => {
                                let cmd = app.input.value();
                                app.mode = Mode::Normal;
                                if let Err(e) = explorenova::shell::run_once(&start, &cmd) {
                                    app.status = format!("shell failed: {e}");
                                }
                            }
                            _ => app.mode = Mode::Normal,
                        },
                        _ => {}
                    },
                    KeyCode::Tab => {
                        if app.mode == Mode::Input
                            && app.pending == Some(Action::PathInput)
                        {
                            let c = explorenova::nav::completions(&app.input.value());
                            if let Some(lcp) =
                                explorenova::nav::longest_common(&app.input.value(), &c)
                            {
                                app.input.set(&lcp);
                            }
                        }
                    }
                    KeyCode::Left => app.input.left(),
                    KeyCode::Right => app.input.right(),
                    KeyCode::Home => app.input.home(),
                    KeyCode::End => app.input.end(),
                    KeyCode::Delete => {
                        app.input.delete();
                        if app.mode == Mode::Search {
                            app.live_search();
                        }
                    }
                    KeyCode::Backspace => {
                        app.input.backspace();
                        if app.mode == Mode::Search {
                            app.live_search();
                        }
                    }
                    // Some terminals send Backspace as DEL (0x7f) or Ctrl-H (0x08).
                    KeyCode::Char('\u{7f}') | KeyCode::Char('\u{8}') => {
                        app.input.backspace();
                        if app.mode == Mode::Search {
                            app.live_search();
                        }
                    }
                    KeyCode::Char(ch) if !ch.is_control() => {
                        app.input.insert(ch);
                        if app.mode == Mode::Search {
                            app.live_search();
                        }
                    }
                    _ => {}
                },
                Mode::Confirm => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        app.confirm_yes();
                        // confirm_yes lives in app.rs and can't see the
                        // git_tx — refresh from here so the freshly
                        // unlinked entries drop out of the tree at once.
                        refresh_in_place(&mut app, &git_tx);
                    }
                    _ => {
                        app.pending = None;
                        app.mode = Mode::Normal;
                        app.status.clear();
                    }
                },
            }
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

        if app.should_quit {
            break;
        }
    }

    app.state.last_cwd = Some(start.clone());
    if !args.tree_only {
        app.state.capture_panes(&app.config.panes);
    }
    if let Some(parent) = sp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&sp, app.state.to_toml());
    Ok(())
}

/// Bridge actions that need IO to their modules; pure actions go through App::apply.
/// Re-root the tree to the parent of the current root, selecting the
/// directory just left. No-op at the filesystem root. Filesystem IO, so
/// it lives here (not in the pure `App::apply`).
/// Re-root the arena at `path`, preserving hidden-files toggle, resetting
/// selection, and kicking a git rescan. Used by JumpHome, OpenTrash, and
/// the devices-modal "navigate outside current root" fallback.
fn reroot(app: &mut App, path: &std::path::Path, git_tx: &mpsc::Sender<()>) {
    let prev_hidden = app.arena.show_hidden;
    match Arena::new(path) {
        Ok(mut na) => {
            if prev_hidden {
                na.show_hidden = true;
                na.nodes[na.root].children = None;
                let _ = na.load_children(na.root);
                na.nodes[na.root].expanded = true;
            }
            app.arena = na;
            app.visible = app.arena.flatten();
            app.selected = 0;
            app.status = format!("rooted at {}", path.display());
            let _ = git_tx.send(());
        }
        Err(e) => {
            app.status = format!("cannot open {}: {e}", path.display());
        }
    }
}

/// Rebuild the arena at the current root, preserving the show_hidden
/// toggle, ALL previously-expanded directories, and the cursor path.
/// Used after destructive batch ops (trash, permanent delete, cut-paste)
/// to drop stale entries immediately instead of waiting on the fs-watch
/// debounce — which can miss `trash` crate moves entirely when they go
/// through D-Bus.
fn refresh_in_place(app: &mut App, git_tx: &mpsc::Sender<()>) {
    let cur_root = app.arena.nodes[app.arena.root].path.clone();
    let prev_hidden = app.arena.show_hidden;
    let sel_path = app
        .visible
        .get(app.selected)
        .map(|&id| app.arena.nodes[id].path.clone());
    // Snapshot every expanded directory path so we can replay the
    // expansion state on the new arena. Filter to dirs that still
    // exist on disk so stale entries (the ones the user just deleted)
    // don't fail the reveal silently and produce a half-collapsed tree.
    let expanded_paths: Vec<std::path::PathBuf> = app
        .arena
        .nodes
        .iter()
        .filter(|n| n.is_dir() && n.expanded)
        .map(|n| n.path.clone())
        .filter(|p| p.exists())
        .collect();
    if let Ok(mut na) = Arena::new(&cur_root) {
        if prev_hidden {
            na.show_hidden = true;
            na.nodes[na.root].children = None;
            let _ = na.load_children(na.root);
        }
        na.nodes[na.root].expanded = true;
        app.arena = na;
        // Replay expansions. `arena.reveal` only expands ANCESTORS of
        // the target — the target node itself stays collapsed. So we
        // reveal each previously-expanded dir AND then explicitly
        // expand the returned node. Paths outside the new root are
        // skipped (reveal returns Ok(None)).
        for p in &expanded_paths {
            if let Ok(Some(id)) = app.arena.reveal(p) {
                let _ = app.arena.expand(id);
            }
        }
        app.visible = app.arena.flatten();
        app.selected = 0;
        // Restore cursor. If the original path was deleted (Trash /
        // PermanentDelete), reveal returns Ok(None) — walk up to the
        // nearest existing ancestor so the user lands on the parent
        // of what they just removed rather than the tree root.
        if let Some(mut p) = sel_path {
            loop {
                if app.navigate_to(&p) {
                    break;
                }
                match p.parent() {
                    Some(parent) if parent != p.as_path() => {
                        p = parent.to_path_buf();
                    }
                    _ => break,
                }
            }
        }
        if !app.visible.is_empty() && app.selected >= app.visible.len() {
            app.selected = app.visible.len() - 1;
        }
    }
    let _ = git_tx.send(());
}

fn ascend(app: &mut App) {
    let root_path = app.arena.nodes[app.arena.root].path.clone();
    let parent = match root_path.parent() {
        Some(p) if p != root_path => p.to_path_buf(),
        _ => {
            app.status = "at filesystem root".into();
            return;
        }
    };
    let prev_hidden = app.arena.show_hidden;
    match Arena::new(&parent) {
        Ok(mut na) => {
            if prev_hidden {
                na.show_hidden = true;
                na.nodes[na.root].children = None;
                let _ = na.load_children(na.root);
                na.nodes[na.root].expanded = true;
            }
            app.arena = na;
            app.visible = app.arena.flatten();
            app.selected = 0;
            app.navigate_to(&root_path);
        }
        Err(_) => {
            app.status = format!("cannot open {}", parent.display());
        }
    }
}

fn handle(
    app: &mut App,
    action: Action,
    cwd: &std::path::Path,
    git_tx: &mpsc::Sender<()>,
) -> Result<()> {
    use explorenova::shell;
    match action {
        Action::OpenDefault => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if !app.arena.nodes[id].is_dir() {
                    let _ = open::that_detached(&p);
                }
            }
        }
        Action::ShellDrop => {
            // Don't let a shell hiccup tear down the TUI. Convert any
            // io::Error (raw-mode toggle failure, alt-screen restore
            // glitch, stdin EOF on readline) into a status message and
            // keep the app alive.
            if let Err(e) = shell::drop_to_shell(cwd) {
                app.status = format!("shell: {e}");
            }
        }
        Action::ShowDevices => {
            match explorenova::devices::list() {
                Ok(items) => {
                    app.dialog =
                        explorenova::app::Dialog::Devices { items, sel: 0 };
                    app.mode = explorenova::keymap::Mode::Input;
                }
                Err(e) => {
                    app.status = format!("devices: {e}");
                }
            }
        }
        Action::Refresh => {
            let _ = git_tx.send(());
        }
        Action::JumpHome => {
            let home = match std::env::var_os("HOME") {
                Some(h) => std::path::PathBuf::from(h),
                None => {
                    app.status = "$HOME not set".into();
                    return Ok(());
                }
            };
            reroot(app, &home, git_tx);
        }
        Action::OpenTrash => {
            // XDG: $XDG_DATA_HOME/Trash/files, falling back to
            // $HOME/.local/share/Trash/files. (Per the freedesktop.org
            // trash spec; explorenova writes here via the `trash` crate.)
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".local/share"))
                });
            let Some(base) = base else {
                app.status = "neither $XDG_DATA_HOME nor $HOME is set".into();
                return Ok(());
            };
            let trash = base.join("Trash/files");
            if !trash.exists() {
                app.status = format!("trash dir not found: {}", trash.display());
                return Ok(());
            }
            reroot(app, &trash, git_tx);
        }
        Action::SysClipCopy => {
            let targets = app.targets();
            if targets.is_empty() {
                app.status = "nothing to copy".into();
            } else {
                match explorenova::clipboard::copy_paths(&targets) {
                    Ok(n) => {
                        app.status = if n == 1 {
                            format!(
                                "copied to clipboard: {}",
                                targets[0].display()
                            )
                        } else {
                            format!("copied {n} paths to clipboard")
                        };
                    }
                    Err(e) => {
                        app.status = format!("clipboard: {e}");
                    }
                }
            }
        }
        Action::Trash => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to trash".into();
            } else {
                let mut ok = 0usize;
                let mut failed: Vec<String> = Vec::new();
                for p in &targets {
                    match explorenova::fs_ops::trash_path(p) {
                        Ok(_) => ok += 1,
                        Err(_) => failed.push(p.display().to_string()),
                    }
                }
                app.status = if failed.is_empty() {
                    if n == 1 {
                        format!("trashed {}", targets[0].display())
                    } else {
                        format!("trashed {n}")
                    }
                } else {
                    let preview =
                        failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    let tail = if failed.len() > 2 { ", …" } else { "" };
                    format!(
                        "trashed {}/{} ({} failed: {}{})",
                        ok,
                        n,
                        failed.len(),
                        preview,
                        tail
                    )
                };
                app.marks.clear();
                refresh_in_place(app, git_tx);
            }
        }
        Action::ToggleHidden => {
            // Arena::load_children caches children, so flipping
            // arena.show_hidden alone won't re-scan. Rebuild the arena
            // rooted at the CURRENT root path (NOT the startup `cwd`
            // arg — the user may have re-rooted via H/T/devices/ascend)
            // and re-reveal whichever path was under the cursor.
            let _ = cwd; // intentionally unused here; see comment above
            let cur_root = app.arena.nodes[app.arena.root].path.clone();
            let sel_path = app
                .visible
                .get(app.selected)
                .map(|&id| app.arena.nodes[id].path.clone());
            let new_hidden = !app.arena.show_hidden;
            if let Ok(mut na) = Arena::new(&cur_root) {
                na.show_hidden = new_hidden;
                na.nodes[na.root].children = None;
                let _ = na.load_children(na.root);
                na.nodes[na.root].expanded = true;
                app.arena = na;
                app.visible = app.arena.flatten();
                if let Some(p) = sel_path {
                    app.navigate_to(&p);
                }
            }
        }
        Action::Search => {
            app.begin_search();
            app.enter_mode(Mode::Search, "");
        }
        Action::SearchNext => app.search_cycle(true),
        Action::SearchPrev => app.search_cycle(false),
        Action::PathInput => {
            app.pending = Some(Action::PathInput);
            app.enter_mode(Mode::Input, "");
        }
        Action::Create => {
            app.pending = Some(Action::Create);
            app.enter_mode(Mode::Input, "");
        }
        Action::Rename => {
            let seed = app
                .visible
                .get(app.selected)
                .map(|&id| {
                    app.arena.nodes[id]
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            app.pending = Some(Action::Rename);
            app.enter_mode(Mode::Input, &seed);
        }
        Action::PermanentDelete => {
            let n = app.targets().len();
            if n == 0 {
                app.status = "nothing to delete".into();
            } else {
                app.pending = Some(Action::PermanentDelete);
                app.mode = Mode::Confirm;
                app.status = if n == 1 {
                    let name = app
                        .targets()
                        .into_iter()
                        .next()
                        .and_then(|p| {
                            p.file_name().map(|s| s.to_string_lossy().to_string())
                        })
                        .unwrap_or_default();
                    format!("permanently delete {name}? y/N")
                } else {
                    format!("permanently delete {n}? y/N")
                };
            }
        }
        Action::Copy => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to copy".into();
            } else {
                app.clipboard = Some((targets, false));
                app.status = format!("copied {n}");
            }
        }
        Action::Cut => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.status = "nothing to cut".into();
            } else {
                app.clipboard = Some((targets, true));
                app.status = format!("cut {n}");
            }
        }
        Action::Paste => {
            if app.op.is_some() {
                app.status = "another op in progress".into();
                return Ok(());
            }
            let snapshot = app.clipboard.clone();
            let Some((srcs, is_cut)) = snapshot else {
                app.status = "nothing to paste".into();
                return Ok(());
            };
            let dst_dir = app
                .visible
                .get(app.selected)
                .map(|&id| {
                    let n = &app.arena.nodes[id];
                    if n.is_dir() {
                        n.path.clone()
                    } else {
                        n.path.parent().unwrap_or(cwd).to_path_buf()
                    }
                })
                .unwrap_or_else(|| cwd.to_path_buf());
            let mode = if is_cut {
                explorenova::fs_ops::PasteMode::Move
            } else {
                explorenova::fs_ops::PasteMode::Copy
            };
            // Cut clears clipboard + marks immediately (matches the
            // pre-async behaviour where moved sources became unreachable
            // post-paste). Capture src list before moving into worker.
            if is_cut {
                app.clipboard = None;
                for p in &srcs {
                    app.marks.remove(p);
                }
            }
            app.op = Some(explorenova::ops::spawn_paste(srcs, dst_dir, mode));
            app.status = "starting…".into();
        }
        Action::OpenWith => {
            if let Some(&id) = app.visible.get(app.selected) {
                let path = app.arena.nodes[id].path.clone();
                let items = explorenova::apps::apps_for(&path);
                app.mode = Mode::Input;
                app.pending = Some(Action::OpenWith);
                app.status.clear();
                app.dialog = explorenova::app::Dialog::AppPick {
                    items,
                    sel: 0,
                    filter: String::new(),
                };
            }
        }
        Action::ShellOnce => {
            app.pending = Some(Action::ShellOnce);
            app.enter_mode(Mode::Input, "");
        }
        Action::Toggle => {
            // Bookmarks pane focused: Enter opens the highlighted entry.
            // If the path is under the current arena root we reveal it
            // in-place; otherwise we re-root at the bookmark (matches
            // the device-mount / OpenTrash behaviour). Files reveal to
            // the parent dir and select the file row.
            if app.focus == explorenova::app::Pane::Bookmarks {
                let target = app
                    .state
                    .bookmarks
                    .get(app.bookmark_sel)
                    .cloned();
                if let Some(p) = target {
                    if !app.navigate_to(&p) {
                        // path outside current root → reroot. If target
                        // is a file, reroot at its parent then reveal.
                        let root_for = if p.is_file() {
                            p.parent()
                                .map(|pp| pp.to_path_buf())
                                .unwrap_or_else(|| p.clone())
                        } else {
                            p.clone()
                        };
                        reroot(app, &root_for, git_tx);
                        if p.is_file() {
                            app.navigate_to(&p);
                        }
                    }
                    app.focus = explorenova::app::Pane::Tree;
                }
                return Ok(());
            }
            // Enter: directories re-root the tree at that folder (cd-in
            // semantics); archives extract to a sibling dir and reveal;
            // other files open in the default application. Use h/l (or
            // arrow keys) to expand/collapse without re-rooting.
            if let Some(&id) = app.visible.get(app.selected) {
                if app.arena.nodes[id].is_dir() {
                    let p = app.arena.nodes[id].path.clone();
                    reroot(app, &p, git_tx);
                } else {
                    let p = app.arena.nodes[id].path.clone();
                    if explorenova::fs_ops::is_archive(&p) {
                        if app.op.is_some() {
                            app.status = "another op in progress".into();
                        } else if let Some(state) =
                            explorenova::ops::spawn_extract(p.clone())
                        {
                            app.op = Some(state);
                            app.status = "starting…".into();
                        }
                    } else if let Err(e) = open::that_detached(&p) {
                        app.status = format!("open failed: {e}");
                    }
                }
            }
        }
        Action::Parent => ascend(app),
        Action::ToggleMark => app.toggle_mark(),
        Action::SelectAllVisible => app.select_all_visible(),
        Action::ToggleVisualMode => app.toggle_visual_mode(),
        Action::Collapse => {
            if app.visible.get(app.selected).copied() == Some(app.arena.root) {
                ascend(app);
            } else {
                app.apply(Action::Collapse);
            }
        }
        Action::NewTab => match app.open_tab() {
            Ok(()) => {
                app.status = if app.is_split() {
                    "opened tab".into()
                } else {
                    "already split".into()
                };
                // New active arena needs a fresh git scan.
                let _ = git_tx.send(());
            }
            Err(e) => app.status = format!("open tab failed: {e}"),
        },
        Action::CloseTab => {
            if app.is_split() {
                app.close_tab();
                app.status = "closed tab".into();
                let _ = git_tx.send(());
            }
        }
        Action::FocusNext => {
            // Tab key in split mode + Tree focus swaps the two tabs;
            // otherwise falls back to the existing bookmarks↔tree cycle.
            if app.is_split() && app.focus == explorenova::app::Pane::Tree {
                app.swap_tab();
                let _ = git_tx.send(());
            } else {
                app.apply(Action::FocusNext);
            }
        }
        Action::SortMenu => app.mode = explorenova::keymap::Mode::SortMenu,
        Action::Help => {
            app.help_scroll = 0;
            app.help_filter.clear();
            app.mode = explorenova::keymap::Mode::Help;
        }
        Action::SortByName => {
            app.set_sort_mode(explorenova::sort::SortMode::Name);
            app.apply_sort();
            app.mode = explorenova::keymap::Mode::Normal;
            app.status = "sort: name".into();
        }
        Action::SortBySize => {
            app.set_sort_mode(explorenova::sort::SortMode::Size);
            app.apply_sort();
            app.mode = explorenova::keymap::Mode::Normal;
            app.status = "sort: size".into();
        }
        Action::SortByMtime => {
            app.set_sort_mode(explorenova::sort::SortMode::Mtime);
            app.apply_sort();
            app.mode = explorenova::keymap::Mode::Normal;
            app.status = "sort: mtime".into();
        }
        Action::SortByExt => {
            app.set_sort_mode(explorenova::sort::SortMode::Ext);
            app.apply_sort();
            app.mode = explorenova::keymap::Mode::Normal;
            app.status = "sort: ext".into();
        }
        Action::SortToggleReverse => {
            app.toggle_sort_reverse();
            app.apply_sort();
            app.status = format!("reverse: {}", if app.sort.reverse { "on" } else { "off" });
        }
        Action::SortToggleDirFirst => {
            app.toggle_sort_dir_first();
            app.apply_sort();
            app.status = format!("dir first: {}", if app.sort.dir_first { "on" } else { "off" });
        }
        other => app.apply(other),
    }
    Ok(())
}
