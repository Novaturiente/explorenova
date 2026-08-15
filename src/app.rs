use crate::apps::AppEntry;
use crate::config::Config;
use crate::keymap::{Action, Mode};
use crate::state::State;
use crate::textinput::TextInput;
use crate::tree::Arena;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Max filter matches revealed per keystroke. ponytail: each match triggers
/// a `reveal` (disk read + tree mutation) on the UI thread, so an unbounded
/// match set on a short query would freeze; cap it. Excess matches are not
/// shown (status notes "capped").
const FILTER_MAX_MATCHES: usize = 500;

#[derive(Debug, Default)]
pub enum Dialog {
    #[default]
    None,
    Input,
    Confirm,
    AppPick { items: Vec<AppEntry>, sel: usize, filter: String },
    Devices { items: Vec<crate::devices::Device>, sel: usize },
    ScriptPick { items: Vec<PathBuf>, sel: usize, filter: String },
}

impl Dialog {
    /// Move the selection cursor on dialogs that have one (AppPick,
    /// Devices). Wraps around at both ends per [`crate::ui::modal::wrap_nav`].
    /// No-op on dialogs without a selection (None, Input, Confirm).
    pub fn nav(&mut self, len: usize, forward: bool) {
        let sel = match self {
            Dialog::Devices { sel, .. } => sel,
            Dialog::AppPick { sel, .. } => sel,
            Dialog::ScriptPick { sel, .. } => sel,
            _ => return,
        };
        *sel = crate::ui::modal::wrap_nav(*sel, len, forward);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Bookmarks,
    Tree,
    Preview,
}

/// Per-tab state. The active tab's data lives directly on `App`; the
/// inactive tab (when split) is stashed in `App::tabs`. Open/close/swap
/// move state between App fields and snapshots via `mem::swap` — no
/// codebase-wide refactor needed.
pub struct PaneSnapshot {
    pub arena: Arena,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub marks: BTreeSet<PathBuf>,
    pub visual_mode: bool,
    pub search_results: Vec<(PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub filter_query: String,
    pub filter_cands: Option<Vec<PathBuf>>,
    pub preview_scroll: usize,
    pub mount_floor: Option<PathBuf>,
}

pub struct App {
    pub arena: Arena,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub mode: Mode,
    pub focus: Pane,
    pub config: Config,
    /// Parsed color theme, built once from `config.theme` in `App::new`.
    pub theme: crate::ui::theme::Theme,
    pub state: State,
    pub clipboard: Option<(Vec<PathBuf>, bool)>, // (paths, is_cut) — batch (shared across tabs)
    /// A cut clipboard stashed while its move is in flight, restored if the
    /// move fails so the user can retry without re-selecting the sources.
    pub cut_restore: Option<(Vec<PathBuf>, bool)>,
    pub marks: BTreeSet<PathBuf>,
    /// Validated `[commands]` bindings: single-token chord -> shell template.
    /// Filtered against the keymap at startup (see `command::validate_commands`).
    pub commands: std::collections::HashMap<String, String>,
    /// Set after a shell-out (editor, command, picker, drop-to-shell, SMB mount)
    /// to force a full repaint next frame — ratatui's diff alone won't repaint a
    /// screen a child clobbered. The loop clears the terminal and resets this.
    pub force_redraw: bool,
    pub status: String,
    /// True when the current `status` text is an error, so the status bar
    /// paints it red instead of the default fg. Reset by every `set_status`.
    pub status_err: bool,
    pub should_quit: bool,
    pub input: TextInput,
    pub search_results: Vec<(PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub pending: Option<Action>,
    pub dialog: Dialog,
    pub visual_mode: bool,
    pub cursor_moved_at: std::time::Instant,
    /// Visible tree rows, updated each frame from the terminal size. Drives
    /// the half-page jump (Ctrl-d/u) so it scales with the window.
    pub viewport_h: usize,
    /// Cursor inside the bookmarks pane (when focused). Independent of
    /// the tree cursor so switching focus doesn't shuffle either.
    pub bookmark_sel: usize,
    /// Inactive tab(s). Empty = single-tab mode. Len 1 = split mode
    /// (`tabs[0]` is the other pane; App fields are the active pane).
    pub tabs: Vec<PaneSnapshot>,
    /// Visual slot of the active pane in split mode: 0 = left, 1 = right.
    /// The render layer dispatches App fields to this slot and `tabs[0]`
    /// to the other slot so positions stay fixed across Tab swaps —
    /// only the highlight (and which pane receives keystrokes) moves.
    pub active_slot: u8,
    /// Global sort state (mirrors `state.sort`). Pushed into every Arena
    /// via `apply_sort_to_arenas`; persisted to state.toml on change.
    pub sort: crate::sort::SortState,
    /// Active tree linemode column (size/mtime/perms/none). Cycled with
    /// `L`, mirrored into `state.line_mode`, persisted on quit.
    pub line_mode: crate::linemode::LineMode,
    /// Cached free-disk for current root. Refreshed on cwd change OR
    /// every 30s.
    pub disk_free: Option<(PathBuf, u64, std::time::Instant)>,
    /// Timestamp of last `set_status` call. Status text fades after 3s.
    pub status_set_at: std::time::Instant,
    /// Chord buffer awaiting more keys (vim `showcmd`-style). Mirrored
    /// from the event loop's local buffer after each key so the status
    /// bar can show a pending `y`/`g` instead of appearing dead.
    pub pending_chord: String,
    /// Scroll offset (lines) for text/meta previews (ctrl-j / ctrl-k).
    /// Reset whenever the tree cursor moves. Images ignore it.
    pub preview_scroll: usize,
    /// In-flight long-running op (copy/move/extract). `None` when idle.
    /// While `Some`, the modal overlay renders a progress bar / spinner
    /// and conflicting dispatch arms are no-ops.
    pub op: Option<crate::ops::OpState>,
    /// Scroll offset (in lines) for the help overlay. Reset to 0 each
    /// time the user opens `?`.
    pub help_scroll: usize,
    /// Filter string for the help overlay. Typed chars narrow the
    /// visible entries (substring, case-insensitive). Cleared on open.
    pub help_filter: String,
    /// Cached candidate path list for the live `/` search. Populated
    /// when Search mode is entered (one tree-walk) so every keystroke
    /// only needs to re-rank, not re-collect. None when not in search.
    pub search_cands: Option<Vec<PathBuf>>,
    /// Active filter query (empty = no filter). When non-empty the tree
    /// is pruned to fuzzy matches + their ancestor dirs.
    pub filter_query: String,
    /// Cached candidate paths for the live filter (collected once on
    /// begin_filter, dropped on clear).
    pub filter_cands: Option<Vec<PathBuf>>,
    /// Saved SMB shares (loaded from shares.toml at startup, mutated on
    /// add/delete in the devices modal).
    pub shares: Vec<crate::smb::SmbShare>,
    /// Path to shares.toml; written on every add/delete.
    pub shares_path: PathBuf,
    /// When browsing a device/share mounted via the `M` modal, the
    /// mountpoint the user may not ascend above. `ascend` refuses to
    /// go past it; cleared by `reroot` (Home/Trash/bookmark jumps).
    /// `None` during ordinary filesystem browsing.
    pub mount_floor: Option<PathBuf>,
    /// Directories the app just mutated, with an expiry. The fs-watcher
    /// echo for these is ignored until the deadline so an action doesn't
    /// trigger a second (redundant) refresh.
    pub self_write_dirs: std::collections::HashSet<std::path::PathBuf>,
    pub self_write_until: Option<std::time::Instant>,
    /// ponytail: URL of the last credentialed share whose background mount
    /// attempt we kicked off (not necessarily failed — success is implied
    /// by the share showing mounted). If Enter is pressed on the SAME
    /// still-unmounted share again, dispatch falls back to the interactive
    /// `gio` prompt instead of retrying silently. Two-step UX (background
    /// try → explicit retry) instead of auto-fallback, keeps the main loop
    /// free of modal re-entry on op completion.
    pub last_mount_failed_auth: Option<String>,
    /// Auto-detected well-known dirs (Home + any of Desktop/Documents/
    /// Downloads/Music/Pictures/Videos that exist), computed once at
    /// startup. Sidebar "bookmarks" section.
    pub well_known: Vec<(String, PathBuf)>,
    /// Cached disks/shares list for the sidebar "disks" section.
    /// Populated at startup and on `Action::Refresh`; not polled live.
    pub disks: Vec<crate::devices::Device>,
}

impl App {
    pub fn new(mut arena: Arena, config: Config, state: State) -> Self {
        let sort = state.sort;
        let line_mode = state.line_mode;
        arena.set_sort(sort);
        let visible = arena.flatten();
        let (theme, theme_warnings) = crate::ui::theme::Theme::from_config(&config.theme);
        let status = if theme_warnings.is_empty() {
            String::new()
        } else {
            format!("config: {}", theme_warnings.join("; "))
        };
        App {
            arena,
            visible,
            selected: 0,
            mode: Mode::Normal,
            focus: Pane::Tree,
            config,
            theme,
            state,
            clipboard: None,
            cut_restore: None,
            marks: BTreeSet::new(),
            commands: std::collections::HashMap::new(),
            force_redraw: false,
            status,
            status_err: false,
            should_quit: false,
            input: TextInput::new(),
            search_results: Vec::new(),
            search_idx: 0,
            pending: None,
            dialog: Dialog::None,
            visual_mode: false,
            cursor_moved_at: std::time::Instant::now(),
            viewport_h: 0,
            bookmark_sel: 0,
            tabs: Vec::new(),
            active_slot: 0,
            sort,
            line_mode,
            disk_free: None,
            status_set_at: std::time::Instant::now(),
            pending_chord: String::new(),
            preview_scroll: 0,
            op: None,
            help_scroll: 0,
            help_filter: String::new(),
            search_cands: None,
            filter_query: String::new(),
            filter_cands: None,
            shares: Vec::new(),
            shares_path: PathBuf::new(),
            mount_floor: None,
            self_write_dirs: std::collections::HashSet::new(),
            self_write_until: None,
            last_mount_failed_auth: None,
            // Populated by main.rs after startup (reads real $HOME /
            // shells out to udiskie-info) — kept empty here so App::new
            // stays deterministic for tests.
            well_known: Vec::new(),
            disks: Vec::new(),
        }
    }

    /// Mark `dirs` as just-written-by-us so the watcher echo is ignored
    /// for a short window.
    pub fn mark_self_write(&mut self, dirs: impl IntoIterator<Item = std::path::PathBuf>) {
        self.self_write_dirs.extend(dirs);
        self.self_write_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
    }

    /// Set transient status text and stamp the time so the status bar
    /// can fade it after 3 seconds.
    pub fn set_status<S: Into<String>>(&mut self, s: S) {
        self.status = s.into();
        self.status_err = false;
        self.status_set_at = std::time::Instant::now();
    }

    /// Like [`set_status`] but flags the text as an error so the status bar
    /// renders it red. Use for any "… failed" / rejected-action message.
    pub fn set_err<S: Into<String>>(&mut self, s: S) {
        self.status = s.into();
        self.status_err = true;
        self.status_set_at = std::time::Instant::now();
    }

    /// Ask an in-flight op to cancel. No-op when idle.
    pub fn cancel_op(&mut self) {
        if let Some(op) = &self.op {
            op.request_cancel();
            self.set_status("cancelling…");
        }
    }

    /// Reveal `target` in the tree, reflatten, and move the cursor to
    /// the revealed row. Returns true if the path was found and selected.
    ///
    /// Centralises the reveal → flatten → position pattern used after
    /// any operation that may expand new ancestors (paste, extract,
    /// rename, create, refresh, search jump). Callers should prefer this
    /// over hand-rolling the three-step dance — it is the only way to
    /// keep `visible` and `selected` consistent.
    pub fn navigate_to(&mut self, target: &std::path::Path) -> bool {
        if let Ok(Some(id)) = self.arena.reveal(target) {
            self.visible = self.arena.flatten();
            if let Some(i) = self.visible.iter().position(|&n| n == id) {
                self.selected = i;
                return true;
            }
        }
        false
    }

    /// Compute the size sum of marked files (skipping directories).
    /// Called by the status renderer per frame.
    ///
    /// O(nodes · log marks) single pass — safe to call per frame even after
    /// select-all on a large tree (the old shape was O(marks · nodes)).
    pub fn marks_bytes(&self) -> u64 {
        self.arena.nodes.iter()
            .filter(|n| !n.is_dir() && self.marks.contains(&n.path))
            .map(|n| n.size)
            .sum()
    }

    /// Total byte weight of the current delete/op targets (marks, else the
    /// cursor file). Dirs contribute 0 — we don't recurse for a confirm line.
    ///
    /// Same single-pass shape as `marks_bytes`: build the lookup set once,
    /// then one arena pass.
    pub fn targets_bytes(&self) -> u64 {
        let targets = self.targets();
        let set: BTreeSet<&std::path::Path> = targets.iter().map(|p| p.as_path()).collect();
        self.arena.nodes.iter()
            .filter(|n| !n.is_dir() && set.contains(n.path.as_path()))
            .map(|n| n.size)
            .sum()
    }

    /// Refresh `disk_free` if cache is stale (>30s) or cwd changed.
    pub fn refresh_disk_free(&mut self) {
        use std::time::{Duration, Instant};
        let cwd = self.arena.nodes[self.arena.root].path.clone();
        let stale = match &self.disk_free {
            None => true,
            Some((p, _, t)) => p != &cwd || t.elapsed() > Duration::from_secs(30),
        };
        if stale {
            if let Some(b) = crate::status::disk_free_bytes(&cwd) {
                self.disk_free = Some((cwd, b, Instant::now()));
            }
        }
    }

    /// Push current sort to active arena AND each tab snapshot; re-flatten
    /// all visible lists and reclamp the cursor.
    ///
    /// The cursor follows the previously-selected *file* (via `navigate_to`),
    /// not its old row index — a resort reorders rows, so keeping the index
    /// would land the cursor on a different file.
    pub fn apply_sort(&mut self) {
        let sel_path = self.visible.get(self.selected)
            .map(|&id| self.arena.nodes[id].path.clone());
        self.arena.set_sort(self.sort);
        match sel_path {
            Some(p) if self.navigate_to(&p) => {}
            _ => {
                self.visible = self.arena.flatten();
                let cap = self.visible.len().saturating_sub(1);
                self.selected = self.selected.min(cap);
            }
        }
        for tab in self.tabs.iter_mut() {
            tab.arena.set_sort(self.sort);
            tab.visible = tab.arena.flatten();
            let cap = tab.visible.len().saturating_sub(1);
            tab.selected = tab.selected.min(cap);
        }
        self.state.sort = self.sort;
    }

    pub fn set_sort_mode(&mut self, mode: crate::sort::SortMode) {
        self.sort.mode = mode;
    }

    pub fn toggle_sort_reverse(&mut self) {
        self.sort.reverse = !self.sort.reverse;
    }

    pub fn toggle_sort_dir_first(&mut self) {
        self.sort.dir_first = !self.sort.dir_first;
    }

    /// True when two tabs are open (split-pane mode).
    pub fn is_split(&self) -> bool {
        !self.tabs.is_empty()
    }

    /// Open a second tab rooted at the current tab's root. The new tab
    /// becomes active; the previous tab is stashed in `tabs[0]`. No-op
    /// when already split (cap = 2 tabs).
    pub fn open_tab(&mut self) -> std::io::Result<()> {
        if self.is_split() {
            return Ok(());
        }
        let root_path = self.arena.nodes[self.arena.root].path.clone();
        let mut new_arena = crate::tree::Arena::new(&root_path)?;
        new_arena.set_sort(self.sort);
        let snap = PaneSnapshot {
            arena: std::mem::replace(&mut self.arena, new_arena),
            visible: std::mem::take(&mut self.visible),
            selected: std::mem::replace(&mut self.selected, 0),
            marks: std::mem::take(&mut self.marks),
            visual_mode: std::mem::replace(&mut self.visual_mode, false),
            search_results: std::mem::take(&mut self.search_results),
            search_idx: std::mem::replace(&mut self.search_idx, 0),
            filter_query: std::mem::take(&mut self.filter_query),
            filter_cands: std::mem::take(&mut self.filter_cands),
            preview_scroll: std::mem::replace(&mut self.preview_scroll, 0),
            mount_floor: std::mem::take(&mut self.mount_floor),
        };
        self.visible = self.arena.flatten();
        self.tabs.push(snap);
        // New tab lives in the right slot and starts active.
        self.active_slot = 1;
        Ok(())
    }

    /// Close the active tab, restoring `tabs[0]`'s state as the active
    /// pane. No-op when not split.
    pub fn close_tab(&mut self) {
        let Some(snap) = self.tabs.pop() else {
            return;
        };
        self.arena = snap.arena;
        self.visible = snap.visible;
        self.selected = snap.selected;
        self.marks = snap.marks;
        self.visual_mode = snap.visual_mode;
        self.search_results = snap.search_results;
        self.search_idx = snap.search_idx;
        self.filter_query = snap.filter_query;
        self.filter_cands = snap.filter_cands;
        self.preview_scroll = snap.preview_scroll;
        self.mount_floor = snap.mount_floor;
        // Back to single mode — only one slot exists.
        self.active_slot = 0;
    }

    /// Move focus to the other tab without changing visual positions.
    /// Internally swaps the data between App fields and `tabs[0]` and
    /// flips `active_slot`, so the render layer puts the active pane in
    /// the same screen position it occupied before — only the highlight
    /// (and the pane that now receives keystrokes) moves.
    pub fn swap_tab(&mut self) {
        let Some(snap) = self.tabs.first_mut() else {
            return;
        };
        std::mem::swap(&mut self.arena, &mut snap.arena);
        std::mem::swap(&mut self.visible, &mut snap.visible);
        std::mem::swap(&mut self.selected, &mut snap.selected);
        std::mem::swap(&mut self.marks, &mut snap.marks);
        std::mem::swap(&mut self.visual_mode, &mut snap.visual_mode);
        std::mem::swap(&mut self.search_results, &mut snap.search_results);
        std::mem::swap(&mut self.search_idx, &mut snap.search_idx);
        std::mem::swap(&mut self.filter_query, &mut snap.filter_query);
        std::mem::swap(&mut self.filter_cands, &mut snap.filter_cands);
        std::mem::swap(&mut self.preview_scroll, &mut snap.preview_scroll);
        std::mem::swap(&mut self.mount_floor, &mut snap.mount_floor);
        self.active_slot ^= 1;
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
                if self.focus == Pane::Bookmarks {
                    let n = crate::sidebar::build(self).len();
                    if n > 0 && self.bookmark_sel + 1 < n {
                        self.bookmark_sel += 1;
                    }
                } else if self.selected + 1 < self.visible.len() {
                    self.selected += 1;
                }
            }
            Action::Up => {
                if self.focus == Pane::Bookmarks {
                    self.bookmark_sel = self.bookmark_sel.saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            Action::Top => {
                if self.focus == Pane::Bookmarks {
                    self.bookmark_sel = 0;
                } else {
                    self.selected = 0;
                }
            }
            Action::Bottom => {
                if self.focus == Pane::Bookmarks {
                    self.bookmark_sel =
                        crate::sidebar::build(self).len().saturating_sub(1);
                } else {
                    self.selected = self.visible.len().saturating_sub(1);
                }
            }
            Action::HalfDown => {
                // Half the visible page, scaling with the window; fall back
                // to 10 before the first frame sets viewport_h.
                let step = if self.viewport_h >= 2 { self.viewport_h / 2 } else { 10 };
                if self.focus == Pane::Bookmarks {
                    let n = crate::sidebar::build(self).len();
                    self.bookmark_sel =
                        (self.bookmark_sel + step).min(n.saturating_sub(1));
                } else {
                    self.selected = (self.selected + step)
                        .min(self.visible.len().saturating_sub(1));
                }
            }
            Action::HalfUp => {
                let step = if self.viewport_h >= 2 { self.viewport_h / 2 } else { 10 };
                if self.focus == Pane::Bookmarks {
                    self.bookmark_sel = self.bookmark_sel.saturating_sub(step);
                } else {
                    self.selected = self.selected.saturating_sub(step);
                }
            }
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
                // Tab cycles between the two interactive panes only.
                // The preview pane has no cursor or actions; skipping
                // it makes Tab feel symmetric (bookmarks ↔ tree).
                // If the bookmarks pane is hidden, Tab is a no-op.
                self.focus = match self.focus {
                    Pane::Tree if self.config.panes.bookmarks.visible => {
                        Pane::Bookmarks
                    }
                    Pane::Bookmarks => Pane::Tree,
                    Pane::Preview => Pane::Tree, // never reached normally
                    Pane::Tree => Pane::Tree,
                };
            }
            Action::SidebarNextSection => {
                // Shift-Tab: jump the cursor to the next sidebar box
                // (bookmarks -> pinned -> disks -> wrap), skipping empty
                // sections. No-op outside the sidebar pane.
                if self.focus == Pane::Bookmarks {
                    let rows = crate::sidebar::build(self);
                    let ranges = crate::sidebar::section_ranges(&rows);
                    let cur = ranges.iter().position(|&(_, start, count)| {
                        count > 0 && self.bookmark_sel >= start && self.bookmark_sel < start + count
                    });
                    if let Some(cur) = cur {
                        for step in 1..=ranges.len() {
                            let next = (cur + step) % ranges.len();
                            if ranges[next].2 > 0 {
                                self.bookmark_sel = ranges[next].1;
                                break;
                            }
                        }
                    } else if let Some(&(_, start, _)) = ranges.iter().find(|&&(_, _, c)| c > 0) {
                        self.bookmark_sel = start;
                    }
                }
            }
            Action::ToggleBookmarksPane => {
                self.config.panes.bookmarks.visible = !self.config.panes.bookmarks.visible;
                // Don't leave focus stranded on a now-hidden pane, or j/k
                // and Enter would drive an invisible cursor.
                if !self.config.panes.bookmarks.visible && self.focus == Pane::Bookmarks {
                    self.focus = Pane::Tree;
                }
            }
            Action::TogglePreviewPane => {
                self.config.panes.preview.visible = !self.config.panes.preview.visible;
            }
            Action::TogglePin => {
                if self.focus == Pane::Bookmarks {
                    // Only the Pinned section is user-editable — Bookmarks
                    // (well-known dirs) and Disks are auto-derived.
                    let rows = crate::sidebar::build(self);
                    let target = rows
                        .get(self.bookmark_sel)
                        .filter(|r| r.section == crate::sidebar::Section::Pinned)
                        .and_then(|r| r.path.clone());
                    match target {
                        Some(p) => {
                            self.state.toggle_pin(p);
                            let n = crate::sidebar::build(self).len();
                            if n == 0 {
                                self.bookmark_sel = 0;
                            } else if self.bookmark_sel >= n {
                                self.bookmark_sel = n - 1;
                            }
                        }
                        None => self.set_status("not pinnable"),
                    }
                } else if let Some(id) = self.sel_id() {
                    self.state.toggle_pin(
                        self.arena.nodes[id].path.clone(),
                    );
                }
            }
            Action::Quit => self.should_quit = true,
            Action::Cancel => {
                self.mode = Mode::Normal;
                let n = self.marks.len();
                self.marks.clear();
                self.visual_mode = false;
                if n > 0 {
                    // Esc silently nuking a large selection reads as data
                    // loss; say what happened.
                    self.set_status(format!("cleared {n} mark{}", if n == 1 { "" } else { "s" }));
                }
            }
            Action::PreviewScrollDown => {
                self.preview_scroll = self.preview_scroll.saturating_add(5);
            }
            Action::PreviewScrollUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(5);
            }
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

    /// Enter Search mode. The directory walk is deferred to the first
    /// non-empty keystroke (see `live_search`) so pressing `/` opens the
    /// prompt instantly instead of freezing on a full-tree walk.
    pub fn begin_search(&mut self) {
        self.search_cands = None;
        self.search_results.clear();
        self.search_idx = 0;
    }

    /// Re-rank the candidate list against the current input, then jump the
    /// cursor to the top match. The candidate list is collected lazily on
    /// the first non-empty query (capped) and cached for later keystrokes.
    pub fn live_search(&mut self) {
        let root = self.arena.nodes[self.arena.root].path.clone();
        let query = self.input.value();
        if query.is_empty() {
            self.search_results.clear();
            self.search_idx = 0;
            return;
        }
        if self.search_cands.is_none() {
            self.search_cands = Some(crate::search::collect(&root, self.arena.show_hidden));
        }
        let cands = self.search_cands.as_ref().unwrap();
        // At the walk cap the index is incomplete — matches beyond it are
        // silently absent, so say so instead of implying a full search.
        let capped = cands.len() >= crate::search::WALK_MAX_ENTRIES;
        let ranked = crate::search::rank(cands, &root, &query);
        self.search_results = ranked
            .into_iter()
            .map(|(p, _, idx)| (p, idx))
            .collect();
        self.search_idx = 0;
        let n = self.search_results.len();
        let cap_note = if capped { " — index capped, deep matches missed" } else { "" };
        if n == 0 {
            self.set_status(format!("no matches{cap_note}"));
        } else {
            self.set_status(format!("1/{n} matches{cap_note}"));
        }
        self.jump_to_search();
    }

    /// Drop the cached candidate list. Called when Search mode exits
    /// (Enter or Esc) so we don't hold a giant Vec around indefinitely.
    pub fn end_search(&mut self) {
        self.search_cands = None;
    }

    pub fn jump_to_search(&mut self) {
        if let Some((p, _)) = self.search_results.get(self.search_idx).cloned() {
            self.navigate_to(&p);
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
        self.set_status(format!("{}/{n} matches", self.search_idx + 1));
        self.jump_to_search();
    }

    pub fn filter_active(&self) -> bool {
        !self.filter_query.is_empty()
    }

    /// Enter filter mode. The directory walk is deferred to the first
    /// non-empty keystroke (see `live_filter`) so pressing `f` opens the
    /// prompt instantly instead of freezing on a full-tree walk.
    pub fn begin_filter(&mut self) {
        self.filter_cands = None;
        self.filter_query.clear();
    }

    /// Recompute the allowed set from the current input and re-prune the
    /// tree. Empty query clears the prune (shows everything) but keeps the
    /// session candidate cache.
    ///
    /// ponytail: recomputes the allowed set (reveal each match) on every
    /// keystroke — fine for typical dirs; a 100k-node tree would lag.
    pub fn live_filter(&mut self) {
        self.filter_query = self.input.value();
        if self.filter_query.is_empty() {
            self.arena.filter = None;
            self.visible = self.arena.flatten();
            self.selected = self.selected.min(self.visible.len().saturating_sub(1));
            self.set_status("filter: (all)");
            return;
        }
        let root = self.arena.nodes[self.arena.root].path.clone();
        // Collect lazily on the first non-empty query (capped) and cache.
        if self.filter_cands.is_none() {
            self.filter_cands = Some(crate::search::collect(&root, self.arena.show_hidden));
        }
        let cands = self.filter_cands.as_ref().unwrap();
        let capped = cands.len() >= crate::search::WALK_MAX_ENTRIES;
        let ranked = crate::search::rank(cands, &root, &self.filter_query);
        let mut allowed: std::collections::HashSet<usize> = std::collections::HashSet::new();
        // Cap reveals: each reveal hits disk + mutates the tree on the UI
        // thread, so an unbounded match set on a one-char query would freeze.
        for (p, _, _) in ranked.iter().take(FILTER_MAX_MATCHES) {
            if let Ok(Some(id)) = self.arena.reveal(p) {
                let mut cur = Some(id);
                while let Some(c) = cur {
                    allowed.insert(c);
                    cur = self.arena.nodes[c].parent;
                }
            }
        }
        let n = ranked.len();
        self.arena.filter = Some(allowed);
        self.visible = self.arena.flatten();
        self.selected = 0;
        let cap_note = if capped { " — index capped" } else { "" };
        if n == 0 {
            self.set_status(format!("filter: no matches{cap_note}"));
        } else if n > FILTER_MAX_MATCHES {
            self.set_status(format!("filter: {FILTER_MAX_MATCHES}/{n} (capped){cap_note}"));
        } else {
            self.set_status(format!("filter: {n} matches{cap_note}"));
        }
    }

    /// Clear an active filter and restore the full tree.
    pub fn clear_filter(&mut self) {
        self.filter_query.clear();
        self.filter_cands = None;
        if self.arena.filter.is_some() {
            self.arena.filter = None;
            let sel_path = self.visible.get(self.selected)
                .map(|&id| self.arena.nodes[id].path.clone());
            self.visible = self.arena.flatten();
            match sel_path {
                Some(p) if self.navigate_to(&p) => {}
                _ => self.selected = self.selected.min(self.visible.len().saturating_sub(1)),
            }
        }
    }

    /// Returns the created path on success so callers can refresh the
    /// arena and snap the cursor to the new entry. None on failure.
    pub fn commit_create(&mut self) -> Option<PathBuf> {
        let base = self
            .sel_dir()
            .unwrap_or_else(|| self.arena.nodes[self.arena.root].path.clone());
        // Strip any trailing separator from the user's input so the
        // returned path is a clean file/dir handle; fs_ops::create has
        // already used the trailing-slash signal to pick dir vs file.
        let raw = self.input.value();
        let stripped = raw.trim_end_matches(std::path::MAIN_SEPARATOR);
        let target = base.join(stripped);
        let out = match crate::fs_ops::create(&base.join(&raw)) {
            Ok(_) => {
                self.set_status(format!("created {}", target.display()));
                Some(target)
            }
            Err(e) => {
                self.set_err(format!("create failed: {e}"));
                None
            }
        };
        self.mode = Mode::Normal;
        out
    }

    /// Returns the renamed path on success so callers can refresh the
    /// arena and snap the cursor to the new entry. None on failure.
    pub fn commit_rename(&mut self) -> Option<PathBuf> {
        let mut out = None;
        if let Some(&id) = self.visible.get(self.selected) {
            let p = self.arena.nodes[id].path.clone();
            match crate::fs_ops::rename(&p, &self.input.value()) {
                Ok(np) => {
                    self.set_status(format!("renamed -> {}", np.display()));
                    out = Some(np);
                }
                Err(e) => self.set_err(format!("rename failed: {e}")),
            }
        }
        self.mode = Mode::Normal;
        out
    }

    pub fn toggle_mark(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            let p = self.arena.nodes[id].path.clone();
            if self.marks.contains(&p) {
                self.marks.remove(&p);
            } else {
                self.marks.insert(p);
            }
        }
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    pub fn select_all_visible(&mut self) {
        for &id in &self.visible {
            let p = self.arena.nodes[id].path.clone();
            self.marks.insert(p);
        }
    }

    /// Flip visual mode. When entering, mark the cursor row so a bare
    /// `v` press selects the current item even without movement.
    pub fn toggle_visual_mode(&mut self) {
        self.visual_mode = !self.visual_mode;
        if self.visual_mode {
            self.mark_cursor();
        }
    }

    /// Insert the cursor row's path into `marks` (idempotent; never
    /// removes). Used by visual-mode auto-mark on movement.
    pub fn mark_cursor(&mut self) {
        if let Some(&id) = self.visible.get(self.selected) {
            self.marks.insert(self.arena.nodes[id].path.clone());
        }
    }

    /// Insert every visible row in `[lo, hi]` (inclusive) into `marks`.
    /// Used by visual-mode auto-mark on multi-row jumps (G/gg/Ctrl-d).
    pub fn mark_range(&mut self, lo: usize, hi: usize) {
        let hi = hi.min(self.visible.len().saturating_sub(1));
        for i in lo..=hi {
            if let Some(&id) = self.visible.get(i) {
                self.marks.insert(self.arena.nodes[id].path.clone());
            }
        }
    }

    /// Shared chokepoint for every script trigger: expand a command template
    /// against the current selection and run it (suspending the TUI). No-op
    /// with a status message when nothing is selected. Used by the `!` ad-hoc
    /// prompt, `[commands]` keys, and the scripts-dir picker.
    pub fn run_command(&mut self, template: &str) {
        let targets = self.targets();
        // Block only when the template needs the selection but none exists, so a
        // fileless command (mkdir foo) still works the way bare `!` always did.
        if targets.is_empty() && crate::command::references_selection(template) {
            self.set_status("no selection");
            return;
        }
        let root = self.arena.nodes[self.arena.root].path.clone();
        let dir = self.sel_dir().unwrap_or_else(|| root.clone());
        let cmd = crate::command::expand(template, &crate::command::Subst { targets, dir });
        if let Err(e) = crate::shell::run_once(&root, &cmd) {
            self.set_err(format!("shell failed: {e}"));
        }
        // The child clobbered the alt screen; force a full repaint next frame.
        self.force_redraw = true;
    }

    /// The opener template for `path`'s extension, if any. Pure lookup.
    pub fn opener_for(&self, path: &std::path::Path) -> Option<String> {
        let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();
        self.config.openers.get(&ext).cloned()
    }

    /// Open `path`: run its extension opener (against just this path), or
    /// fall back to the OS default app. Used by `o` and Enter-on-file.
    /// ponytail: openers run suspended via `run_once` (like `!` and `[commands]`) —
    /// correct for terminal editors; a GUI app briefly suspends the TUI until it's closed.
    /// Add a non-blocking flag later if that feels laggy.
    pub fn open_path(&mut self, path: &std::path::Path) {
        if let Some(template) = self.opener_for(path) {
            let dir = path.parent().map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.arena.nodes[self.arena.root].path.clone());
            let subst = crate::command::Subst { targets: vec![path.to_path_buf()], dir };
            let cmd = crate::command::expand(&template, &subst);
            if let Err(e) = crate::shell::run_once(
                &self.arena.nodes[self.arena.root].path.clone(), &cmd) {
                self.set_err(format!("opener failed: {e}"));
            }
            self.force_redraw = true;
        } else if let Err(e) = open::that_detached(path) {
            self.set_err(format!("open failed: {e}"));
        }
    }

    /// Single chokepoint for the "selection-if-any-else-cursor" rule.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marks.is_empty() {
            self.marks.iter().cloned().collect()
        } else if let Some(&id) = self.visible.get(self.selected) {
            vec![self.arena.nodes[id].path.clone()]
        } else {
            Vec::new()
        }
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
            let targets = self.targets();
            let n = targets.len();
            let mut ok = 0usize;
            let mut failed: Vec<String> = Vec::new();
            for p in &targets {
                match crate::fs_ops::permanent_delete(p) {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("{}: {e}", p.display())),
                }
            }
            let msg = if failed.is_empty() {
                if n == 1 {
                    format!("deleted {}", targets[0].display())
                } else {
                    format!("deleted {n}")
                }
            } else {
                let preview =
                    failed.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                let tail = if failed.len() > 2 { ", …" } else { "" };
                format!(
                    "deleted {}/{} ({} failed: {}{})",
                    ok,
                    n,
                    failed.len(),
                    preview,
                    tail
                )
            };
            self.set_status(msg);
            self.marks.clear();
        }
        // Action::Trash never reaches here — handle_confirm_key intercepts
        // it before calling confirm_yes and routes through spawn_trash
        // (the background op) instead. One trash implementation.
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

    /// Build an App rooted at `path` (unlike `app()`, which always roots
    /// at "."). Needed by tests that assert on paths inside a tempdir.
    fn test_app(path: &std::path::Path) -> App {
        let arena = Arena::new(path).unwrap();
        App::new(arena, Config::with_defaults(), State::default())
    }

    /// An App already split into two tabs (via `open_tab`), for tests
    /// that exercise `swap_tab`/`close_tab`.
    fn test_app_with_tab(path: &std::path::Path) -> App {
        let mut app = test_app(path);
        app.open_tab().unwrap();
        app
    }

    fn app_with_theme(pairs: &[(&str, &str)]) -> App {
        let arena = Arena::new(std::path::Path::new(".")).unwrap_or_else(|_| {
            Arena::new(std::env::temp_dir().as_path()).unwrap()
        });
        let mut config = Config::with_defaults();
        for (k, v) in pairs {
            config.theme.insert((*k).to_string(), (*v).to_string());
        }
        App::new(arena, config, State::default())
    }

    #[test]
    fn opener_for_matches_extension() {
        let mut a = app();
        a.config.openers.insert("md".into(), "glow {file}".into());
        assert_eq!(a.opener_for(std::path::Path::new("/x/readme.md")).as_deref(), Some("glow {file}"));
        assert!(a.opener_for(std::path::Path::new("/x/photo.png")).is_none());
        assert!(a.opener_for(std::path::Path::new("/x/Makefile")).is_none());
    }

    #[test]
    fn app_new_builds_theme_from_config() {
        let app = app_with_theme(&[("dir", "#102030")]);
        assert_eq!(app.theme.dir, ratatui::style::Color::Rgb(16, 32, 48));
    }

    #[test]
    fn app_new_surfaces_theme_warnings_to_status() {
        let app = app_with_theme(&[("dir", "blu")]);
        assert!(app.status.contains("dir"), "bad theme color should surface in status");
    }

    #[test]
    fn hiding_bookmarks_pane_moves_focus_to_tree() {
        let mut a = app();
        a.config.panes.bookmarks.visible = true;
        a.focus = Pane::Bookmarks;
        a.apply(Action::ToggleBookmarksPane); // hides it
        assert!(!a.config.panes.bookmarks.visible);
        assert_eq!(a.focus, Pane::Tree, "focus must not stay on a hidden pane");
    }

    #[test]
    fn down_then_up_returns_to_zero() {
        let mut a = app();
        a.apply(Action::Down);
        a.apply(Action::Up);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn set_err_flags_status_and_set_status_clears_it() {
        let mut a = app();
        a.set_err("boom");
        assert!(a.status_err, "set_err must mark the status as an error");
        assert_eq!(a.status, "boom");
        a.set_status("ok");
        assert!(!a.status_err, "set_status must reset the error flag");
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
    fn focus_cycles_between_tree_and_bookmarks_only() {
        let mut a = app();
        a.config.panes.bookmarks.visible = true;
        assert_eq!(a.focus, Pane::Tree);
        a.apply(Action::FocusNext);
        assert_eq!(a.focus, Pane::Bookmarks, "tree → bookmarks");
        a.apply(Action::FocusNext);
        assert_eq!(a.focus, Pane::Tree, "bookmarks → tree (no preview)");
    }

    #[test]
    fn shift_tab_jumps_between_sidebar_sections() {
        let mut a = app();
        a.focus = Pane::Bookmarks;
        a.well_known = vec![("Home".to_string(), PathBuf::from("/home/x"))];
        a.state.pinned = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        // No disks -> that section is empty and must be skipped.
        a.bookmark_sel = 0; // sitting on the (only) Bookmarks row
        a.apply(Action::SidebarNextSection);
        assert_eq!(a.bookmark_sel, 1, "jumps to first Pinned row");
        a.apply(Action::SidebarNextSection);
        assert_eq!(a.bookmark_sel, 0, "wraps back to Bookmarks, skipping empty Disks");
    }

    #[test]
    fn m_in_bookmarks_pane_removes_selected_pin() {
        let mut a = app();
        a.state.pinned.push(PathBuf::from("/a"));
        a.state.pinned.push(PathBuf::from("/b"));
        a.state.pinned.push(PathBuf::from("/c"));
        a.focus = Pane::Bookmarks;
        a.bookmark_sel = 1;
        a.apply(Action::TogglePin);
        assert_eq!(
            a.state.pinned,
            vec![PathBuf::from("/a"), PathBuf::from("/c")]
        );
        assert_eq!(a.bookmark_sel, 1, "stays in range");
        assert_eq!(a.focus, Pane::Bookmarks, "focus stays on bookmarks");
        // Remove the last entry — cursor clamps to new last.
        a.bookmark_sel = 1;
        a.apply(Action::TogglePin);
        assert_eq!(a.state.pinned, vec![PathBuf::from("/a")]);
        assert_eq!(a.bookmark_sel, 0);
        // Remove the final entry — cursor stays at 0.
        a.apply(Action::TogglePin);
        assert!(a.state.pinned.is_empty());
        assert_eq!(a.bookmark_sel, 0);
    }

    #[test]
    fn m_on_well_known_row_is_not_pinnable() {
        let mut a = app();
        a.well_known = vec![("Home".to_string(), PathBuf::from("/home/x"))];
        a.focus = Pane::Bookmarks;
        a.bookmark_sel = 0;
        a.apply(Action::TogglePin);
        assert!(a.state.pinned.is_empty());
        assert_eq!(a.status, "not pinnable");
    }

    #[test]
    fn focus_noop_when_bookmarks_hidden() {
        let mut a = app();
        a.config.panes.bookmarks.visible = false;
        assert_eq!(a.focus, Pane::Tree);
        a.apply(Action::FocusNext);
        assert_eq!(a.focus, Pane::Tree, "no other pane to cycle to");
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

    #[test]
    fn toggle_mark_inserts_then_removes() {
        let mut a = app();
        let p = a.arena.nodes[a.visible[a.selected]].path.clone();
        a.toggle_mark();
        assert!(a.marks.contains(&p));
        a.toggle_mark();
        assert!(!a.marks.contains(&p));
    }

    #[test]
    fn clear_marks_empties() {
        let mut a = app();
        a.toggle_mark();
        assert!(!a.marks.is_empty());
        a.clear_marks();
        assert!(a.marks.is_empty());
    }

    #[test]
    fn select_all_visible_marks_every_visible_path() {
        let mut a = app();
        a.select_all_visible();
        assert_eq!(a.marks.len(), a.visible.len());
        for &id in &a.visible {
            assert!(a.marks.contains(&a.arena.nodes[id].path));
        }
    }

    #[test]
    fn targets_returns_marks_when_nonempty_else_cursor() {
        let mut a = app();
        let cursor_path = a.arena.nodes[a.visible[a.selected]].path.clone();
        assert_eq!(a.targets(), vec![cursor_path.clone()]);
        a.toggle_mark();
        let t = a.targets();
        assert_eq!(t.len(), 1);
        assert!(t.contains(&cursor_path));
    }

    #[test]
    fn cancel_clears_marks() {
        let mut a = app();
        a.toggle_mark();
        assert!(!a.marks.is_empty());
        a.apply(Action::Cancel);
        assert!(a.marks.is_empty());
    }

    #[test]
    fn marks_survive_arena_rebuild_by_path() {
        let mut a = app();
        let p = a.arena.nodes[a.visible[a.selected]].path.clone();
        a.toggle_mark();
        let root_path = a.arena.nodes[a.arena.root].path.clone();
        a.arena = crate::tree::Arena::new(&root_path).unwrap();
        a.visible = a.arena.flatten();
        assert!(a.marks.contains(&p), "marks are keyed by path, survive rebuild");
    }

    #[test]
    fn toggle_visual_mode_marks_cursor_on_enter() {
        let mut a = app();
        assert!(!a.visual_mode);
        let p = a.arena.nodes[a.visible[a.selected]].path.clone();
        a.toggle_visual_mode();
        assert!(a.visual_mode);
        assert!(a.marks.contains(&p));
        assert_eq!(a.marks.len(), 1);
    }

    #[test]
    fn toggle_visual_mode_leaves_marks_on_exit() {
        let mut a = app();
        a.toggle_visual_mode();              // enter, marks 1
        a.apply(Action::Down);
        a.mark_cursor();                     // simulate post-dispatch hook
        assert_eq!(a.marks.len(), 2);
        a.toggle_visual_mode();              // exit
        assert!(!a.visual_mode);
        assert_eq!(a.marks.len(), 2, "marks must survive exit");
    }

    #[test]
    fn cancel_exits_visual_and_clears_marks() {
        let mut a = app();
        a.toggle_visual_mode();
        a.apply(Action::Down);
        a.mark_cursor();
        assert!(a.visual_mode && !a.marks.is_empty());
        a.apply(Action::Cancel);
        assert!(!a.visual_mode);
        assert!(a.marks.is_empty());
    }

    #[test]
    fn mark_cursor_is_idempotent() {
        let mut a = app();
        a.mark_cursor();
        a.mark_cursor();
        assert_eq!(a.marks.len(), 1);
    }

    #[test]
    fn mark_cursor_no_op_when_visible_empty() {
        let mut a = app();
        a.visible.clear();
        a.mark_cursor();
        assert!(a.marks.is_empty());
    }

    #[test]
    fn open_tab_enters_split_mode() {
        let mut a = app();
        assert!(!a.is_split());
        a.open_tab().unwrap();
        assert!(a.is_split());
        assert_eq!(a.tabs.len(), 1);
        // Active root unchanged; inactive snapshot holds the prior pane.
        assert_eq!(
            a.arena.nodes[a.arena.root].path,
            a.tabs[0].arena.nodes[a.tabs[0].arena.root].path
        );
    }

    #[test]
    fn open_tab_capped_at_two() {
        let mut a = app();
        a.open_tab().unwrap();
        a.open_tab().unwrap();
        assert_eq!(a.tabs.len(), 1, "second open is no-op");
    }

    #[test]
    fn close_tab_restores_inactive() {
        let mut a = app();
        // Drop a mark on the original pane so we can prove the close
        // brings it back instead of keeping the new pane's empty marks.
        a.toggle_mark();
        let original_marks = a.marks.clone();
        a.open_tab().unwrap();
        assert!(a.marks.is_empty(), "new tab starts with empty marks");
        a.close_tab();
        assert!(!a.is_split());
        assert_eq!(a.marks, original_marks);
    }

    #[test]
    fn close_tab_noop_when_single() {
        let mut a = app();
        a.toggle_mark();
        let before = a.marks.clone();
        a.close_tab(); // no-op
        assert!(!a.is_split());
        assert_eq!(a.marks, before);
    }

    #[test]
    fn swap_tab_swaps_active_and_inactive() {
        let mut a = app();
        a.toggle_mark();
        let marks_a = a.marks.clone();
        a.open_tab().unwrap();
        a.toggle_mark(); // mark cursor row in NEW pane
        let marks_b = a.marks.clone();
        // Sanity: B's marks differ from A's? They might share a path
        // since the root is identical. Use selected position instead.
        a.selected = 0;
        let active_sel_before = a.selected;
        let inactive_sel_before = a.tabs[0].selected;
        a.swap_tab();
        assert_eq!(a.selected, inactive_sel_before);
        assert_eq!(a.tabs[0].selected, active_sel_before);
        // marks moved with the swap
        assert_eq!(a.marks, marks_a);
        assert_eq!(a.tabs[0].marks, marks_b);
    }

    #[test]
    fn open_tab_sets_active_slot_to_right() {
        let mut a = app();
        assert_eq!(a.active_slot, 0, "single mode: only slot 0 exists");
        a.open_tab().unwrap();
        assert_eq!(a.active_slot, 1, "new tab is in the right slot");
    }

    #[test]
    fn swap_tab_toggles_active_slot() {
        let mut a = app();
        a.open_tab().unwrap();
        assert_eq!(a.active_slot, 1);
        a.swap_tab();
        assert_eq!(a.active_slot, 0, "swap moves focus to left slot");
        a.swap_tab();
        assert_eq!(a.active_slot, 1, "swap back");
    }

    #[test]
    fn close_tab_resets_active_slot() {
        let mut a = app();
        a.open_tab().unwrap();
        assert_eq!(a.active_slot, 1);
        a.close_tab();
        assert_eq!(a.active_slot, 0);
    }

    #[test]
    fn swap_tab_noop_when_single() {
        let mut a = app();
        a.toggle_mark();
        let before = a.marks.clone();
        a.swap_tab();
        assert_eq!(a.marks, before);
    }

    #[test]
    fn clipboard_shared_across_tabs() {
        let mut a = app();
        // Simulate a copy in pane A.
        a.clipboard = Some((vec![PathBuf::from("/a")], false));
        a.open_tab().unwrap();
        // Pane B sees the same clipboard.
        assert!(a.clipboard.is_some());
        assert_eq!(a.clipboard.as_ref().unwrap().0, vec![PathBuf::from("/a")]);
        a.swap_tab();
        assert!(a.clipboard.is_some(), "clipboard survives swap");
        a.close_tab();
        assert!(a.clipboard.is_some(), "clipboard survives close");
    }

    #[test]
    fn apply_sort_follows_selected_path() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("aaa"), vec![0u8; 10]).unwrap();
        std::fs::write(d.path().join("zzz"), vec![0u8; 99999]).unwrap();
        let mut app = test_app(d.path());
        // select "zzz" (last by name)
        app.selected = app.visible.len() - 1;
        let sel = app.visible[app.selected];
        let sel_path = app.arena.nodes[sel].path.clone();
        app.sort.mode = crate::sort::SortMode::Size; // zzz becomes... position changes
        app.sort.reverse = true;
        app.apply_sort();
        let now = app.arena.nodes[app.visible[app.selected]].path.clone();
        assert_eq!(now, sel_path, "cursor must follow the file, not the row index");
    }

    #[test]
    fn swap_tab_carries_filter_and_scroll_state() {
        let d = tempdir().unwrap();
        let mut app = test_app_with_tab(d.path());
        app.filter_query = "abc".into();
        app.preview_scroll = 7;
        app.mount_floor = Some(d.path().to_path_buf());
        app.swap_tab();
        assert!(app.filter_query.is_empty(), "filter must not leak into other tab");
        assert_eq!(app.preview_scroll, 0);
        assert!(app.mount_floor.is_none());
        app.swap_tab();
        assert_eq!(app.filter_query, "abc"); // round-trips back
        assert_eq!(app.preview_scroll, 7);
    }

    #[test]
    fn marks_bytes_sums_marked_files_only() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), vec![0u8; 100]).unwrap();
        std::fs::write(d.path().join("b.txt"), vec![0u8; 50]).unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let mut app = test_app(d.path());
        app.marks.insert(d.path().join("a.txt"));
        app.marks.insert(d.path().join("sub"));
        assert_eq!(app.marks_bytes(), 100);
    }

    #[test]
    fn live_filter_prunes_then_clears() {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.txt"), "1").unwrap();
        std::fs::write(d.path().join("apple.rs"), "1").unwrap();
        std::fs::write(d.path().join("banana.rs"), "1").unwrap();
        let arena = Arena::new(d.path()).unwrap();
        let mut a = App::new(arena, Config::with_defaults(), State::default());
        a.begin_filter();
        a.input.set("apple");
        a.live_filter();
        let names: Vec<String> = a.visible.iter()
            .map(|&id| a.arena.nodes[id].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "apple.rs"));
        assert!(!names.iter().any(|n| n == "banana.rs"), "non-match pruned");
        a.clear_filter();
        assert!(a.arena.filter.is_none());
        let names2: Vec<String> = a.visible.iter()
            .map(|&id| a.arena.nodes[id].path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names2.iter().any(|n| n == "banana.rs"), "restored after clear");
    }
}
