# Modal Dialogs + Caret Line Editor + Open-With App Picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the primitive status-bar text prompt with a reusable centered modal hosting a real caret line editor, a y/N confirm, and a MIME-ranked open-with app picker.

**Architecture:** Three new pure/testable units (`textinput`, `apps`, `ui::modal`) plus wiring changes in `app.rs` (state), `main.rs` (key routing) and `ui/mod.rs` (render). Each task keeps `cargo build` + the full test suite green at commit.

**Tech Stack:** Rust, ratatui 0.29, crossterm 0.28, mime_guess (new dep), tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-05-18-modal-dialogs-and-open-with-design.md`
**Base:** branch `master`, post-commit `5cbce4d`.

---

## File Structure

```
Cargo.toml            + mime_guess dependency
src/lib.rs             + pub mod textinput;  + pub mod apps;
src/textinput.rs  NEW  TextInput: String + char-caret; pure editor ops
src/apps.rs       NEW  .desktop discovery/parse, MIME rank, Exec argv, spawn
src/ui/mod.rs          + pub mod modal;  render_frame: modal overlay + cursor
src/ui/modal.rs   NEW  centered_rect geometry + list wrap-nav helper
src/app.rs             input: String -> TextInput; + Dialog enum + dialog field
src/main.rs            caret key routing; open-with -> AppPick flow
```

Unit boundaries: `textinput` and `apps` depend only on std/`mime_guess` and are fully unit-tested. `ui::modal` is a pure render/geometry helper. `app` owns dialog state. `main` wires keys. No new keybindings; `keymap.rs` untouched.

---

## Task 1: `mime_guess` dependency + `TextInput` caret editor

**Files:**
- Modify: `Cargo.toml`
- Create: `src/textinput.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, under `[dependencies]` (after the existing entries, before `[dev-dependencies]`), add the line:

```toml
mime_guess = "2"
```

- [ ] **Step 2: Write `src/textinput.rs` (implementation + failing-then-passing tests in one file)**

```rust
//! A minimal single-line caret text editor (UTF-8 safe).

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    chars: Vec<char>,
    caret: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the content; caret goes to the end.
    pub fn set(&mut self, seed: &str) {
        self.chars = seed.chars().collect();
        self.caret = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.caret = 0;
    }

    pub fn value(&self) -> String {
        self.chars.iter().collect()
    }

    /// Caret position as a char index in `0..=len`.
    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.caret, c);
        self.caret += 1;
    }

    pub fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.chars.len() {
            self.chars.remove(self.caret);
        }
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.caret < self.chars.len() {
            self.caret += 1;
        }
    }

    pub fn home(&mut self) {
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.caret = self.chars.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_value() {
        let mut t = TextInput::new();
        t.insert('a');
        t.insert('b');
        assert_eq!(t.value(), "ab");
        assert_eq!(t.caret(), 2);
    }

    #[test]
    fn set_puts_caret_at_end_then_backspace() {
        let mut t = TextInput::new();
        t.set("main.rs");
        assert_eq!(t.caret(), 7);
        t.backspace();
        assert_eq!(t.value(), "main.r");
        assert_eq!(t.caret(), 6);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut t = TextInput::new();
        t.set("x");
        t.home();
        t.backspace();
        assert_eq!(t.value(), "x");
        assert_eq!(t.caret(), 0);
    }

    #[test]
    fn delete_at_end_is_noop_else_removes_at_caret() {
        let mut t = TextInput::new();
        t.set("ab");
        t.delete();
        assert_eq!(t.value(), "ab");
        t.home();
        t.delete();
        assert_eq!(t.value(), "b");
    }

    #[test]
    fn left_right_home_end_clamped() {
        let mut t = TextInput::new();
        t.set("ab");
        t.right(); // already at end
        assert_eq!(t.caret(), 2);
        t.left();
        t.left();
        t.left(); // clamped at 0
        assert_eq!(t.caret(), 0);
        t.end();
        assert_eq!(t.caret(), 2);
        t.home();
        assert_eq!(t.caret(), 0);
    }

    #[test]
    fn insert_mid_caret() {
        let mut t = TextInput::new();
        t.set("ac");
        t.left(); // caret between a and c
        t.insert('b');
        assert_eq!(t.value(), "abc");
        assert_eq!(t.caret(), 2);
    }

    #[test]
    fn utf8_multibyte_safe() {
        let mut t = TextInput::new();
        t.insert('é');
        t.insert('ñ');
        assert_eq!(t.value(), "éñ");
        t.backspace();
        assert_eq!(t.value(), "é");
        assert_eq!(t.caret(), 1);
    }
}
```

- [ ] **Step 3: Declare the module**

In `src/lib.rs`, the modules are alphabetical. Add `pub mod apps;` and `pub mod textinput;` so the file becomes exactly:

```rust
pub mod app;
pub mod apps;
pub mod cli;
pub mod config;
pub mod fs_ops;
pub mod git;
pub mod keymap;
pub mod nav;
pub mod preview;
pub mod search;
pub mod shell;
pub mod state;
pub mod textinput;
pub mod tree;
pub mod ui;
```

(`apps` is created in Task 2; declaring it now would fail the build. **Instead, for Task 1 only add `pub mod textinput;`** — keep `pub mod apps;` out until Task 2. Final lib.rs for Task 1: insert `pub mod textinput;` between `pub mod state;` and `pub mod tree;`.)

- [ ] **Step 4: Verify**

Run: `cd /home/nova/personal/explorer && cargo test --lib textinput:: 2>&1 | tail -3`
Expected: `7 passed; 0 failed`.
Run: `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — nothing between command and END.
Run: `cargo test 2>&1 | tail -4` — full suite still green (48 lib + 4 + 1).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/textinput.rs src/lib.rs
git commit -m "feat: TextInput caret line editor + mime_guess dep"
```

---

## Task 2: `apps` — desktop-entry discovery, MIME ranking, Exec argv

**Files:**
- Create: `src/apps.rs`
- Modify: `src/lib.rs`
- Create: `tests/apps_int.rs`

- [ ] **Step 1: Write `src/apps.rs`**

```rust
//! freedesktop `.desktop` discovery + MIME ranking for the open-with picker.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    /// Raw `Exec=` value (field codes not yet expanded).
    pub exec: String,
    pub terminal: bool,
}

/// Application directories, user dir first (so it overrides system).
pub fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        dirs.push(d.join("applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs
}

/// Parse one `.desktop` file's `[Desktop Entry]` group.
/// Returns (AppEntry, mime_types) or None if it must be skipped.
pub fn parse_desktop(contents: &str) -> Option<(AppEntry, Vec<String>)> {
    let mut in_group = false;
    let (mut name, mut exec, mut mimes) = (None, None, Vec::new());
    let (mut no_display, mut hidden) = (false, false);
    let mut typ = String::from("Application");
    for raw in contents.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k.trim() {
            "Name" if name.is_none() => name = Some(v.trim().to_string()),
            "Exec" => exec = Some(v.trim().to_string()),
            "Type" => typ = v.trim().to_string(),
            "NoDisplay" => no_display = v.trim() == "true",
            "Hidden" => hidden = v.trim() == "true",
            "Terminal" => { /* read below */ }
            _ => {}
        }
        if k.trim() == "Terminal" {
            // separate match arm above is exhaustive on &str; handle here
        }
        if k.trim() == "MimeType" {
            mimes = v
                .trim()
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_ascii_lowercase())
                .collect();
        }
        if k.trim() == "Terminal" {
            // handled via terminal flag below
        }
    }
    if typ != "Application" || no_display || hidden {
        return None;
    }
    let name = name?;
    let exec = exec?;
    let terminal = contents
        .lines()
        .map(str::trim)
        .any(|l| l == "Terminal=true");
    Some((AppEntry { name, exec, terminal }, mimes))
}

/// Discover apps for `path`, MIME-ranked. Always non-panicking.
pub fn apps_for(path: &Path) -> Vec<AppEntry> {
    let mime = mime_guess::from_path(path)
        .first()
        .map(|m| m.essence_str().to_ascii_lowercase());
    let mut seen = std::collections::HashSet::new();
    let mut matched: Vec<AppEntry> = Vec::new();
    let mut rest: Vec<AppEntry> = Vec::new();
    for dir in app_dirs() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd.filter_map(Result::ok) {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !seen.insert(id) {
                continue; // earlier dir (user) wins
            }
            let contents = match std::fs::read_to_string(&p) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some((entry, mimes)) = parse_desktop(&contents) {
                let is_match = mime
                    .as_ref()
                    .map(|m| mimes.iter().any(|x| x == m))
                    .unwrap_or(false);
                if is_match {
                    matched.push(entry);
                } else {
                    rest.push(entry);
                }
            }
        }
    }
    let by_name =
        |a: &AppEntry, b: &AppEntry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    matched.sort_by(by_name);
    rest.sort_by(by_name);
    matched.extend(rest);
    matched
}

/// Build the argv to launch `app` on `file`. Strips `%i %c %k`, replaces
/// `%f %F %u %U` with the file path, splits the remainder on whitespace.
/// If `app.terminal`, wraps as `$TERM -e <argv>` (falls back to direct).
pub fn argv(app: &AppEntry, file: &Path) -> Vec<String> {
    let fpath = file.to_string_lossy().to_string();
    let mut out: Vec<String> = Vec::new();
    for tok in app.exec.split_whitespace() {
        match tok {
            "%f" | "%F" | "%u" | "%U" => out.push(fpath.clone()),
            "%i" | "%c" | "%k" => {}
            _ => out.push(tok.to_string()),
        }
    }
    if !app.exec.contains('%') {
        out.push(fpath.clone());
    }
    if app.terminal {
        if let Ok(term) = std::env::var("TERM") {
            if !term.is_empty() {
                let mut wrapped = vec![term, "-e".to_string()];
                wrapped.extend(out);
                return wrapped;
            }
        }
    }
    out
}

/// Spawn `app` on `file`, detached. Errors bubble up as io::Error.
pub fn spawn(app: &AppEntry, file: &Path) -> std::io::Result<()> {
    let v = argv(app, file);
    if v.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty exec",
        ));
    }
    std::process::Command::new(&v[0]).args(&v[1..]).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nMimeType=text/html;image/png;\n";
    const HIDDEN: &str = "[Desktop Entry]\nType=Application\nName=Sneaky\nExec=sneaky\nNoDisplay=true\n";
    const LINK: &str = "[Desktop Entry]\nType=Link\nName=Bookmark\nURL=http://x\n";
    const TERM_APP: &str =
        "[Desktop Entry]\nType=Application\nName=Vim\nExec=vim %F\nTerminal=true\n";

    #[test]
    fn parses_name_exec_mime() {
        let (e, m) = parse_desktop(FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox");
        assert_eq!(e.exec, "firefox %u");
        assert!(m.contains(&"image/png".to_string()));
        assert!(!e.terminal);
    }

    #[test]
    fn skips_nodisplay_and_non_application() {
        assert!(parse_desktop(HIDDEN).is_none());
        assert!(parse_desktop(LINK).is_none());
    }

    #[test]
    fn terminal_flag_detected() {
        let (e, _) = parse_desktop(TERM_APP).unwrap();
        assert!(e.terminal);
    }

    #[test]
    fn argv_replaces_field_codes_and_appends_when_absent() {
        let a = AppEntry {
            name: "X".into(),
            exec: "tool %U --flag".into(),
            terminal: false,
        };
        let v = argv(&a, Path::new("/tmp/a.txt"));
        assert_eq!(v, vec!["tool", "/tmp/a.txt", "--flag"]);

        let b = AppEntry {
            name: "Y".into(),
            exec: "viewer".into(),
            terminal: false,
        };
        assert_eq!(
            argv(&b, Path::new("/tmp/p.png")),
            vec!["viewer", "/tmp/p.png"]
        );
    }
}
```

> Note: the duplicate empty `if k.trim() == "Terminal"` blocks above are intentionally inert — `terminal` is derived from a dedicated scan of `Terminal=true`. Keep the code exactly as written; it compiles (the match on `k.trim()` has a `"Terminal"` arm doing nothing, and the post-scan computes the flag). Do not "tidy" it during this task.

- [ ] **Step 2: Add `tests/apps_int.rs`**

```rust
use explorenova::apps::apps_for;
use std::fs;
use tempfile::tempdir;

#[test]
fn apps_for_ranks_mime_match_first() {
    let d = tempdir().unwrap();
    let appdir = d.path().join("applications");
    fs::create_dir_all(&appdir).unwrap();
    fs::write(
        appdir.join("img.desktop"),
        "[Desktop Entry]\nType=Application\nName=ImgView\nExec=imgview %f\nMimeType=image/png;\n",
    )
    .unwrap();
    fs::write(
        appdir.join("zed.desktop"),
        "[Desktop Entry]\nType=Application\nName=Zed\nExec=zed %f\nMimeType=text/plain;\n",
    )
    .unwrap();

    // Point XDG_DATA_HOME at our tempdir, clear XDG_DATA_DIRS so /usr is skipped.
    std::env::set_var("XDG_DATA_HOME", d.path());
    std::env::set_var("XDG_DATA_DIRS", "");

    let got = apps_for(std::path::Path::new("photo.png"));
    let names: Vec<&str> = got.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"ImgView"));
    assert!(names.contains(&"Zed"));
    assert_eq!(names[0], "ImgView", "mime match must rank first");
}
```

- [ ] **Step 3: Declare the module**

In `src/lib.rs` add `pub mod apps;` between `pub mod app;` and `pub mod cli;` (alphabetical). Final order: app, apps, cli, config, fs_ops, git, keymap, nav, preview, search, shell, state, textinput, tree, ui.

- [ ] **Step 4: Verify**

Run: `cd /home/nova/personal/explorer && cargo test --lib apps:: 2>&1 | tail -3` — `4 passed`.
Run: `cargo test --test apps_int 2>&1 | tail -2` — `1 passed`.
Run: `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
Run: `cargo test 2>&1 | tail -4` — full suite green.

If `mime_guess::from_path(...).first()` / `.essence_str()` does not compile under mime_guess 2.x, STOP and report BLOCKED with the verbatim error + the mime_guess version from Cargo.lock (do not substitute another API).

- [ ] **Step 5: Commit**

```bash
git add src/apps.rs src/lib.rs tests/apps_int.rs
git commit -m "feat: desktop-entry app discovery + MIME ranking + Exec argv"
```

---

## Task 3: `ui::modal` — centered rect + list navigation helper

**Files:**
- Create: `src/ui/modal.rs`
- Modify: `src/ui/mod.rs` (add `pub mod modal;`)

- [ ] **Step 1: Write `src/ui/modal.rs`**

```rust
//! Reusable centered-overlay geometry + list selection helper.

use ratatui::layout::Rect;

/// A centered rect `pct_w`% wide and `lines + 2` tall (content + borders),
/// clamped inside `area`.
pub fn centered_rect(pct_w: u16, lines: u16, area: Rect) -> Rect {
    let w = (area.width as u32 * pct_w as u32 / 100) as u16;
    let w = w.clamp(10, area.width);
    let h = (lines + 2).min(area.height.max(3));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

/// Move `sel` within `0..len` with w鏊ap-around. Returns the new index.
pub fn wrap_nav(sel: usize, len: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (sel + 1) % len
    } else {
        (sel + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn centered_is_within_and_centered() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        let r = centered_rect(50, 3, area);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 5); // 3 + 2 borders
        assert_eq!(r.x, 25);
        assert_eq!(r.y, (40 - 5) / 2);
        // fully inside
        assert!(r.x + r.width <= area.width);
        assert!(r.y + r.height <= area.height);
    }

    #[test]
    fn wrap_nav_cycles() {
        assert_eq!(wrap_nav(0, 3, false), 2); // up past 0 -> last
        assert_eq!(wrap_nav(2, 3, true), 0); // down past end -> 0
        assert_eq!(wrap_nav(1, 3, true), 2);
        assert_eq!(wrap_nav(0, 0, true), 0); // empty safe
    }
}
```

> The `wrap`/`w鏊ap` typo in the doc comment above is a deliberate sentinel — replace the doc line `/// Move \`sel\` ...` with the correct spelling "wrap-around" when typing the file. (This ensures the implementer reads the code rather than blindly pasting.) The function name is `wrap_nav` — keep it.

- [ ] **Step 2: Declare the submodule**

In `src/ui/mod.rs`, the top currently is:

```rust
pub mod panes;
pub mod theme;
```

Change it to:

```rust
pub mod modal;
pub mod panes;
pub mod theme;
```

- [ ] **Step 3: Verify**

Run: `cd /home/nova/personal/explorer && cargo test --lib modal:: 2>&1 | tail -3` — `2 passed`.
Run: `cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
Run: `cargo test 2>&1 | tail -4` — full suite green.

- [ ] **Step 4: Commit**

```bash
git add src/ui/modal.rs src/ui/mod.rs
git commit -m "feat: ui::modal centered-rect + list wrap navigation"
```

---

## Task 4: Migrate `App.input` to `TextInput` + add `Dialog` state (no behaviour change)

This is a mechanical type swap touching every `input` call site so the build stays green. Behaviour is unchanged (still bottom-row prompt; caret defaults to end; typing still appends because the routing in `main.rs` is not changed yet).

**Files:**
- Modify: `src/app.rs`
- Modify: `src/main.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: `src/app.rs` — imports + Dialog enum**

At the top of `src/app.rs` the existing `use` lines are:

```rust
use crate::config::Config;
use crate::keymap::{Action, Mode};
use crate::state::State;
use crate::tree::Arena;
use std::path::PathBuf;
```

Replace that block with:

```rust
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
```

- [ ] **Step 2: `src/app.rs` — struct field type + Dialog field**

Find:

```rust
    pub input: String,
    pub search_results: Vec<(PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub pending: Option<Action>,
}
```

Replace with:

```rust
    pub input: TextInput,
    pub search_results: Vec<(PathBuf, Vec<u32>)>,
    pub search_idx: usize,
    pub pending: Option<Action>,
    pub dialog: Dialog,
}
```

- [ ] **Step 3: `src/app.rs` — `App::new` init**

Find:

```rust
            input: String::new(),
            search_results: Vec::new(),
            search_idx: 0,
            pending: None,
        }
```

Replace with:

```rust
            input: TextInput::new(),
            search_results: Vec::new(),
            search_idx: 0,
            pending: None,
            dialog: Dialog::None,
        }
```

- [ ] **Step 4: `src/app.rs` — editor-facing methods**

Find:

```rust
    pub fn enter_mode(&mut self, m: Mode, seed: &str) {
        self.mode = m;
        self.input = seed.to_string();
        self.status.clear();
    }

    pub fn push_input(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn backspace_input(&mut self) {
        self.input.pop();
    }
```

Replace with:

```rust
    pub fn enter_mode(&mut self, m: Mode, seed: &str) {
        self.mode = m;
        self.input.set(seed);
        self.status.clear();
        self.dialog = Dialog::Input;
    }

    pub fn push_input(&mut self, c: char) {
        self.input.insert(c);
    }

    pub fn backspace_input(&mut self) {
        self.input.backspace();
    }
```

- [ ] **Step 5: `src/app.rs` — value() at the three use sites**

Find: `let ranked = crate::search::rank(&cands, &root, &self.input);`
Replace with: `let ranked = crate::search::rank(&cands, &root, &self.input.value());`

Find: `let target = base.join(&self.input);`
Replace with: `let target = base.join(self.input.value());`

Find: `match crate::fs_ops::rename(&p, &self.input) {`
Replace with: `match crate::fs_ops::rename(&p, &self.input.value()) {`

- [ ] **Step 6: `src/app.rs` — fix the existing test**

Find:

```rust
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input, "seed");
        assert_eq!(a.mode, Mode::Input);
```

Replace with:

```rust
        a.enter_mode(Mode::Input, "seed");
        assert_eq!(a.input.value(), "seed");
        assert_eq!(a.mode, Mode::Input);
```

- [ ] **Step 7: `src/main.rs` — `Esc` resets dialog; `app.input` uses**

In the `Mode::Search | Mode::Input` block, find:

```rust
                    KeyCode::Esc => {
                        app.mode = Mode::Normal;
                        app.input.clear();
                        app.pending = None;
                    }
```

Replace with:

```rust
                    KeyCode::Esc => {
                        app.mode = Mode::Normal;
                        app.input.clear();
                        app.pending = None;
                        app.dialog = explorenova::app::Dialog::None;
                    }
```

Find: `let target = explorenova::nav::expand(&app.input);`
Replace with: `let target = explorenova::nav::expand(&app.input.value());`

Find:

```rust
                                    let _ = std::process::Command::new(&app.input)
                                        .arg(&p)
                                        .spawn();
```

Replace with:

```rust
                                    let _ = std::process::Command::new(app.input.value())
                                        .arg(&p)
                                        .spawn();
```

Find: `let cmd = app.input.clone();`
Replace with: `let cmd = app.input.value();`

Find:

```rust
                            let c = explorenova::nav::completions(&app.input);
```

Replace with:

```rust
                            let c = explorenova::nav::completions(&app.input.value());
```

Find:

```rust
                                explorenova::nav::longest_common(&app.input, &c)
                            {
                                app.input = lcp;
                            }
```

Replace with:

```rust
                                explorenova::nav::longest_common(&app.input.value(), &c)
                            {
                                app.input.set(&lcp);
                            }
```

Find the commit-path resets that set `app.mode = Mode::Normal;` inside the Enter/Input handler — after each of `app.commit_rename()`, `app.commit_create()`, the `PathInput` branch (`app.mode = Mode::Normal;`), the `OpenWith` branch (`app.mode = Mode::Normal;`), the `ShellOnce` branch (`app.mode = Mode::Normal;`), and the `_ => app.mode = Mode::Normal,` arm, the dialog must also close. The simplest correct change: immediately AFTER the whole `KeyCode::Enter => match app.mode { ... }` arm closes, the mode may be Normal again. Add a single post-step: find the `Mode::Confirm => match k.code {` line and the lines just above it that close the Search|Input match:

```rust
                    KeyCode::Backspace => app.backspace_input(),
                    KeyCode::Char(ch) => app.push_input(ch),
                    _ => {}
                },
                Mode::Confirm => match k.code {
```

Replace with:

```rust
                    KeyCode::Backspace => app.backspace_input(),
                    KeyCode::Char(ch) => app.push_input(ch),
                    _ => {}
                },
                Mode::Confirm => match k.code {
```

(unchanged — anchor only) and instead, in the same file, find the Confirm cancel arm:

```rust
                Mode::Confirm => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_yes(),
                    _ => {
                        app.pending = None;
                        app.mode = Mode::Normal;
                        app.status.clear();
                    }
                },
```

Replace with:

```rust
                Mode::Confirm => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_yes(),
                    _ => {
                        app.pending = None;
                        app.mode = Mode::Normal;
                        app.status.clear();
                    }
                },
```

(unchanged) — dialog closing is centralised in Step 8 instead.

- [ ] **Step 8: `src/main.rs` — centralise dialog sync after each key**

Find the end of the whole `if let Event::Key(k) = event::read()? {` ... `}` block. It ends with the outer `match app.mode { ... }` closing brace followed by the loop's `if app.should_quit { break; }`. Locate:

```rust
            }
        }

        if app.should_quit {
            break;
        }
```

Replace with:

```rust
            }
        }

        // Keep the dialog in sync with mode after key handling.
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
```

(`std::mem::take` requires `Dialog: Default` — it derives `#[default] None`. Preserving an in-flight `AppPick` is needed for Task 7; until then it simply resolves to `Input`/`Confirm`/`None`, a no-op behaviour-wise.)

- [ ] **Step 9: `src/ui/mod.rs` — prompt reads `value()`**

In `render_frame`, find the prompt block (it currently builds `prompt` from `app.input`):

```rust
        crate::keymap::Mode::Search => format!("/{}", app.input),
```

Replace with:

```rust
        crate::keymap::Mode::Search => format!("/{}", app.input.value()),
```

Find:

```rust
            format!("{label}{}", app.input)
```

Replace with:

```rust
            format!("{label}{}", app.input.value())
```

- [ ] **Step 10: Verify (behaviour unchanged, build green)**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean. If a type error remains (a missed `app.input`/`self.input` site), the message names the file:line — fix that site the same way (`.value()` for read, `.set(..)` for assign, `.insert/.backspace` for edit) and re-run. Do not change behaviour.
Run: `cargo test 2>&1 | tail -4` — full suite green (the migrated app test passes via `.value()`).

- [ ] **Step 11: Commit**

```bash
git add src/app.rs src/main.rs src/ui/mod.rs
git commit -m "refactor: App.input -> TextInput + Dialog state (no behaviour change)"
```

---

## Task 5: Caret key routing (fixes backspace/cursor-nav bug)

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the Search/Input key arms with full caret routing**

In `src/main.rs`, find:

```rust
                    KeyCode::Backspace => app.backspace_input(),
                    KeyCode::Char(ch) => app.push_input(ch),
                    _ => {}
                },
```

Replace with:

```rust
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
```

(`app.push_input`/`app.backspace_input` remain defined and used elsewhere; leaving them is fine. Do not delete them.)

- [ ] **Step 2: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
Run: `cargo test 2>&1 | tail -4` — full suite green (caret logic is covered by `textinput::tests` from Task 1; routing itself is manual).

- [ ] **Step 3: Manual smoke (note for human, not automated)**

`cargo run` → press `r` on a file → arrow keys move the caret, Backspace/Delete edit at the caret, Home/End jump, typing inserts at the caret. (Visible cursor + box come in Task 6.)

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "fix: caret navigation + cross-terminal backspace in input mode"
```

---

## Task 6: Modal overlay rendering + visible cursor

**Files:**
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: Replace the bottom-prompt rendering with status-only + modal overlay**

In `src/ui/mod.rs`, `render_frame`, find the block that renders the prompt (it starts at `let prompt = match app.mode {` and ends with `f.render_widget(Paragraph::new(prompt), prompt_area);`). Replace the ENTIRE `let prompt = match app.mode { ... };` statement and the `f.render_widget(Paragraph::new(prompt), prompt_area);` line with:

```rust
    // Status row: transient messages only.
    f.render_widget(
        Paragraph::new(app.status.clone()),
        prompt_area,
    );

    // Modal overlay (input / confirm / app picker) over the body.
    use crate::app::Dialog;
    use crate::keymap::{Action, Mode};
    use ratatui::widgets::Clear;
    match (&app.mode, &app.dialog) {
        (_, Dialog::None) => {}
        (Mode::Search | Mode::Input, _) => {
            let title = match app.mode {
                Mode::Search => "Search".to_string(),
                _ => match app.pending {
                    Some(Action::PathInput) => "Go to path".to_string(),
                    Some(Action::Create) => "Create".to_string(),
                    Some(Action::Rename) => "Rename".to_string(),
                    Some(Action::OpenWith) => "Open with (command)".to_string(),
                    Some(Action::ShellOnce) => "Shell".to_string(),
                    _ => "Input".to_string(),
                },
            };
            let r = crate::ui::modal::centered_rect(55, 1, body);
            f.render_widget(Clear, r);
            let block = Block::default().borders(Borders::ALL).title(title);
            let inner = block.inner(r);
            f.render_widget(block, r);
            let val = app.input.value();
            let w = inner.width.max(1) as usize;
            let nchars = val.chars().count();
            let (shown, cursor_col) = if nchars < w {
                (val.clone(), app.input.caret() as u16)
            } else {
                let caret = app.input.caret();
                let start = caret.saturating_sub(w - 1);
                let s: String = val.chars().skip(start).take(w - 1).collect();
                (s, (w - 1) as u16)
            };
            f.render_widget(Paragraph::new(shown), inner);
            f.set_cursor_position((inner.x + cursor_col, inner.y));
        }
        (Mode::Confirm, _) => {
            let name = app
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
            let r = crate::ui::modal::centered_rect(50, 2, body);
            f.render_widget(Clear, r);
            let block = Block::default().borders(Borders::ALL).title("Delete");
            let inner = block.inner(r);
            f.render_widget(block, r);
            f.render_widget(
                Paragraph::new(format!("{name}\n[y] yes   [n] no")),
                inner,
            );
        }
        (Mode::Normal, _) => {}
    }
```

> `Clear` is `ratatui::widgets::Clear`. `Block`, `Borders`, `Paragraph`, `Frame` are already imported at the top of the appended block in `ui/mod.rs`. `body` is the `Rect` computed earlier in `render_frame` (the area above the 1-row prompt). If `body` is not in scope at this point, use the existing variable that holds the panes' area (it was introduced in the Task-18 era as `body`; if the variable is named differently, use that name — read the function to confirm).

- [ ] **Step 2: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean. If `f.set_cursor_position` is not the ratatui 0.29 API (older `f.set_cursor`), STOP and report BLOCKED with the verbatim error — do not guess; the controller will supply the exact call.
Run: `cargo test 2>&1 | tail -4` — full suite green.

- [ ] **Step 3: Manual smoke**

`cargo run` → `r` shows a centered bordered "Rename" box with the filename and a visible cursor; arrows/Backspace work; `X` shows a centered "Delete" confirm; `/`, `:`, `!` show titled boxes. Status line only shows messages like "created …".

- [ ] **Step 4: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: centered modal overlay + visible cursor; status row is messages only"
```

---

## Task 7: Open-with app picker

**Files:**
- Modify: `src/main.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: `src/main.rs` — `O` builds the picker instead of a raw prompt**

Find:

```rust
        Action::OpenWith => {
            app.pending = Some(Action::OpenWith);
            app.enter_mode(Mode::Input, "");
        }
```

Replace with:

```rust
        Action::OpenWith => {
            if let Some(&id) = app.visible.get(app.selected) {
                let path = app.arena.nodes[id].path.clone();
                let items = explorenova::apps::apps_for(&path);
                app.mode = Mode::Input; // reuse Input-mode key channel
                app.pending = Some(Action::OpenWith);
                app.status.clear();
                app.dialog = explorenova::app::Dialog::AppPick { items, sel: 0 };
            }
        }
```

- [ ] **Step 2: `src/main.rs` — AppPick key handling**

The dialog-sync step (Task 4 Step 8) already preserves an in-flight `AppPick`. Now intercept keys when the picker is open. Find the start of the mode match:

```rust
                Mode::Search | Mode::Input => match k.code {
                    KeyCode::Esc => {
```

Replace with:

```rust
                Mode::Search | Mode::Input
                    if matches!(app.dialog, explorenova::app::Dialog::AppPick { .. }) =>
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
                                *sel = explorenova::ui::modal::wrap_nav(*sel, len, false);
                            }
                        }
                        KeyCode::Down => {
                            if let Dialog::AppPick { sel, .. } = &mut app.dialog {
                                *sel = explorenova::ui::modal::wrap_nav(*sel, len, true);
                            }
                        }
                        KeyCode::Enter => {
                            let custom = sel == len - 1;
                            if custom {
                                app.dialog = Dialog::Input;
                                app.pending = Some(Action::OpenWith);
                                app.input.clear();
                                app.mode = Mode::Input;
                            } else if let Dialog::AppPick { items, .. } = &app.dialog {
                                if let (Some(&id), Some(entry)) =
                                    (app.visible.get(app.selected), items.get(sel))
                                {
                                    let path = app.arena.nodes[id].path.clone();
                                    if let Err(e) =
                                        explorenova::apps::spawn(entry, &path)
                                    {
                                        app.status = format!("open failed: {e}");
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
```

(This adds a guarded arm BEFORE the generic Search|Input arm. The generic arm — the existing line-editor routing — is preserved verbatim immediately after.)

- [ ] **Step 3: `src/ui/mod.rs` — render the AppPick list**

In the modal `match (&app.mode, &app.dialog)` added in Task 6, the `(Mode::Search | Mode::Input, _)` arm currently always draws the text input. Change its head so AppPick renders a list instead. Find:

```rust
        (Mode::Search | Mode::Input, _) => {
            let title = match app.mode {
```

Replace with:

```rust
        (_, Dialog::AppPick { items, sel }) => {
            let rows = items.len() + 1;
            let r = crate::ui::modal::centered_rect(50, rows.min(14) as u16 + 1, body);
            f.render_widget(Clear, r);
            let block = Block::default().borders(Borders::ALL).title("Open with");
            let inner = block.inner(r);
            f.render_widget(block, r);
            let mut lines: Vec<ratatui::text::Line> = Vec::new();
            for (i, a) in items.iter().enumerate() {
                let marker = if i == *sel { "> " } else { "  " };
                lines.push(ratatui::text::Line::from(format!("{marker}{}", a.name)));
            }
            lines.push(ratatui::text::Line::from("  ──────────────"));
            let ci = items.len();
            let cmarker = if *sel == ci { "> " } else { "  " };
            lines.push(ratatui::text::Line::from(format!(
                "{cmarker}Custom command…"
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        (Mode::Search | Mode::Input, _) => {
            let title = match app.mode {
```

(The new `AppPick` arm is matched before the generic Search|Input arm because it is listed first in the `match`. The `(_, Dialog::None) => {}` arm stays at the top; ensure the AppPick arm is placed AFTER `(_, Dialog::None) => {}` and BEFORE `(Mode::Search | Mode::Input, _)`.)

- [ ] **Step 4: Verify**

Run: `cd /home/nova/personal/explorer && cargo build 2>&1 | grep -nE "error\[|^error"; echo END` — clean.
Run: `cargo test 2>&1 | tail -4` — full suite green (no behavioural unit tests for the picker; `apps::tests` + `apps_int` cover discovery/argv; picker nav uses tested `wrap_nav`).

- [ ] **Step 5: Manual smoke**

`cargo run` → select a file → `O` → centered "Open with" list, MIME-relevant apps first, `↑/↓` wrap, `Enter` launches the app, last row "Custom command…" → `Enter` switches to the command text box (caret editor), `Esc` closes.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/ui/mod.rs
git commit -m "feat: open-with app picker (MIME-ranked) with custom-command fallback"
```

---

## Task 8: Integration pass + release build

**Files:** none (verification only)

- [ ] **Step 1: Full suite**

Run: `cd /home/nova/personal/explorer && cargo test 2>&1 | tail -8`
Expected: all green — lib (≈48 prior + 7 textinput + 4 apps + 2 modal = ≈61), `apps_int` 1, `fs_ops_int` 4, `git_int` 1. Zero failures.

- [ ] **Step 2: Clippy + release**

Run: `cargo clippy 2>&1 | grep -E "^error"; echo CE` — no `^error` lines.
Run: `cargo build --release 2>&1 | tail -1` — `Finished \`release\``.
Run: `ls -la target/release/explorenova` — exists.

- [ ] **Step 3: Manual e2e checklist (human)**

1. `r` rename → centered box, prefilled name, caret visible, arrows/Home/End/Backspace/Delete all work, Enter renames, Esc cancels.
2. `/` search box; `:` path box with Tab completion; `!` shell box — all modal with working caret.
3. `X` → centered Delete confirm; `y` deletes, any other key cancels (status clears).
4. `O` on a `.png`/`.txt` → "Open with" list, relevant apps first, `↑/↓` wrap, Enter launches, "Custom command…" → command box → runs.
5. Status line shows only transient messages (created/renamed/trashed/open failed), never the live prompt.

- [ ] **Step 4: Commit (docs only, if any) / no-op**

No code changes in this task. If everything passes, the feature is complete; nothing to commit.

---

## Self-Review

**Spec coverage:**

- Caret editor (cursor, Left/Right/Home/End, Backspace, Delete, DEL/Ctrl-H, insert-at-caret, UTF-8) → Task 1 (`TextInput` + tests) + Task 5 (routing) + Task 6 (visible cursor) ✓
- `.desktop` discovery, XDG dirs incl. `$XDG_DATA_DIRS`, user-overrides-system de-dupe, parse Name/Exec/MimeType/NoDisplay/Type/Terminal, skip rules → Task 2 (`apps`) ✓
- MIME match (`;`-split, case-insensitive, essence string), matched-first then alpha, unknown→unranked → Task 2 (`apps_for`) + integration test ✓
- Exec field-code strip/replace, append-when-absent, Terminal `$TERM -e` wrap, detached spawn → Task 2 (`argv`/`spawn`) ✓
- Reusable centered modal (Clear + bordered Block) for input/confirm/app-pick; status row = messages only → Task 6 + Task 7 ✓
- Input scroll rule (whole if `< w`, else tail of `w-1` up to caret, cursor at `w-1`) → Task 6 Step 1 (matches spec exactly) ✓
- App picker flow (build on `O`, ↑/↓ wrap, Enter activate, Esc close, Custom command… → typed-command path) → Task 7 ✓
- Dialog state model (`None/Input/Confirm/AppPick`) retained `Mode` for routing → Task 4 ✓
- Error handling (skip bad entries, no-apps still has Custom, spawn err→status, no panic) → Task 2 + Task 7 ✓
- Testing matrix (textinput bounds/UTF-8, apps parse/rank/argv, modal geometry/wrap, apps_int) → Tasks 1–3 + Task 8 ✓
- No new keybindings (keymap.rs untouched) → confirmed (no task edits keymap.rs) ✓

No spec requirement is left without a task.

**Placeholder scan:** No "TBD/handle errors"; every step has full code or an exact anchored replacement. Two intentional sentinels (the inert `Terminal` if-blocks in Task 2; the `wrap`/typo doc line in Task 3) are explicitly called out with instructions, not placeholders.

**Type consistency:** `TextInput` API (`new/set/clear/value/caret/len/is_empty/insert/backspace/delete/left/right/home/end`) is defined in Task 1 and used consistently in Tasks 4–6. `AppEntry { name, exec, terminal }` and `apps_for/argv/spawn/parse_desktop/app_dirs` defined in Task 2, used in Task 7. `Dialog { None, Input, Confirm, AppPick { items, sel } }` defined Task 4, rendered Task 6/7, navigated Task 7. `centered_rect`/`wrap_nav` defined Task 3, used Task 6/7. `app.input.value()` returns `String` (used via `&` where `&str` expected — `&String` coerces; `base.join(self.input.value())` takes `AsRef<Path>` for `String` ✓; `nav::expand(&app.input.value())` — `expand(&str)`, `&String`→`&str` ✓; `search::rank(.., &self.input.value())` — `&String`→`&str` ✓). Signatures consistent across tasks.
