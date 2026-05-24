//! Action and key-event dispatch.
//!
//! Splits responsibility cleanly:
//! - [`handle_key`] interprets a [`KeyEvent`] against the current mode
//!   (Normal chord build, Search/Input editing, Confirm prompt, Sort
//!   menu, Help overlay) and dispatches to [`handle`] when the mode
//!   produces an action.
//! - [`handle`] is the [`Action`] match — pure UI actions delegate to
//!   [`App::apply`]; IO-bearing actions (shell drop, paste/extract
//!   workers, reroot) live here so `App` stays free of channels and
//!   process spawns.
//!
//! All filesystem mutations re-flatten the tree via [`refresh_in_place`]
//! or [`App::navigate_to`] so the visible list and cursor never drift.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;
use std::sync::mpsc;

use crate::app::{App, Dialog, Pane};
use crate::apps::AppEntry;
use crate::keymap::{key_token, Action, Keymap, Mode};
use crate::tree::Arena;

/// Case-insensitive substring filter. Returns indices into `items`
/// whose `.name` contains every space-separated token in `filter`.
pub fn filter_app_indices(items: &[AppEntry], filter: &str) -> Vec<usize> {
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

/// Re-root the arena at `path`, preserving hidden-files toggle, resetting
/// selection, and kicking a git rescan. Used by JumpHome, OpenTrash, and
/// the devices-modal "navigate outside current root" fallback.
pub fn reroot(app: &mut App, path: &Path, git_tx: &mpsc::Sender<()>) {
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
            app.set_status(format!("rooted at {}", path.display()));
            let _ = git_tx.send(());
        }
        Err(e) => {
            app.set_status(format!("cannot open {}: {e}", path.display()));
        }
    }
}

/// Rebuild the arena at the current root, preserving the show_hidden
/// toggle, ALL previously-expanded directories, and the cursor path.
/// Used after destructive batch ops (trash, permanent delete, cut-paste)
/// to drop stale entries immediately instead of waiting on the fs-watch
/// debounce — which can miss `trash` crate moves entirely when they go
/// through D-Bus.
pub fn refresh_in_place(app: &mut App, git_tx: &mpsc::Sender<()>) {
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

/// Re-root to the parent of the current arena root, leaving the cursor
/// on the directory we just ascended out of. No-op at filesystem root.
pub fn ascend(app: &mut App) {
    let root_path = app.arena.nodes[app.arena.root].path.clone();
    let parent = match root_path.parent() {
        Some(p) if p != root_path => p.to_path_buf(),
        _ => {
            app.set_status("at filesystem root");
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
            app.set_status(format!("cannot open {}", parent.display()));
        }
    }
}

/// Dispatch a single key event against the current `app.mode`.
///
/// Normal mode accumulates `chord` until [`Keymap::resolve`] either
/// produces an [`Action`] or rejects the prefix. Other modes (Search,
/// Input, Confirm, SortMenu, Help) consume `KeyCode` directly so they
/// can see Backspace/Delete/printable chars that `key_token` skips.
pub fn handle_key(
    app: &mut App,
    k: &KeyEvent,
    keymap: &Keymap,
    chord: &mut String,
    start: &Path,
    git_tx: &mpsc::Sender<()>,
) -> Result<()> {
    match app.mode {
        Mode::Normal => {
            let Some(tok) = key_token(k) else { return Ok(()) };
            chord.push_str(&tok);
            if let Some(action) = keymap.resolve(Mode::Normal, chord) {
                chord.clear();
                handle(app, action, start, git_tx)?;
            } else if keymap.is_prefix(chord) {
                // wait for more keys
            } else {
                chord.clear();
            }
        }
        Mode::SortMenu => {
            let Some(tok) = key_token(k) else { return Ok(()) };
            if let Some(action) = keymap.resolve(Mode::SortMenu, &tok) {
                if action == Action::Cancel {
                    app.mode = Mode::Normal;
                } else {
                    handle(app, action, start, git_tx)?;
                }
            }
        }
        Mode::Help => handle_help_key(app, k),
        Mode::Search | Mode::Input
            if matches!(app.dialog, Dialog::Devices { .. }) =>
        {
            handle_devices_key(app, k);
        }
        Mode::Search | Mode::Input
            if matches!(app.dialog, Dialog::AppPick { .. }) =>
        {
            handle_appick_key(app, k);
        }
        Mode::Search | Mode::Input => handle_text_input_key(app, k, start, git_tx),
        Mode::Confirm => handle_confirm_key(app, k, git_tx),
    }
    Ok(())
}

fn handle_help_key(app: &mut App, k: &KeyEvent) {
    // Arrow keys + Ctrl-d/u scroll; Backspace trims filter; Esc closes
    // (or clears filter first when non-empty); printable chars append
    // to filter.
    let scroll_step = |app: &mut App, by: i32| {
        if by < 0 {
            app.help_scroll = app.help_scroll.saturating_sub((-by) as usize);
        } else {
            app.help_scroll = app.help_scroll.saturating_add(by as usize);
        }
    };
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match (k.code, ctrl) {
        (KeyCode::Down, _) => scroll_step(app, 1),
        (KeyCode::Up, _) => scroll_step(app, -1),
        (KeyCode::PageDown, _) => scroll_step(app, 8),
        (KeyCode::PageUp, _) => scroll_step(app, -8),
        (KeyCode::Char('d'), true) => scroll_step(app, 8),
        (KeyCode::Char('u'), true) => scroll_step(app, -8),
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
            // Allow any printable char for the filter. Space,
            // punctuation, digits all useful for multi-token search
            // (e.g. "ctrl t").
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

fn handle_devices_key(app: &mut App, k: &KeyEvent) {
    let len = match &app.dialog {
        Dialog::Devices { items, .. } => items.len(),
        _ => 0,
    };
    match k.code {
        KeyCode::Esc | KeyCode::Char('M') => {
            app.mode = Mode::Normal;
            app.dialog = Dialog::None;
        }
        KeyCode::Down | KeyCode::Char('j') => app.dialog.nav(len, true),
        KeyCode::Up | KeyCode::Char('k') => app.dialog.nav(len, false),
        KeyCode::Char('r') => refresh_devices(app),
        KeyCode::Char('u') => devices_unmount_selected(app),
        KeyCode::Enter => devices_enter_selected(app),
        _ => {}
    }
}

fn refresh_devices(app: &mut App) {
    match crate::devices::list() {
        Ok(items) => {
            let new_len = items.len();
            if let Dialog::Devices { items: it, sel } = &mut app.dialog {
                *it = items;
                if *sel >= new_len {
                    *sel = new_len.saturating_sub(1);
                }
            }
        }
        Err(e) => app.set_status(format!("devices: {e}")),
    }
}

fn devices_unmount_selected(app: &mut App) {
    let dev = match &app.dialog {
        Dialog::Devices { items, sel } => items.get(*sel).cloned(),
        _ => None,
    };
    let Some(d) = dev else { return };
    if !d.is_mounted() {
        app.set_status("not mounted");
        return;
    }
    match crate::devices::unmount(&d.device) {
        Ok(_) => app.set_status(format!("unmounted {}", d.device)),
        Err(e) => app.set_status(format!("umount failed: {e}")),
    }
    refresh_devices(app);
}

fn devices_enter_selected(app: &mut App) {
    let dev = match &app.dialog {
        Dialog::Devices { items, sel } => items.get(*sel).cloned(),
        _ => None,
    };
    let Some(d) = dev else { return };
    if let Some(mp) = d.mountpoint.clone() {
        // mounted → cd then close. Try reveal within current arena
        // first; if the mountpoint is outside (typical: /run/media/...)
        // re-root the arena at the mountpoint.
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
                    app.set_status(format!("rooted at {}", mp.display()));
                }
                Err(e) => {
                    app.set_status(format!("cannot open {}: {e}", mp.display()));
                }
            }
        }
        app.mode = Mode::Normal;
        app.dialog = Dialog::None;
    } else {
        // unmounted → mount + refresh, stay
        match crate::devices::mount(&d.device) {
            Ok(_) => app.set_status(format!("mounted {}", d.device)),
            Err(e) => app.set_status(format!("mount failed: {e}")),
        }
        refresh_devices(app);
    }
}

fn handle_appick_key(app: &mut App, k: &KeyEvent) {
    // `len` = filtered apps + 1 (custom-command row). `sel` is the
    // index into that filtered+custom view.
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
        KeyCode::Up => app.dialog.nav(len, false),
        KeyCode::Down => app.dialog.nav(len, true),
        KeyCode::Backspace => {
            if let Dialog::AppPick { sel, filter, .. } = &mut app.dialog {
                filter.pop();
                *sel = 0;
            }
        }
        KeyCode::Char(c) => {
            if let Dialog::AppPick { sel, filter, .. } = &mut app.dialog {
                filter.push(c);
                *sel = 0;
            }
        }
        KeyCode::Enter => appick_enter(app, sel, len, &filtered_idx),
        _ => {}
    }
}

fn appick_enter(app: &mut App, sel: usize, len: usize, filtered_idx: &[usize]) {
    if sel == len - 1 {
        // "Custom command…" → typed-command path
        app.input.clear();
        app.pending = Some(Action::OpenWith);
        app.mode = Mode::Input;
        app.dialog = Dialog::Input;
        return;
    }
    let entry = if let Dialog::AppPick { items, .. } = &app.dialog {
        filtered_idx
            .get(sel)
            .and_then(|&i| items.get(i).cloned())
    } else {
        None
    };
    if let (Some(entry), Some(&id)) = (entry, app.visible.get(app.selected)) {
        let path = app.arena.nodes[id].path.clone();
        if let Err(e) = crate::apps::spawn(&entry, &path) {
            app.set_status(format!("open failed: {e}"));
        }
    }
    app.mode = Mode::Normal;
    app.pending = None;
    app.dialog = Dialog::None;
}

fn handle_text_input_key(
    app: &mut App,
    k: &KeyEvent,
    start: &Path,
    git_tx: &mpsc::Sender<()>,
) {
    match k.code {
        KeyCode::Esc => {
            if app.mode == Mode::Search {
                app.end_search();
            }
            app.mode = Mode::Normal;
            app.input.clear();
            app.pending = None;
        }
        KeyCode::Enter => text_input_enter(app, start, git_tx),
        KeyCode::Tab => {
            if app.mode == Mode::Input && app.pending == Some(Action::PathInput) {
                let c = crate::nav::completions(&app.input.value());
                if let Some(lcp) = crate::nav::longest_common(&app.input.value(), &c) {
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
        KeyCode::Backspace
        // Some terminals send Backspace as DEL (0x7f) or Ctrl-H (0x08).
        | KeyCode::Char('\u{7f}')
        | KeyCode::Char('\u{8}') => {
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
    }
}

fn text_input_enter(app: &mut App, start: &Path, git_tx: &mpsc::Sender<()>) {
    match app.mode {
        Mode::Search => {
            // Cursor is already on the top live match; Enter just
            // dismisses the prompt. Cached candidates can be freed now.
            app.end_search();
            app.mode = Mode::Normal;
        }
        Mode::Input => match app.pending.take() {
            Some(Action::Rename) => {
                let np = app.commit_rename();
                refresh_in_place(app, git_tx);
                if let Some(p) = np {
                    app.navigate_to(&p);
                }
            }
            Some(Action::Create) => {
                let np = app.commit_create();
                refresh_in_place(app, git_tx);
                if let Some(p) = np {
                    app.navigate_to(&p);
                }
            }
            Some(Action::PathInput) => {
                let target = crate::nav::expand(&app.input.value());
                if !app.navigate_to(Path::new(&target)) {
                    app.set_status("path not found");
                }
                app.mode = Mode::Normal;
            }
            Some(Action::OpenWith) => {
                if let Some(&id) = app.visible.get(app.selected) {
                    let p = app.arena.nodes[id].path.clone();
                    if let Err(e) = std::process::Command::new(app.input.value())
                        .arg(&p)
                        .spawn()
                    {
                        app.set_status(format!("open failed: {e}"));
                    }
                }
                app.mode = Mode::Normal;
            }
            Some(Action::ShellOnce) => {
                let cmd = app.input.value();
                app.mode = Mode::Normal;
                if let Err(e) = crate::shell::run_once(start, &cmd) {
                    app.set_status(format!("shell failed: {e}"));
                }
            }
            _ => app.mode = Mode::Normal,
        },
        _ => {}
    }
}

fn handle_confirm_key(app: &mut App, k: &KeyEvent, git_tx: &mpsc::Sender<()>) {
    match k.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.confirm_yes();
            // confirm_yes lives in app.rs and can't see git_tx —
            // refresh from here so the freshly unlinked entries drop
            // out of the tree at once.
            refresh_in_place(app, git_tx);
        }
        _ => {
            app.pending = None;
            app.mode = Mode::Normal;
            app.status.clear();
        }
    }
}

/// Dispatch a resolved [`Action`]. Pure UI actions defer to
/// [`App::apply`]; IO-bearing actions (shell drop, paste/extract
/// workers, reroot, devices listing) handle themselves here so `App`
/// stays free of channels and process spawns.
pub fn handle(
    app: &mut App,
    action: Action,
    cwd: &Path,
    git_tx: &mpsc::Sender<()>,
) -> Result<()> {
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
            if let Err(e) = crate::shell::drop_to_shell(cwd) {
                app.set_status(format!("shell: {e}"));
            }
        }
        Action::ShowDevices => match crate::devices::list() {
            Ok(items) => {
                app.dialog = Dialog::Devices { items, sel: 0 };
                app.mode = Mode::Input;
            }
            Err(e) => app.set_status(format!("devices: {e}")),
        },
        Action::Refresh => {
            let _ = git_tx.send(());
        }
        Action::JumpHome => {
            let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
            else {
                app.set_status("$HOME not set");
                return Ok(());
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
                app.set_status("neither $XDG_DATA_HOME nor $HOME is set");
                return Ok(());
            };
            let trash = base.join("Trash/files");
            if !trash.exists() {
                app.set_status(format!("trash dir not found: {}", trash.display()));
                return Ok(());
            }
            reroot(app, &trash, git_tx);
        }
        Action::SysClipCopy => sysclip_copy(app),
        Action::Trash => trash_targets(app, git_tx),
        Action::ToggleHidden => toggle_hidden(app, cwd),
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
        Action::PermanentDelete => prompt_permanent_delete(app),
        Action::Copy => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.set_status("nothing to copy");
            } else {
                app.clipboard = Some((targets, false));
                app.set_status(format!("copied {n}"));
            }
        }
        Action::Cut => {
            let targets = app.targets();
            let n = targets.len();
            if n == 0 {
                app.set_status("nothing to cut");
            } else {
                app.clipboard = Some((targets, true));
                app.set_status(format!("cut {n}"));
            }
        }
        Action::Paste => paste(app, cwd),
        Action::OpenWith => {
            if let Some(&id) = app.visible.get(app.selected) {
                let path = app.arena.nodes[id].path.clone();
                let items = crate::apps::apps_for(&path);
                app.mode = Mode::Input;
                app.pending = Some(Action::OpenWith);
                app.status.clear();
                app.dialog = Dialog::AppPick {
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
        Action::Toggle => toggle_action(app, git_tx),
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
                app.set_status(if app.is_split() { "opened tab" } else { "already split" });
                // New active arena needs a fresh git scan.
                let _ = git_tx.send(());
            }
            Err(e) => app.set_status(format!("open tab failed: {e}")),
        },
        Action::CloseTab => {
            if app.is_split() {
                app.close_tab();
                app.set_status("closed tab");
                let _ = git_tx.send(());
            }
        }
        Action::FocusNext => {
            // Tab key in split mode + Tree focus swaps the two tabs;
            // otherwise falls back to the existing bookmarks↔tree cycle.
            if app.is_split() && app.focus == Pane::Tree {
                app.swap_tab();
                let _ = git_tx.send(());
            } else {
                app.apply(Action::FocusNext);
            }
        }
        Action::SortMenu => app.mode = Mode::SortMenu,
        Action::Help => {
            app.help_scroll = 0;
            app.help_filter.clear();
            app.mode = Mode::Help;
        }
        Action::SortByName => set_sort(app, crate::sort::SortMode::Name, "sort: name"),
        Action::SortBySize => set_sort(app, crate::sort::SortMode::Size, "sort: size"),
        Action::SortByMtime => set_sort(app, crate::sort::SortMode::Mtime, "sort: mtime"),
        Action::SortByExt => set_sort(app, crate::sort::SortMode::Ext, "sort: ext"),
        Action::SortToggleReverse => {
            app.toggle_sort_reverse();
            app.apply_sort();
            app.set_status(format!(
                "reverse: {}",
                if app.sort.reverse { "on" } else { "off" }
            ));
        }
        Action::SortToggleDirFirst => {
            app.toggle_sort_dir_first();
            app.apply_sort();
            app.set_status(format!(
                "dir first: {}",
                if app.sort.dir_first { "on" } else { "off" }
            ));
        }
        other => app.apply(other),
    }
    Ok(())
}

fn set_sort(app: &mut App, mode: crate::sort::SortMode, msg: &str) {
    app.set_sort_mode(mode);
    app.apply_sort();
    app.mode = Mode::Normal;
    app.set_status(msg);
}

fn sysclip_copy(app: &mut App) {
    let targets = app.targets();
    if targets.is_empty() {
        app.set_status("nothing to copy");
        return;
    }
    match crate::clipboard::copy_paths(&targets) {
        Ok(n) => {
            let msg = if n == 1 {
                format!("copied to clipboard: {}", targets[0].display())
            } else {
                format!("copied {n} paths to clipboard")
            };
            app.set_status(msg);
        }
        Err(e) => app.set_status(format!("clipboard: {e}")),
    }
}

fn trash_targets(app: &mut App, git_tx: &mpsc::Sender<()>) {
    let targets = app.targets();
    let n = targets.len();
    if n == 0 {
        app.set_status("nothing to trash");
        return;
    }
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for p in &targets {
        match crate::fs_ops::trash_path(p) {
            Ok(_) => ok += 1,
            Err(_) => failed.push(p.display().to_string()),
        }
    }
    let msg = if failed.is_empty() {
        if n == 1 {
            format!("trashed {}", targets[0].display())
        } else {
            format!("trashed {n}")
        }
    } else {
        let preview = failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
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
    app.set_status(msg);
    app.marks.clear();
    refresh_in_place(app, git_tx);
}

fn toggle_hidden(app: &mut App, _cwd: &Path) {
    // Arena::load_children caches children, so flipping arena.show_hidden
    // alone won't re-scan. Rebuild the arena rooted at the CURRENT root
    // path (NOT the startup `cwd` arg — the user may have re-rooted via
    // H/T/devices/ascend) and re-reveal whichever path was under the
    // cursor.
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

fn prompt_permanent_delete(app: &mut App) {
    let n = app.targets().len();
    if n == 0 {
        app.set_status("nothing to delete");
        return;
    }
    app.pending = Some(Action::PermanentDelete);
    app.mode = Mode::Confirm;
    let msg = if n == 1 {
        let name = app
            .targets()
            .into_iter()
            .next()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        format!("permanently delete {name}? y/N")
    } else {
        format!("permanently delete {n}? y/N")
    };
    app.set_status(msg);
}

fn paste(app: &mut App, cwd: &Path) {
    if app.op.is_some() {
        app.set_status("another op in progress");
        return;
    }
    let snapshot = app.clipboard.clone();
    let Some((srcs, is_cut)) = snapshot else {
        app.set_status("nothing to paste");
        return;
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
        crate::fs_ops::PasteMode::Move
    } else {
        crate::fs_ops::PasteMode::Copy
    };
    // Cut clears clipboard + marks immediately (matches the pre-async
    // behaviour where moved sources became unreachable post-paste).
    // Capture src list before moving into worker.
    if is_cut {
        app.clipboard = None;
        for p in &srcs {
            app.marks.remove(p);
        }
    }
    app.op = Some(crate::ops::spawn_paste(srcs, dst_dir, mode));
    app.set_status("starting…");
}

fn toggle_action(app: &mut App, git_tx: &mpsc::Sender<()>) {
    // Bookmarks pane focused: Enter opens the highlighted entry. If
    // the path is under the current arena root we reveal it in-place;
    // otherwise we re-root at the bookmark (matches the device-mount /
    // OpenTrash behaviour). Files reveal to the parent dir and select
    // the file row.
    if app.focus == Pane::Bookmarks {
        let target = app.state.bookmarks.get(app.bookmark_sel).cloned();
        if let Some(p) = target {
            if !app.navigate_to(&p) {
                // path outside current root → reroot. If target is a
                // file, reroot at its parent then reveal.
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
            app.focus = Pane::Tree;
        }
        return;
    }
    // Enter: directories re-root the tree at that folder (cd-in
    // semantics); archives extract to a sibling dir and reveal; other
    // files open in the default application. Use h/l (or arrow keys)
    // to expand/collapse without re-rooting.
    if let Some(&id) = app.visible.get(app.selected) {
        if app.arena.nodes[id].is_dir() {
            let p = app.arena.nodes[id].path.clone();
            reroot(app, &p, git_tx);
        } else {
            let p = app.arena.nodes[id].path.clone();
            if crate::fs_ops::is_archive(&p) {
                if app.op.is_some() {
                    app.set_status("another op in progress");
                } else if let Some(state) = crate::ops::spawn_extract(p.clone()) {
                    app.op = Some(state);
                    app.set_status("starting…");
                }
            } else if let Err(e) = open::that_detached(&p) {
                app.set_status(format!("open failed: {e}"));
            }
        }
    }
}
