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
use std::time::Duration;

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
    let mut state = std::fs::read_to_string(&sp)
        .map(|s| State::from_str(&s))
        .unwrap_or_default();

    let mut config = load_config();
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
    app.state = state.clone();
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

    let mut picker = ui::make_picker();
    let _guard = TermGuard::new()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut chord = String::new();
    let mut img_cache = ui::ImageCache::default();

    loop {
        while let Ok(map) = git_res_rx.try_recv() {
            for n in app.arena.nodes.iter_mut() {
                if let Some(s) = map.get(&n.path) {
                    n.git = *s;
                }
            }
        }

        term.draw(|f| {
            ui::render_frame(f, &mut app, &mut preview_cache, &mut picker, &mut img_cache)
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            let Some(tok) = key_token(&k) else { continue };
            match app.mode {
                Mode::Normal => {
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
                _ => {
                    if keymap.resolve(app.mode, &tok) == Some(Action::Cancel) {
                        app.mode = Mode::Normal;
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    app.state.last_cwd = Some(start.clone());
    app.state.bookmarks = app.state.bookmarks.clone();
    if let Some(parent) = sp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&sp, app.state.to_toml());
    Ok(())
}

/// Bridge actions that need IO to their modules; pure actions go through App::apply.
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
        other => app.apply(other),
    }
    Ok(())
}
