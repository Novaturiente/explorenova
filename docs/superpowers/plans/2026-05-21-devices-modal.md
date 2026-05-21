# Mounted Devices Modal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `M` key that opens a modal listing handleable mounted/unmounted devices (via `udiskie-info`), with inline mount, unmount, refresh, and cd-to-mountpoint operations.

**Architecture:** A new `devices` module parses `udiskie-info -a` output into `Device` structs and shells to `udiskie-mount`/`udiskie-umount`. A new `Dialog::Devices` variant + `Action::ShowDevices` plug into the existing modal-overlay pattern. UI renders a table inside `ui/mod.rs`. Reuses `ui::modal::{centered_rect, wrap_nav}`.

**Tech Stack:** Rust, anyhow, ratatui 0.29, std::process::Command (no new deps).

---

## File Map

| File | Change |
|---|---|
| `src/devices.rs` (NEW) | `Device` struct; `list()` parses `udiskie-info -a` output; `mount()`/`unmount()` shell out. |
| `src/lib.rs` | `pub mod devices;` |
| `src/keymap.rs` | `Action::ShowDevices` variant + `from_name` entry + default chord `"M"`. |
| `src/app.rs` | Add `Devices { items, sel }` variant to `Dialog` enum. |
| `src/main.rs` | `Action::ShowDevices` arm in `handle()`; new `Mode::Input` modal-dispatch branch when `Dialog::Devices`; preservation of `Dialog::Devices` in the sync block. |
| `src/ui/mod.rs` | New `(_, Dialog::Devices)` arm in the modal overlay match. |

---

## Task 1: `src/devices.rs` — Device struct + parser + shell helpers

**Files:**
- Create: `src/devices.rs`
- Modify: `src/lib.rs` (add `pub mod devices;`)

- [ ] **Step 1: Write the failing tests**

Create `src/devices.rs` with the test module first:

```rust
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub device: String,
    pub label: Option<String>,
    pub size: Option<String>,
    pub fstype: Option<String>,
    pub mountpoint: Option<PathBuf>,
}

impl Device {
    pub fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }
}

/// Parse a single tab-separated line from `udiskie-info -a -o ...`.
/// Expected columns: device_presentation, is_mounted, mount_paths,
/// device_size, id_label, id_type. Empty columns become `None`.
/// Returns `None` if the line has fewer than 6 columns.
fn parse_line(line: &str) -> Option<Device> {
    let cols: Vec<&str> = line.split('\t').collect();
    if cols.len() < 6 { return None; }
    let device = cols[0].trim().to_string();
    if device.is_empty() { return None; }
    let mount_paths = cols[2].trim();
    let mountpoint = if mount_paths.is_empty() {
        None
    } else {
        // multi-mount: take the first path
        let first = mount_paths.split(',').next()?.trim();
        if first.is_empty() { None } else { Some(PathBuf::from(first)) }
    };
    let opt = |s: &str| {
        let s = s.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    };
    Some(Device {
        device,
        label: opt(cols[4]),
        size: opt(cols[3]),
        fstype: opt(cols[5]),
        mountpoint,
    })
}

pub fn list() -> anyhow::Result<Vec<Device>> {
    let out = Command::new("udiskie-info")
        .args([
            "-a",
            "-o",
            "{device_presentation}\t{is_mounted}\t{mount_paths}\t{device_size}\t{id_label}\t{id_type}",
        ])
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("udiskie-info failed: {}", err.trim());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().filter_map(parse_line).collect())
}

pub fn mount(device: &str) -> anyhow::Result<()> {
    let out = Command::new("udiskie-mount").arg(device).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}

pub fn unmount(device: &str) -> anyhow::Result<()> {
    let out = Command::new("udiskie-umount").arg(device).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_with_all_fields() {
        let line = "/dev/sdb1\tTrue\t/run/media/u/Backup\t32 GB\tBackup\text4";
        let d = parse_line(line).unwrap();
        assert_eq!(d.device, "/dev/sdb1");
        assert_eq!(d.size.as_deref(), Some("32 GB"));
        assert_eq!(d.label.as_deref(), Some("Backup"));
        assert_eq!(d.fstype.as_deref(), Some("ext4"));
        assert_eq!(
            d.mountpoint,
            Some(PathBuf::from("/run/media/u/Backup"))
        );
        assert!(d.is_mounted());
    }

    #[test]
    fn parse_line_with_empty_label_and_mountpoint() {
        let line = "/dev/sdc1\tFalse\t\t8 GB\t\tvfat";
        let d = parse_line(line).unwrap();
        assert_eq!(d.device, "/dev/sdc1");
        assert!(d.label.is_none());
        assert!(d.mountpoint.is_none());
        assert!(!d.is_mounted());
        assert_eq!(d.fstype.as_deref(), Some("vfat"));
    }

    #[test]
    fn parse_skips_malformed_lines() {
        assert!(parse_line("only\ttwo\tcols").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn mount_paths_takes_first_when_comma_separated() {
        let line = "/dev/sdd1\tTrue\t/mnt/a,/mnt/b\t1 GB\tdouble\text4";
        let d = parse_line(line).unwrap();
        assert_eq!(d.mountpoint, Some(PathBuf::from("/mnt/a")));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/lib.rs` — add `pub mod devices;` alongside the existing `pub mod …` lines.

- [ ] **Step 3: Run tests to verify pass**

Run: `cargo test --lib devices::tests 2>&1 | tail -15`
Expected: 4 tests pass.

Run: `cargo build --release 2>&1 | grep -E "warning|error" | head -10`
Expected: only the pre-existing `parse_chord is never used` warning.

- [ ] **Step 4: Commit**

```bash
git add src/devices.rs src/lib.rs
git commit -m "feat: devices module - udiskie-info parser + mount/umount"
```

---

## Task 2: `Action::ShowDevices` keybinding

**Files:**
- Modify: `src/keymap.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/keymap.rs`:

```rust
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
```

- [ ] **Step 2: Run test — confirm compile failure**

Run: `cargo test --lib keymap::tests::capital_m_resolves_to_show_devices 2>&1 | tail -10`

- [ ] **Step 3: Add `ShowDevices` variant**

In `pub enum Action { … }`, append:

```rust
    ShowDevices,
```

- [ ] **Step 4: Register name in `from_name`**

Add to the `match s` arm:

```rust
            "show_devices" => ShowDevices,
```

- [ ] **Step 5: Add default binding**

In `Keymap::defaults()`, append before `(Quit, …)`:

```rust
            (ShowDevices, vec!["M"]),
```

(`M` is uppercase — confirm via `grep -n '"M"' src/keymap.rs` that it's not already bound. If a collision is found, ABORT and report — the spec says `M` is free.)

- [ ] **Step 6: Verify pass**

Run: `cargo test --lib keymap::tests 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/keymap.rs
git commit -m "feat: Action::ShowDevices bound to M"
```

---

## Task 3: `Dialog::Devices` variant in App

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Add `Devices` variant**

In `pub enum Dialog { … }` (around the top of `src/app.rs`), add:

```rust
    Devices { items: Vec<crate::devices::Device>, sel: usize },
```

(after the `AppPick { … }` variant for grouping.)

- [ ] **Step 2: Verify it compiles**

Run: `cargo build --release 2>&1 | grep -E "warning|error" | head -10`
Expected: clean (only `parse_chord` pre-existing).

Run: `cargo test 2>&1 | grep "test result" | tail -5`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add src/app.rs
git commit -m "feat: Dialog::Devices variant"
```

---

## Task 4: `Action::ShowDevices` arm + modal dispatch in main.rs

**Files:**
- Modify: `src/main.rs`

**Context:** `handle()` already routes IO-bearing actions. We add a `ShowDevices` arm that calls `devices::list()`, populates the dialog, sets `mode = Mode::Input`. The existing modal-dispatch path in the event loop (the `Mode::Search | Mode::Input` arm when `Dialog::AppPick` matches) becomes a model for a new branch when `Dialog::Devices`. The `app.dialog = match app.mode { … }` sync block at the bottom of the event loop must preserve `Dialog::Devices` (just like it preserves `Dialog::AppPick`).

- [ ] **Step 1: Add `Action::ShowDevices` arm in `handle()`**

In `fn handle()`'s `match action` block, add (alongside `Action::ShellDrop` / `Action::Refresh`):

```rust
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
```

- [ ] **Step 2: Preserve `Dialog::Devices` in the sync block**

Find the block (around line 374 onwards):

```rust
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
```

Change the inner `Mode::Search | Mode::Input` arm to also preserve `Devices`:

```rust
            Mode::Search | Mode::Input => match std::mem::take(&mut app.dialog) {
                explorenova::app::Dialog::AppPick { items, sel } => {
                    explorenova::app::Dialog::AppPick { items, sel }
                }
                explorenova::app::Dialog::Devices { items, sel } => {
                    explorenova::app::Dialog::Devices { items, sel }
                }
                _ => explorenova::app::Dialog::Input,
            },
```

- [ ] **Step 3: Add modal-dispatch branch for `Dialog::Devices`**

Find the existing `Mode::Search | Mode::Input if matches!(app.dialog, Dialog::AppPick { .. })` branch (around line 215). Add a sibling branch BEFORE it (or AFTER — order doesn't matter since the guards are exclusive on dialog type):

```rust
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
                                    // mounted → cd then close
                                    if let Ok(Some(id)) =
                                        app.arena.reveal(&mp)
                                    {
                                        app.visible = app.arena.flatten();
                                        if let Some(i) = app
                                            .visible
                                            .iter()
                                            .position(|&n| n == id)
                                        {
                                            app.selected = i;
                                        }
                                    } else {
                                        app.status = format!(
                                            "cannot navigate to {}",
                                            mp.display()
                                        );
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
```

- [ ] **Step 4: Build + test**

Run: `cargo build --release 2>&1 | tail -10`
Run: `cargo test 2>&1 | grep "test result"`

Expected: clean build, all tests green.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: ShowDevices dispatch + Devices modal key handling"
```

---

## Task 5: Render the `Devices` modal in ui/mod.rs

**Files:**
- Modify: `src/ui/mod.rs`

**Context:** The modal overlay match block currently has arms for `Dialog::None`, `Dialog::AppPick`, and `Mode::Search | Mode::Input` (catch-all for Input). Add a `Dialog::Devices` arm between `AppPick` and the input arm.

- [ ] **Step 1: Inspect existing AppPick arm**

Run: `grep -n "Dialog::AppPick\|Dialog::Devices" src/ui/mod.rs`

- [ ] **Step 2: Add the `Devices` arm**

Insert this arm into the `match (&app.mode, &app.dialog) { … }` block, immediately after the `Dialog::AppPick` arm:

```rust
        (_, crate::app::Dialog::Devices { items, sel }) => {
            use ratatui::style::Style;
            use ratatui::text::{Line, Span};
            let rows = items.len().max(1);
            let r = crate::ui::modal::centered_rect(
                70,
                (rows.min(14) as u16) + 2,
                body,
            );
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title("Devices");
            let inner = block.inner(r);
            f.render_widget(block, r);
            let mut lines: Vec<Line> = Vec::new();
            if items.is_empty() {
                lines.push(Line::from("  no handleable devices"));
            } else {
                for (i, d) in items.iter().enumerate() {
                    let marker = if i == *sel { "> " } else { "  " };
                    let mounted = if d.is_mounted() { "●" } else { "○" };
                    let label = d.label.as_deref().unwrap_or("-");
                    let size = d.size.as_deref().unwrap_or("?");
                    let fs = d.fstype.as_deref().unwrap_or("");
                    let mp = d
                        .mountpoint
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(not mounted)".to_string());
                    lines.push(Line::from(format!(
                        "{marker}{mounted} {:<14} {:<8} {:<8} {:<14} {}",
                        d.device, size, fs, label, mp
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter cd/mount   u umount   r refresh   Esc close",
                Style::default().fg(theme.dim),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
```

- [ ] **Step 3: Build + clippy**

Run: `cargo build --release 2>&1 | tail -10`
Run: `cargo clippy --all-targets 2>&1 | grep -E "error|warning" | grep -v "trash_path\|PaneConfig\|parse_chord" | head`

Expected: clean (pre-existing warnings in `trash_path`/`PaneConfig`/`parse_chord` are out of scope).

Run: `cargo test 2>&1 | grep "test result" | tail -5`
Expected: all green.

- [ ] **Step 4: Manual smoke test**

```bash
cargo build --release
./target/release/explorenova
```

Verify:
- Press `M` — modal opens; expect either device list or "no handleable devices".
- `j`/`k` navigate; `>` marker moves.
- `Esc` closes.
- (If a USB stick is available) plug it in, press `M`, see it unmounted; `Enter` → mounts, footer shows "mounted /dev/...". `Enter` again → cd into it, modal closes, cursor is in the mountpoint.
- `u` on the mounted device → "unmounted /dev/...".
- `r` after hot-plug → list updates.

- [ ] **Step 5: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: render Devices modal (table with ●/○ + mountpoint + hints)"
```

---

## Task 6: Final whole-branch review

**Files:** none.

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1 | grep "test result"`
Expected: 98 total (94 pre-existing + 4 devices parser tests + 1 keymap test = 99 actually; recount: 84 lib → +4 +1 = 89 lib; 1 apps_int + 8 fs_ops_int + 1 git_int = 99 total).

- [ ] **Step 2: Clippy + release build**

Run: `cargo clippy --all-targets 2>&1 | grep -E "error|warning" | grep -v "trash_path\|PaneConfig\|parse_chord" | head`
Run: `cargo build --release 2>&1 | grep -E "warning|error" | head`

Expected: no new warnings.

- [ ] **Step 3: Inspect commits**

Run: `git log --oneline master..HEAD`
Expected: 5 feature commits (Tasks 1–5) + 1 spec + 1 plan.

- [ ] **Step 4: Hand off**

Tests + clippy + smoke all green → invoke `superpowers:finishing-a-development-branch`.
