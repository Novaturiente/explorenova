use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Input,
    Confirm,
    SortMenu,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    ToggleVisualMode,
    Toggle,
    OpenDefault,
    OpenWith,
    FocusNext,
    SidebarNextSection,
    ToggleBookmarksPane,
    TogglePreviewPane,
    ToggleFooter,
    ResizeShrink,
    ResizeGrow,
    ReorderLeft,
    ReorderRight,
    Create,
    Copy,
    Cut,
    Paste,
    Rename,
    BulkRename,
    Trash,
    PermanentDelete,
    TogglePin,
    Search,
    SearchNext,
    SearchPrev,
    ContentSearch,
    Filter,
    PathInput,
    ShellOnce,
    ShellDrop,
    ScriptPick,
    ToggleHidden,
    Refresh,
    ShowDevices,
    /// Pending sentinel for the SMB-add text input. No keybinding —
    /// triggered from inside the devices modal.
    AddSmb,
    JumpHome,
    ZoxideJump,
    OpenTrash,
    SysClipCopy,
    NewTab,
    CloseTab,
    SortMenu,
    SortByName,
    SortBySize,
    SortByMtime,
    SortByExt,
    SortToggleReverse,
    SortToggleDirFirst,
    CycleLinemode,
    PreviewScrollDown,
    PreviewScrollUp,
    Help,
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
            "sidebar_next_section" => SidebarNextSection,
            "toggle_bookmarks_pane" => ToggleBookmarksPane,
            "toggle_preview_pane" => TogglePreviewPane,
            "toggle_footer" => ToggleFooter,
            "resize_shrink" => ResizeShrink, "resize_grow" => ResizeGrow,
            "reorder_left" => ReorderLeft, "reorder_right" => ReorderRight,
            "create" => Create, "copy" => Copy, "cut" => Cut, "paste" => Paste,
            "rename" => Rename, "bulk_rename" => BulkRename,
            "trash" => Trash, "permanent_delete" => PermanentDelete,
            "toggle_pin" | "toggle_bookmark" => TogglePin,
            "search" => Search, "search_next" => SearchNext, "search_prev" => SearchPrev,
            "content_search" => ContentSearch,
            "filter" => Filter,
            "path_input" => PathInput,
            "shell_once" => ShellOnce, "shell_drop" => ShellDrop,
            "script_pick" => ScriptPick,
            "toggle_hidden" => ToggleHidden, "refresh" => Refresh,
            "show_devices" => ShowDevices,
            "jump_home" => JumpHome,
            "zoxide_jump" => ZoxideJump,
            "open_trash" => OpenTrash,
            "sys_clip_copy" => SysClipCopy,
            "new_tab" => NewTab,
            "close_tab" => CloseTab,
            "sort_menu" => SortMenu,
            "sort_name" => SortByName,
            "sort_size" => SortBySize,
            "sort_mtime" => SortByMtime,
            "sort_ext" => SortByExt,
            "sort_reverse" => SortToggleReverse,
            "sort_dir_first" => SortToggleDirFirst,
            "cycle_linemode" => CycleLinemode,
            "preview_down" => PreviewScrollDown,
            "preview_up" => PreviewScrollUp,
            "help" => Help,
            "parent" => Parent,
            "toggle_mark" => ToggleMark,
            "toggle_visual_mode" => ToggleVisualMode,
            "select_all_visible" => SelectAllVisible,
            "quit" => Quit, "cancel" => Cancel,
            _ => return None,
        })
    }
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

    pub(crate) fn defaults() -> Vec<(Action, Vec<&'static str>)> {
        use Action::*;
        vec![
            (Down, vec!["j", "down"]), (Up, vec!["k", "up"]),
            (Top, vec!["gg"]), (Bottom, vec!["G"]),
            (HalfDown, vec!["ctrl-d"]), (HalfUp, vec!["ctrl-u"]),
            (Open, vec!["l", "right"]), (Collapse, vec!["h", "left"]),
            (Parent, vec!["-"]),
            (ToggleMark, vec![" "]),
            (SelectAllVisible, vec!["ctrl-a"]),
            (ToggleVisualMode, vec!["v"]),
            (Toggle, vec!["enter"]),
            (OpenDefault, vec!["o"]), (OpenWith, vec!["O"]),
            (FocusNext, vec!["tab"]),
            (SidebarNextSection, vec!["backtab"]),
            (ToggleBookmarksPane, vec!["B"]), (TogglePreviewPane, vec!["P"]),
            (ToggleFooter, vec!["F"]),
            (ResizeShrink, vec!["<"]), (ResizeGrow, vec![">"]),
            (ReorderLeft, vec!["["]), (ReorderRight, vec!["]"]),
            (Create, vec!["a"]), (Copy, vec!["yy"]), (Cut, vec!["dd"]),
            (Paste, vec!["p"]), (Rename, vec!["r"]), (BulkRename, vec!["b"]),
            (Trash, vec!["x", "del"]), (PermanentDelete, vec!["X"]),
            (OpenTrash, vec!["T"]),
            (SysClipCopy, vec!["Y"]),
            (TogglePin, vec!["m"]),
            (Search, vec!["/"]), (SearchNext, vec!["n"]), (SearchPrev, vec!["N"]),
            (Filter, vec!["f"]),
            (PathInput, vec![":"]),
            (ShellOnce, vec!["!"]), (ShellDrop, vec!["S"]),
            (ScriptPick, vec!["\\"]),
            (ToggleHidden, vec!["."]), (Refresh, vec!["R"]),
            (ShowDevices, vec!["M"]),
            (JumpHome, vec!["H"]),
            (ZoxideJump, vec!["z"]),
            (NewTab, vec!["t"]), (CloseTab, vec!["w"]),
            (SortMenu, vec!["s"]),
            (CycleLinemode, vec!["L"]),
            (PreviewScrollDown, vec!["ctrl-j"]),
            (PreviewScrollUp, vec!["ctrl-k"]),
            (Help, vec!["?"]),
            (Quit, vec!["q"]), (Cancel, vec!["esc"]),
        ]
    }

    /// Resolve a pending chord buffer in the given mode. Returns the action and
    /// whether the buffer should be cleared.
    pub fn resolve(&self, mode: Mode, buffer: &str) -> Option<Action> {
        match mode {
            Mode::Normal => self.normal.get(buffer).copied(),
            // Search, Input, Confirm and Help never arrive here: `handle_key`
            // routes each of them to its own handler, which reads `KeyCode`
            // directly so it can see the Backspace/Home/End keys that
            // `key_token` drops. The mappings that used to sit here were
            // dead code that looked authoritative.
            Mode::Search | Mode::Input | Mode::Confirm | Mode::Help => None,
            Mode::SortMenu => match buffer {
                "esc" => Some(Action::Cancel),
                "1" | "n" => Some(Action::SortByName),
                "2" | "s" => Some(Action::SortBySize),
                "3" | "m" => Some(Action::SortByMtime),
                "4" | "e" => Some(Action::SortByExt),
                "r" => Some(Action::SortToggleReverse),
                "d" => Some(Action::SortToggleDirFirst),
                _ => None,
            },
        }
    }

    /// Could `buffer` still grow into a binding (e.g. "g" -> "gg")?
    ///
    /// The match has to land on a token boundary. A plain `starts_with` made
    /// `c` look like a prefix of `ctrl-d` and `e` like a prefix of `esc`,
    /// even though both of those arrive from [`key_token`] whole and are
    /// never typed letter by letter. Pressing such a key twice then left the
    /// chord buffer showing a pending `c` in the status bar indefinitely,
    /// because the dead-prefix retry re-pushed the token and the guard that
    /// should have cleared it asked this same question again.
    pub fn is_prefix(&self, buffer: &str) -> bool {
        self.normal.keys().any(|k| {
            k.len() > buffer.len() && k.starts_with(buffer) && on_token_boundary(k, buffer.len())
        })
    }

    /// Would a single-token `[commands]` key shadow a builtin binding?
    /// True if `token` is itself a bound chord, or is the first key of a
    /// modifier-free multi-key builtin chord (e.g. `g` begins `gg`) — pressing
    /// the command on `g` would prevent `gg` from ever resolving.
    ///
    /// Atomic tokens (`ctrl-d`, `esc`) arrive whole and are never grown
    /// key-by-key, so a command on `c` is NOT a real prefix of `ctrl-d`;
    /// [`on_token_boundary`] is what draws that line.
    pub fn would_shadow(&self, token: &str) -> bool {
        self.normal.contains_key(token)
            || self.normal.keys().any(|k| {
                k.len() > token.len() && k.starts_with(token) && on_token_boundary(k, token.len())
            })
    }

    /// Reverse-lookup: all chords currently bound to `action`, most typeable
    /// first.
    ///
    /// A plain `sort()` put `down` ahead of `j` and `del` ahead of `x`, and
    /// the key-hint bar takes the first entry — so it advertised the arrow
    /// key for movement and Delete for trash. Shortest first, alphabetical
    /// within a length, puts the vim chord back in front.
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        let mut v: Vec<String> = self.normal.iter()
            .filter(|(_, &a)| a == action)
            .map(|(k, _)| k.clone())
            .collect();
        v.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        v
    }
}

/// The multi-character bases [`key_token`] can produce. Longest first so
/// `backtab` is not read as `tab`.
const NAMED_BASES: [&str; 9] =
    ["backtab", "enter", "right", "left", "down", "esc", "tab", "del", "up"];

/// Split a chord back into the tokens it was concatenated from.
fn chord_tokens(chord: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = chord;
    while !rest.is_empty() {
        let mut len = 0;
        for m in ["ctrl-", "alt-"] {
            if rest[len..].starts_with(m) {
                len += m.len();
            }
        }
        let base = &rest[len..];
        let base_len = NAMED_BASES
            .iter()
            .find(|n| base.starts_with(**n))
            .map(|n| n.len())
            .or_else(|| base.chars().next().map(char::len_utf8))
            .unwrap_or(0);
        if base_len == 0 {
            break;
        }
        len += base_len;
        out.push(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

/// Does byte offset `at` fall between two tokens of `chord`? Only there can
/// a chord buffer legitimately still be growing.
fn on_token_boundary(chord: &str, at: usize) -> bool {
    let mut off = 0;
    for t in chord_tokens(chord) {
        if off == at {
            return true;
        }
        off += t.len();
    }
    off == at
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
        KeyCode::BackTab => "backtab".into(),
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

    #[test]
    fn atomic_tokens_are_not_growable_prefixes() {
        let km = Keymap::build(&Default::default());
        // `gg` is genuinely built one `g` at a time.
        assert!(km.is_prefix("g"), "g must still wait for a second g");
        // These only ever arrive whole from key_token, so an unbound `c`
        // or `e` must resolve immediately instead of parking the buffer.
        assert!(!km.is_prefix("c"), "c is not a prefix of ctrl-d");
        assert!(!km.is_prefix("e"), "e is not a prefix of esc/enter");
        assert!(!km.is_prefix("u"), "u is not a prefix of up");
    }

    #[test]
    fn keys_for_puts_the_vim_chord_first() {
        let km = Keymap::build(&Default::default());
        assert_eq!(km.keys_for(Action::Down).first().map(String::as_str), Some("j"));
    }

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

    #[test]
    fn v_resolves_to_toggle_visual_mode() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "v"), Some(Action::ToggleVisualMode));
        assert_eq!(
            Action::from_name("toggle_visual_mode"),
            Some(Action::ToggleVisualMode)
        );
    }

    #[test]
    fn capital_m_resolves_to_show_devices() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(
            km.resolve(Mode::Normal, "M"),
            Some(Action::ShowDevices)
        );
        assert_eq!(
            Action::from_name("show_devices"),
            Some(Action::ShowDevices)
        );
    }

    #[test]
    fn capital_h_resolves_to_jump_home() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "H"), Some(Action::JumpHome));
        assert_eq!(Action::from_name("jump_home"), Some(Action::JumpHome));
        // Lowercase h is still Collapse — must not be overridden.
        assert_eq!(km.resolve(Mode::Normal, "h"), Some(Action::Collapse));
    }

    #[test]
    fn capital_y_resolves_to_sys_clip_copy() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        // Capital Y → system clipboard; lowercase yy → internal copy.
        assert_eq!(km.resolve(Mode::Normal, "Y"), Some(Action::SysClipCopy));
        assert_eq!(km.resolve(Mode::Normal, "yy"), Some(Action::Copy));
        assert_eq!(
            Action::from_name("sys_clip_copy"),
            Some(Action::SysClipCopy)
        );
    }

    #[test]
    fn t_resolves_to_new_tab_and_w_to_close_tab() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "t"), Some(Action::NewTab));
        assert_eq!(km.resolve(Mode::Normal, "w"), Some(Action::CloseTab));
        assert_eq!(Action::from_name("new_tab"), Some(Action::NewTab));
        assert_eq!(Action::from_name("close_tab"), Some(Action::CloseTab));
    }

    #[test]
    fn s_resolves_to_sort_menu() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::Normal, "s"), Some(Action::SortMenu));
        assert_eq!(Action::from_name("sort_menu"), Some(Action::SortMenu));
        assert_eq!(Action::from_name("sort_name"), Some(Action::SortByName));
        assert_eq!(Action::from_name("sort_reverse"), Some(Action::SortToggleReverse));
    }

    #[test]
    fn sort_menu_routes_digits_letters_and_esc() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        assert_eq!(km.resolve(Mode::SortMenu, "1"), Some(Action::SortByName));
        assert_eq!(km.resolve(Mode::SortMenu, "n"), Some(Action::SortByName));
        assert_eq!(km.resolve(Mode::SortMenu, "2"), Some(Action::SortBySize));
        assert_eq!(km.resolve(Mode::SortMenu, "s"), Some(Action::SortBySize));
        assert_eq!(km.resolve(Mode::SortMenu, "3"), Some(Action::SortByMtime));
        assert_eq!(km.resolve(Mode::SortMenu, "4"), Some(Action::SortByExt));
        assert_eq!(km.resolve(Mode::SortMenu, "r"), Some(Action::SortToggleReverse));
        assert_eq!(km.resolve(Mode::SortMenu, "d"), Some(Action::SortToggleDirFirst));
        assert_eq!(km.resolve(Mode::SortMenu, "esc"), Some(Action::Cancel));
        assert_eq!(km.resolve(Mode::SortMenu, "x"), None);
    }

    #[test]
    fn capital_t_resolves_to_open_trash_not_trash() {
        let km = Keymap::build(&std::collections::BTreeMap::new());
        // T opens the XDG trash dir; x/del move files INTO the trash.
        assert_eq!(km.resolve(Mode::Normal, "T"), Some(Action::OpenTrash));
        assert_eq!(km.resolve(Mode::Normal, "x"), Some(Action::Trash));
        assert_eq!(km.resolve(Mode::Normal, "del"), Some(Action::Trash));
        // Lowercase `t` is now NewTab (split-pane mode). Capital `T` still
        // opens trash; the two are intentionally distinct.
        assert_eq!(km.resolve(Mode::Normal, "t"), Some(Action::NewTab));
        assert_eq!(
            Action::from_name("open_trash"),
            Some(Action::OpenTrash)
        );
    }
}
