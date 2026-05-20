use crate::apps::AppEntry;
use crate::config::Config;
use crate::keymap::{Action, Mode};
use crate::state::State;
use crate::textinput::TextInput;
use crate::tree::Arena;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub enum Dialog {
    #[default]
    None,
    Input,
    Confirm,
    AppPick { items: Vec<AppEntry>, sel: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Bookmarks,
    Tree,
    Preview,
}

pub struct App {
    pub arena: Arena,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub mode: Mode,
    pub focus: Pane,
    pub config: Config,
    pub state: State,
    pub clipboard: Option<(PathBuf, bool)>, // (path, is_cut)
    pub status: String,
    pub should_quit: bool,
    pub input: TextInput,
    pub search_results: Vec<(PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub pending: Option<Action>,
    pub dialog: Dialog,
}

impl App {
    pub fn new(arena: Arena, config: Config, state: State) -> Self {
        let visible = arena.flatten();
        App {
            arena,
            visible,
            selected: 0,
            mode: Mode::Normal,
            focus: Pane::Tree,
            config,
            state,
            clipboard: None,
            status: String::new(),
            should_quit: false,
            input: TextInput::new(),
            search_results: Vec::new(),
            search_idx: 0,
            pending: None,
            dialog: Dialog::None,
        }
    }

    fn refresh_visible(&mut self) {
        let cur = self.visible.get(self.selected).copied();
        self.visible = self.arena.flatten();
        if let Some(c) = cur {
            if let Some(i) = self.visible.iter().position(|&n| n == c) {
                self.selected = i;
            }
        }
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    fn sel_id(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    /// Apply a Normal-mode navigation/structural action. IO-bearing actions
    /// (open, shell, paste) are handled in main.rs; this covers pure state.
    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Down => {
                if self.selected + 1 < self.visible.len() {
                    self.selected += 1;
                }
            }
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Top => self.selected = 0,
            Action::Bottom => self.selected = self.visible.len().saturating_sub(1),
            Action::HalfDown => {
                self.selected = (self.selected + 10).min(self.visible.len().saturating_sub(1))
            }
            Action::HalfUp => self.selected = self.selected.saturating_sub(10),
            Action::Open => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() {
                        let _ = self.arena.expand(id);
                        self.refresh_visible();
                    }
                }
            }
            Action::Collapse => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() && self.arena.nodes[id].expanded {
                        self.arena.collapse(id);
                    } else if let Some(p) = self.arena.nodes[id].parent {
                        if let Some(i) = self.visible.iter().position(|&n| n == p) {
                            self.selected = i;
                        }
                    }
                    self.refresh_visible();
                }
            }
            Action::Toggle => {
                if let Some(id) = self.sel_id() {
                    if self.arena.nodes[id].is_dir() {
                        let _ = self.arena.toggle(id);
                        self.refresh_visible();
                    }
                }
            }
            Action::FocusNext => {
                self.focus = match self.focus {
                    Pane::Bookmarks => Pane::Tree,
                    Pane::Tree => Pane::Preview,
                    Pane::Preview => Pane::Bookmarks,
                };
            }
            Action::ToggleBookmarksPane => {
                self.config.panes.bookmarks.visible = !self.config.panes.bookmarks.visible;
            }
            Action::TogglePreviewPane => {
                self.config.panes.preview.visible = !self.config.panes.preview.visible;
            }
            Action::ToggleBookmark => {
                if let Some(id) = self.sel_id() {
                    self.state.toggle_bookmark(self.arena.nodes[id].path.clone());
                }
            }
            Action::Quit => self.should_quit = true,
            Action::Cancel => self.mode = Mode::Normal,
            Action::ResizeShrink => self.resize_focus(-1),
            Action::ResizeGrow => self.resize_focus(1),
            Action::ReorderLeft => self.reorder_focus(-1),
            Action::ReorderRight => self.reorder_focus(1),
            // ToggleHidden is handled in main::handle() (rebuilds arena so
            // cached children re-scan); intentionally a no-op in the pure reducer.
            _ => {}
        }
    }

    pub fn enter_mode(&mut self, m: Mode, seed: &str) {
        // `dialog` is derived from `mode` by the sync block in main's event
        // loop — keep this the single source of truth, don't set it here.
        self.mode = m;
        self.input.set(seed);
        self.status.clear();
    }

    pub fn push_input(&mut self, c: char) {
        self.input.insert(c);
    }

    pub fn backspace_input(&mut self) {
        self.input.backspace();
    }

    pub fn run_search(&mut self) {
        let root = self.arena.nodes[self.arena.root].path.clone();
        let cands = crate::search::collect(&root, self.arena.show_hidden);
        let ranked = crate::search::rank(&cands, &root, &self.input.value());
        self.search_results = ranked.into_iter().map(|(p, _, idx)| (p, idx)).collect();
        self.search_idx = 0;
        self.jump_to_search();
    }

    pub fn jump_to_search(&mut self) {
        if let Some((p, _)) = self.search_results.get(self.search_idx).cloned() {
            if let Ok(Some(id)) = self.arena.reveal(&p) {
                self.visible = self.arena.flatten();
                if let Some(i) = self.visible.iter().position(|&n| n == id) {
                    self.selected = i;
                }
            }
        }
    }

    pub fn search_cycle(&mut self, forward: bool) {
        if self.search_results.is_empty() {
            return;
        }
        let n = self.search_results.len();
        self.search_idx = if forward {
            (self.search_idx + 1) % n
        } else {
            (self.search_idx + n - 1) % n
        };
        self.jump_to_search();
    }

    pub fn commit_create(&mut self) {
        let base = self
            .sel_dir()
            .unwrap_or_else(|| self.arena.nodes[self.arena.root].path.clone());
        let target = base.join(self.input.value());
        match crate::fs_ops::create(&target) {
            Ok(_) => self.status = format!("created {}", target.display()),
            Err(e) => self.status = format!("create failed: {e}"),
        }
        self.mode = Mode::Normal;
    }

    pub fn commit_rename(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            let p = self.arena.nodes[id].path.clone();
            match crate::fs_ops::rename(&p, &self.input.value()) {
                Ok(np) => self.status = format!("renamed -> {}", np.display()),
                Err(e) => self.status = format!("rename failed: {e}"),
            }
        }
        self.mode = Mode::Normal;
    }

    fn sel_dir(&self) -> Option<PathBuf> {
        let id = *self.visible.get(self.selected)?;
        let n = &self.arena.nodes[id];
        Some(if n.is_dir() {
            n.path.clone()
        } else {
            n.path.parent()?.to_path_buf()
        })
    }

    pub fn confirm_yes(&mut self) {
        if let Some(Action::PermanentDelete) = self.pending {
            if let Some(&id) = self.visible.get(self.selected) {
                let p = self.arena.nodes[id].path.clone();
                match crate::fs_ops::permanent_delete(&p) {
                    Ok(_) => self.status = format!("deleted {}", p.display()),
                    Err(e) => {
                        self.status = format!("delete failed: {}: {e}", p.display())
                    }
                }
            }
        }
        self.pending = None;
        self.mode = Mode::Normal;
    }

    fn focus_name(&self) -> &'static str {
        match self.focus {
            Pane::Bookmarks => "bookmarks",
            Pane::Tree => "tree",
            Pane::Preview => "preview",
        }
    }

    fn focus_cfg(&mut self) -> &mut crate::config::PaneConfig {
        match self.focus {
            Pane::Bookmarks => &mut self.config.panes.bookmarks,
            Pane::Tree => &mut self.config.panes.tree,
            Pane::Preview => &mut self.config.panes.preview,
        }
    }

    fn resize_focus(&mut self, dir: i32) {
        let pc = self.focus_cfg();
        match pc.width {
            Some(w) => {
                let nw = if dir < 0 {
                    w.saturating_sub(2)
                } else {
                    w.saturating_add(2)
                };
                pc.width = Some(nw.clamp(4, 200));
            }
            None => {
                let nw = if dir < 0 {
                    pc.weight.saturating_sub(1)
                } else {
                    pc.weight.saturating_add(1)
                };
                pc.weight = nw.clamp(1, 100);
            }
        }
    }

    fn reorder_focus(&mut self, dir: i32) {
        let name = self.focus_name();
        let order = &mut self.config.panes.order;
        if let Some(i) = order.iter().position(|n| n == name) {
            let j = i as i32 + dir;
            if j >= 0 && (j as usize) < order.len() {
                order.swap(i, j as usize);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn app() -> App {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/x.txt"), "1").unwrap();
        std::fs::write(d.path().join("a.txt"), "1").unwrap();
        std::mem::forget(d); // keep dir alive for the test
        let arena = Arena::new(std::path::Path::new(".")).unwrap_or_else(|_| {
            Arena::new(std::env::temp_dir().as_path()).unwrap()
        });
        App::new(arena, Config::with_defaults(), State::default())
    }

    #[test]
    fn down_then_up_returns_to_zero() {
        let mut a = app();
        a.apply(Action::Down);
        a.apply(Action::Up);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn quit_sets_flag() {
        let mut a = app();
        a.apply(Action::Quit);
        assert!(a.should_quit);
    }

    #[test]
    fn toggle_preview_pane_flips_visibility() {
        let mut a = app();
        let before = a.config.panes.preview.visible;
        a.apply(Action::TogglePreviewPane);
        assert_ne!(a.config.panes.preview.visible, before);
    }

    #[test]
    fn focus_cycles() {
        let mut a = app();
        assert_eq!(a.focus, Pane::Tree);
        a.apply(Action::FocusNext);
        assert_eq!(a.focus, Pane::Preview);
    }

    #[test]
    fn permanent_delete_requires_confirm() {
        let mut a = app();
        a.pending = Some(Action::PermanentDelete);
        a.mode = Mode::Confirm;
        a.apply(Action::Cancel);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.status.is_empty(), "cancel must not perform deletion");
    }

    #[test]
    fn enter_mode_seeds_input() {
        let mut a = app();
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input.value(), "seed");
        assert_eq!(a.mode, Mode::Input);
    }

    #[test]
    fn resize_focus_changes_fixed_pane_width() {
        let mut a = app();
        a.focus = Pane::Bookmarks; // default width Some(24)
        a.apply(Action::ResizeGrow);
        assert_eq!(a.config.panes.bookmarks.width, Some(26));
        a.apply(Action::ResizeShrink);
        assert_eq!(a.config.panes.bookmarks.width, Some(24));
    }

    #[test]
    fn reorder_focus_moves_pane_left() {
        let mut a = app();
        a.focus = Pane::Tree; // default order ["bookmarks","tree","preview"], tree idx 1
        let before = a.config.panes.order.clone();
        a.apply(Action::ReorderLeft);
        assert_ne!(a.config.panes.order, before);
        assert_eq!(a.config.panes.order[0], "tree");
    }
}
