use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Input,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    Open,
    Collapse,
    Parent,
    ToggleMark,
    SelectAllVisible,
    Toggle,
    OpenDefault,
    OpenWith,
    FocusNext,
    ToggleBookmarksPane,
    TogglePreviewPane,
    ResizeShrink,
    ResizeGrow,
    ReorderLeft,
    ReorderRight,
    Create,
    Copy,
    Cut,
    Paste,
    Rename,
    Trash,
    PermanentDelete,
    ToggleBookmark,
    Search,
    SearchNext,
    SearchPrev,
    PathInput,
    ShellOnce,
    ShellDrop,
    ToggleHidden,
    Refresh,
    Quit,
    Cancel,
}

impl Action {
    pub fn from_name(s: &str) -> Option<Action> {
        use Action::*;
        Some(match s {
            "down" => Down, "up" => Up, "top" => Top, "bottom" => Bottom,
            "half_down" => HalfDown, "half_up" => HalfUp,
            "open" => Open, "collapse" => Collapse, "toggle" => Toggle,
            "open_default" => OpenDefault, "open_with" => OpenWith,
            "focus_next" => FocusNext,
            "toggle_bookmarks_pane" => ToggleBookmarksPane,
            "toggle_preview_pane" => TogglePreviewPane,
            "resize_shrink" => ResizeShrink, "resize_grow" => ResizeGrow,
            "reorder_left" => ReorderLeft, "reorder_right" => ReorderRight,
            "create" => Create, "copy" => Copy, "cut" => Cut, "paste" => Paste,
            "rename" => Rename, "trash" => Trash, "permanent_delete" => PermanentDelete,
            "toggle_bookmark" => ToggleBookmark,
            "search" => Search, "search_next" => SearchNext, "search_prev" => SearchPrev,
            "path_input" => PathInput,
            "shell_once" => ShellOnce, "shell_drop" => ShellDrop,
            "toggle_hidden" => ToggleHidden, "refresh" => Refresh,
            "parent" => Parent,
            "toggle_mark" => ToggleMark,
            "select_all_visible" => SelectAllVisible,
            "quit" => Quit, "cancel" => Cancel,
            _ => return None,
        })
    }
}

/// Parse a chord like "j", "G", "ctrl-d", "enter", "esc", "gg".
fn parse_chord(s: &str) -> Option<(KeyModifiers, KeyCode)> {
    let mut mods = KeyModifiers::NONE;
    let parts: Vec<&str> = s.split('-').collect();
    let key = *parts.last()?;
    for m in &parts[..parts.len().saturating_sub(1)] {
        match *m {
            "ctrl" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }
    let code = match key {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "del" => KeyCode::Delete,
        k if k.chars().count() == 1 => KeyCode::Char(k.chars().next().unwrap()),
        _ => return None,
    };
    Some((mods, code))
}

pub struct Keymap {
    /// Normal-mode single/double-char chords -> Action
    normal: HashMap<String, Action>,
}

impl Keymap {
    pub fn build(user: &std::collections::BTreeMap<String, Vec<String>>) -> Self {
        let mut normal = HashMap::new();
        for (action, chords) in Self::defaults() {
            for c in chords {
                normal.insert(c.to_string(), action);
            }
        }
        // user overrides: action name -> chords (replace)
        for (name, chords) in user {
            if let Some(a) = Action::from_name(name) {
                normal.retain(|_, v| *v != a);
                for c in chords {
                    normal.insert(c.clone(), a);
                }
            }
        }
        Keymap { normal }
    }

    fn defaults() -> Vec<(Action, Vec<&'static str>)> {
        use Action::*;
        vec![
            (Down, vec!["j", "down"]), (Up, vec!["k", "up"]),
            (Top, vec!["gg"]), (Bottom, vec!["G"]),
            (HalfDown, vec!["ctrl-d"]), (HalfUp, vec!["ctrl-u"]),
            (Open, vec!["l", "right"]), (Collapse, vec!["h", "left"]),
            (Parent, vec!["-"]),
            (ToggleMark, vec![" "]),
            (SelectAllVisible, vec!["ctrl-a"]),
            (Toggle, vec!["enter"]),
            (OpenDefault, vec!["o"]), (OpenWith, vec!["O"]),
            (FocusNext, vec!["tab"]),
            (ToggleBookmarksPane, vec!["B"]), (TogglePreviewPane, vec!["P"]),
            (ResizeShrink, vec!["<"]), (ResizeGrow, vec![">"]),
            (ReorderLeft, vec!["["]), (ReorderRight, vec!["]"]),
            (Create, vec!["a"]), (Copy, vec!["yy"]), (Cut, vec!["dd"]),
            (Paste, vec!["p"]), (Rename, vec!["r"]),
            (Trash, vec!["x", "del"]), (PermanentDelete, vec!["X"]),
            (ToggleBookmark, vec!["m"]),
            (Search, vec!["/"]), (SearchNext, vec!["n"]), (SearchPrev, vec!["N"]),
            (PathInput, vec![":"]),
            (ShellOnce, vec!["!"]), (ShellDrop, vec!["S"]),
            (ToggleHidden, vec!["."]), (Refresh, vec!["R"]),
            (Quit, vec!["q"]), (Cancel, vec!["esc"]),
        ]
    }

    /// Resolve a pending chord buffer in the given mode. Returns the action and
    /// whether the buffer should be cleared.
    pub fn resolve(&self, mode: Mode, buffer: &str) -> Option<Action> {
        match mode {
            Mode::Normal => self.normal.get(buffer).copied(),
            Mode::Search | Mode::Input => match buffer {
                "esc" => Some(Action::Cancel),
                "enter" => Some(Action::Toggle),
                _ => None,
            },
            Mode::Confirm => None,
        }
    }

    /// Could `buffer` still grow into a binding (e.g. "g" -> "gg")?
    pub fn is_prefix(&self, buffer: &str) -> bool {
        self.normal.keys().any(|k| k.len() > buffer.len() && k.starts_with(buffer))
    }
}

/// Reduce a crossterm KeyEvent to a chord token.
pub fn key_token(ev: &KeyEvent) -> Option<String> {
    let mut prefix = String::new();
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        prefix.push_str("ctrl-");
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        prefix.push_str("alt-");
    }
    let base = match ev.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Delete => "del".into(),
        _ => return None,
    };
    Some(format!("{prefix}{base}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn default_j_is_down() {
        let km = Keymap::build(&BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "j"), Some(Action::Down));
        assert_eq!(km.resolve(Mode::Normal, "gg"), Some(Action::Top));
    }

    #[test]
    fn g_is_prefix_of_gg() {
        let km = Keymap::build(&BTreeMap::new());
        assert!(km.is_prefix("g"));
        assert!(km.resolve(Mode::Normal, "g").is_none());
    }

    #[test]
    fn user_override_remaps_quit() {
        let mut user = BTreeMap::new();
        user.insert("quit".to_string(), vec!["Q".to_string()]);
        let km = Keymap::build(&user);
        assert_eq!(km.resolve(Mode::Normal, "Q"), Some(Action::Quit));
        assert_eq!(km.resolve(Mode::Normal, "q"), None);
    }

    #[test]
    fn parse_chord_handles_modifiers() {
        assert_eq!(
            parse_chord("ctrl-d"),
            Some((KeyModifiers::CONTROL, KeyCode::Char('d')))
        );
    }

    #[test]
    fn dash_resolves_to_parent() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "-"), Some(Action::Parent));
        assert_eq!(Action::from_name("parent"), Some(Action::Parent));
    }

    #[test]
    fn space_toggles_mark_and_ctrl_a_selects_all() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, " "), Some(Action::ToggleMark));
        assert_eq!(
            km.resolve(Mode::Normal, "ctrl-a"),
            Some(Action::SelectAllVisible)
        );
        assert_eq!(Action::from_name("toggle_mark"), Some(Action::ToggleMark));
        assert_eq!(
            Action::from_name("select_all_visible"),
            Some(Action::SelectAllVisible)
        );
    }
}
