use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
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
        .map(|s| State::from_str(&s))
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
                let sel_path = app
                    .visible
                    .get(app.selected)
                    .map(|&id| app.arena.nodes[id].path.clone());
                if let Ok(new_arena) = Arena::new(&start) {
                    let prev_hidden = app.arena.show_hidden;
                    app.arena = new_arena;
                    if prev_hidden {
                        app.arena.show_hidden = true;
                        app.arena.nodes[app.arena.root].children = None;
                        let _ = app.arena.load_children(app.arena.root);
                        app.arena.nodes[app.arena.root].expanded = true;
                    }
                    app.visible = app.arena.flatten();
                    if let Some(p) = sel_path {
                        if let Ok(Some(id)) = app.arena.reveal(&p) {
                            app.visible = app.arena.flatten();
                            if let Some(i) = app.visible.iter().position(|&n| n == id) {
                                app.selected = i;
                            }
                        }
                    }
                }
                let _ = git_tx.send(());
                fs_dirty_since = None;
            }
        }

        term.draw(|f| {
            ui::render_frame(f, &mut app, &mut preview_cache, &mut picker, &mut img_cache)
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
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
                Mode::Search | Mode::Input
                    if matches!(
                        app.dialog,
                        explorenova::app::Dialog::AppPick { .. }
                    ) =>
                {
                    use explorenova::app::Dialog;
                    let (len, sel) = match &app.dialog {
                        Dialog::AppPick { items, sel } => (items.len() + 1, *sel),
                        _ => (1, 0),
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
                                    items.get(sel).cloned()
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
                        app.mode = Mode::Normal;
                        app.input.clear();
                        app.pending = None;
                    }
                    KeyCode::Enter => match app.mode {
                        Mode::Search => {
                            app.run_search();
                            app.mode = Mode::Normal;
                        }
                        Mode::Input => match app.pending.take() {
                            Some(Action::Rename) => app.commit_rename(),
                            Some(Action::Create) => app.commit_create(),
                            Some(Action::PathInput) => {
                                let target = explorenova::nav::expand(&app.input.value());
                                match app.arena.reveal(std::path::Path::new(&target)) {
                                    Ok(Some(id)) => {
                                        app.visible = app.arena.flatten();
                                        if let Some(i) =
                                            app.visible.iter().position(|&n| n == id)
                                        {
                                            app.selected = i;
                                        }
                                    }
                                    _ => app.status = "path not found".into(),
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
                    KeyCode::Delete => app.input.delete(),
                    KeyCode::Backspace => app.input.backspace(),
                    // Some terminals send Backspace as DEL (0x7f) or Ctrl-H (0x08).
                    KeyCode::Char('\u{7f}') | KeyCode::Char('\u{8}') => {
                        app.input.backspace()
                    }
                    KeyCode::Char(ch) if !ch.is_control() => app.input.insert(ch),
                    _ => {}
                },
                Mode::Confirm => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_yes(),
                    _ => {
                        app.pending = None;
                        app.mode = Mode::Normal;
                        app.status.clear();
                    }
                },
            }
        }

        app.dialog = match app.mode {
            Mode::Normal => explorenova::app::Dialog::None,
            Mode::Confirm => explorenova::app::Dialog::Confirm,
            Mode::Search | Mode::Input => match std::mem::take(&mut app.dialog) {
                explorenova::app::Dialog::AppPick { items, sel } => {
                    explorenova::app::Dialog::AppPick { items, sel }
                }
                _ => explorenova::app::Dialog::Input,
            },
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
            if let Ok(Some(id)) = app.arena.reveal(&root_path) {
                app.visible = app.arena.flatten();
                if let Some(i) = app.visible.iter().position(|&n| n == id) {
                    app.selected = i;
                }
            }
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
            shell::drop_to_shell(cwd)?;
        }
        Action::Refresh => {
            let _ = git_tx.send(());
        }
        Action::Trash => {
            if let Some(&id) = app.visible.get(app.selected) {
                let p = app.arena.nodes[id].path.clone();
                if let Err(e) = explorenova::fs_ops::trash_path(&p) {
                    app.status = format!("trash failed: {e}");
                } else {
                    app.status = format!("trashed {}", p.display());
                }
            }
        }
        Action::ToggleHidden => {
            // CARRY-FORWARD FIX (Task 6 review): Arena::load_children caches
            // children, so flipping arena.show_hidden alone will not re-scan.
            // Rebuild the arena with the new setting, preserving the selected
            // path and root expansion.
            let sel_path = app
                .visible
                .get(app.selected)
                .map(|&id| app.arena.nodes[id].path.clone());
            let new_hidden = !app.arena.show_hidden;
            if let Ok(mut na) = Arena::new(cwd) {
                na.show_hidden = new_hidden;
                na.nodes[na.root].children = None;
                let _ = na.load_children(na.root);
                na.nodes[na.root].expanded = true;
                app.arena = na;
                app.visible = app.arena.flatten();
                if let Some(p) = sel_path {
                    if let Ok(Some(id)) = app.arena.reveal(&p) {
                        app.visible = app.arena.flatten();
                        if let Some(i) = app.visible.iter().position(|&n| n == id) {
                            app.selected = i;
                        }
                    }
                }
            }
        }
        Action::Search => app.enter_mode(Mode::Search, ""),
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
            app.pending = Some(Action::PermanentDelete);
            app.mode = Mode::Confirm;
            app.status = "permanently delete? y/N".into();
        }
        Action::Copy => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), false));
                app.status = "copied".into();
            }
        }
        Action::Cut => {
            if let Some(&id) = app.visible.get(app.selected) {
                app.clipboard = Some((app.arena.nodes[id].path.clone(), true));
                app.status = "cut".into();
            }
        }
        Action::Paste => {
            if let Some((src, is_cut)) = app.clipboard.clone() {
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
                match explorenova::fs_ops::paste(&src, &dst_dir, mode) {
                    Ok(p) => app.status = format!("pasted -> {}", p.display()),
                    Err(e) => app.status = format!("paste failed: {e}"),
                }
                if is_cut {
                    app.clipboard = None;
                }
            }
        }
        Action::OpenWith => {
            if let Some(&id) = app.visible.get(app.selected) {
                let path = app.arena.nodes[id].path.clone();
                let items = explorenova::apps::apps_for(&path);
                app.mode = Mode::Input;
                app.pending = Some(Action::OpenWith);
                app.status.clear();
                app.dialog = explorenova::app::Dialog::AppPick { items, sel: 0 };
            }
        }
        Action::ShellOnce => {
            app.pending = Some(Action::ShellOnce);
            app.enter_mode(Mode::Input, "");
        }
        Action::Toggle => {
            // Enter: directories toggle expand/collapse (pure reducer);
            // files open in the default application.
            if let Some(&id) = app.visible.get(app.selected) {
                if app.arena.nodes[id].is_dir() {
                    app.apply(Action::Toggle);
                } else {
                    let p = app.arena.nodes[id].path.clone();
                    if let Err(e) = open::that_detached(&p) {
                        app.status = format!("open failed: {e}");
                    }
                }
            }
        }
        Action::Parent => ascend(app),
        Action::Collapse => {
            if app.visible.get(app.selected).copied() == Some(app.arena.root) {
                ascend(app);
            } else {
                app.apply(Action::Collapse);
            }
        }
        other => app.apply(other),
    }
    Ok(())
}
