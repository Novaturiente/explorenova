use crate::config::Config;
use crate::keymap::{Action, Mode};
use crate::state::State;
use crate::tree::Arena;
use std::path::PathBuf;

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
            // ToggleHidden is handled in main::handle() (rebuilds arena so
            // cached children re-scan); intentionally a no-op in the pure reducer.
            _ => {}
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
}
