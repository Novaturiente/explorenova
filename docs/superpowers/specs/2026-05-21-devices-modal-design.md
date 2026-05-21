# explorenova — Mounted Devices Modal

**Date:** 2026-05-21
**Status:** Approved (decisions locked via brainstorming)
**Builds on:** shipped preview-debounce + visual-mode on `master`.

## Problem

The user manages removable drives (USB sticks, SD cards, encrypted
volumes) via `udiskie`. To navigate into a freshly-plugged drive today
they must `:`-type the mountpoint or switch to a separate utility to
mount it first. A first-class "devices" view inside explorenova removes
that friction: list handleable devices, mount/unmount inline, jump the
cursor straight into the mountpoint.

## Decisions (locked)

- **Surface:** Modal popup (same overlay pattern as the Open-With
  picker), toggled with the **`M`** key.
- **Source:** `udiskie-info -a` — only "handleable" devices (USB, SD,
  external/removable). Excludes root, home, swap, tmpfs, etc. by design.
- **Operations (locked):** Navigate (Enter on mounted → cd to
  mountpoint, close modal), Mount (Enter on unmounted →
  `udiskie-mount`, refresh, stay in modal), Unmount (`u` key on mounted
  → `udiskie-umount`, refresh, stay in modal).
- **Refresh:** synchronous (`udiskie-info` runs in ~50 ms). An explicit
  `r` key re-runs the query while the modal is open.
- **Out of scope (YAGNI):** eject/power-off (`-d`), encrypted-volume
  passphrase prompts, fstab mounts, async polling, system-tray
  integration, drilling into block-device children (lsblk-style tree).

## Architecture

| Unit | Responsibility |
|---|---|
| `src/devices.rs` (NEW) | `pub struct Device { device, label, size, fstype, mountpoint }`. `pub fn list() -> Result<Vec<Device>>` shells out to `udiskie-info -a -o "<tab-template>"`, parses lines, returns empty Vec when there are no handleable devices. `pub fn mount(device: &str) -> Result<()>` and `pub fn unmount(device: &str) -> Result<()>` shell to `udiskie-mount` / `udiskie-umount`. Errors surface stderr text. |
| `src/lib.rs` | `pub mod devices;` |
| `src/keymap.rs` | New `Action::ShowDevices` (default chord `"M"`, name `"show_devices"`). |
| `src/app.rs` | Extend `pub enum Dialog` with `Devices { items: Vec<devices::Device>, sel: usize }`. (The `Mode` stays `Input` while open — same pattern as `AppPick`.) |
| `src/main.rs::handle()` | New `Action::ShowDevices` arm: calls `devices::list()`, populates `Dialog::Devices`, sets `mode = Mode::Input`. |
| `src/main.rs` modal dispatch | New `Mode::Input` branch when `Dialog::Devices`: keys `j`/`k`/`down`/`up` (nav), `Enter` (mount-or-cd), `u` (umount), `r` (refresh), `Esc`/`M` (close). |
| `src/ui/mod.rs` modal overlay | New `Dialog::Devices` arm — table-like list (label / size / fstype / mountpoint), `> ` marker on selected row, footer hint line `Enter cd/mount  u umount  r refresh  Esc close`. |

### Key types

```rust
// devices.rs
#[derive(Debug, Clone)]
pub struct Device {
    pub device: String,             // "/dev/sdb1"
    pub label: Option<String>,      // "Backup"
    pub size: Option<String>,       // "32 GB"
    pub fstype: Option<String>,     // "ext4"
    pub mountpoint: Option<std::path::PathBuf>, // None if unmounted
}

impl Device {
    pub fn is_mounted(&self) -> bool { self.mountpoint.is_some() }
}

pub fn list() -> anyhow::Result<Vec<Device>>;
pub fn mount(device: &str) -> anyhow::Result<()>;
pub fn unmount(device: &str) -> anyhow::Result<()>;
```

```rust
// app.rs Dialog
pub enum Dialog {
    None,
    Input,
    Confirm,
    AppPick { items: Vec<AppEntry>, sel: usize },
    Devices { items: Vec<Device>, sel: usize }, // NEW
}
```

### `udiskie-info` template

```
udiskie-info -a -o "{device_presentation}\t{is_mounted}\t{mount_paths}\t{device_size}\t{id_label}\t{id_type}"
```

Parser splits each line on `\t`, expects 6 columns. If a line has fewer
columns it is skipped (defensive). `is_mounted` is the literal string
`"True"`/`"False"`; `mount_paths` is a comma-separated list (we take
the first); empty values are returned as `""` by udiskie and become
`None` in the Device struct.

## Behaviours

### Open modal

`M` → `Action::ShowDevices` → `handle()`:

1. `let items = devices::list().unwrap_or_default();`
2. `app.dialog = Dialog::Devices { items, sel: 0 };`
3. `app.mode = Mode::Input;` (re-uses the existing AppPick-style modal
   dispatch path).

If `devices::list()` errors (e.g. `udiskie-info` not on PATH), set
`app.status = "udiskie not available"` and DO NOT open the dialog.

### In-modal keys

| Key | Action |
|---|---|
| `j` / `down` | sel = wrap_nav(sel, len, true) |
| `k` / `up` | sel = wrap_nav(sel, len, false) |
| `r` | re-run `devices::list()`, replace items, clamp sel |
| `Enter` | if `items[sel].is_mounted()` → cd to mountpoint via `arena.reveal(path)`, set `app.selected`, close modal. Else → `devices::mount(&items[sel].device)`, then re-run `list()`, clamp sel, stay in modal. |
| `u` | if `items[sel].is_mounted()` → `devices::unmount(&items[sel].device)`, refresh, stay. Else status "not mounted". |
| `M` / `Esc` | close modal (`dialog = None`, `mode = Normal`). |
| anything else | ignored |

On mount/umount success: `app.status = format!("mounted {} -> {}", dev, mp)` (or analogous for umount). On failure: `app.status =
format!("mount failed: {e}")`.

### Empty list

If `items.is_empty()`, the modal still opens with a "no handleable
devices" line. `Enter`/`u` are no-ops; `r`/`Esc`/`M` work as normal.

### Refresh after mount

After `devices::mount` succeeds the kernel may take 100-300 ms to
publish the mountpoint. We accept that the immediate `list()` may show
the device as "mounting…" or already mounted — for the first cut, just
re-run `list()` and let the user press `r` again if needed. (Add a 200
ms sleep before refresh if smoke testing shows the mountpoint
consistently missing.)

### Render

`render_frame` modal overlay (after the existing `AppPick` arm):

```rust
(_, Dialog::Devices { items, sel }) => {
    let rows = items.len().max(1);
    let r = centered_rect(70, (rows.min(14) as u16) + 2, body);
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
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
            let mp = d.mountpoint.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(not mounted)".to_string());
            lines.push(Line::from(format!(
                "{marker}{mounted} {:<10} {:<8} {:<8} {:<12} {}",
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

`mounted` glyph: `●` = mounted, `○` = unmounted (terminal-safe Unicode;
works in every modern terminal).

## Error Handling

- `udiskie-info` missing on PATH → `list()` returns `Err`; `handle()`
  catches and sets status, does not open modal.
- `udiskie-mount`/`udiskie-umount` non-zero exit → captured stderr is
  surfaced in `app.status`; modal stays open with the previous list.
- Mountpoint contains spaces / unicode → handled by `PathBuf` end-to-end
  (the udiskie-info column is the raw path).
- Multiple mountpoints (`mount_paths` returns `path1,path2`) → take the
  first; the rest are ignored (rare; usually one).
- Cursor jump fails (`arena.reveal` returns Err / None) → status "cannot
  navigate to <path>"; modal closes anyway (Enter consumed).

## Testing

- **Unit (`devices.rs`):**
  - `parse_line_with_all_fields` — feed a tab-separated 6-column string,
    assert Device fields populated.
  - `parse_line_with_empty_label_and_mountpoint` — `""` columns map to
    `None`.
  - `parse_skips_malformed_lines` — 3-column line skipped, valid line
    after it parsed.
  - `mount_paths_takes_first_when_comma_separated` — `/a,/b` → `/a`.
- **No integration tests** for `list()`/`mount()`/`unmount()` — they
  shell out to udiskie which isn't a stable test surface. Manual smoke
  covers them.
- **Unit (`keymap.rs`):**
  - `M_resolves_to_show_devices`.
- **Manual:**
  - With a USB stick plugged: `M` shows it; Enter mounts; jump (Enter
    again) opens it in tree.
  - `u` unmounts. `r` refreshes after a hot-plug.
  - With no removable media: modal shows "no handleable devices",
    closes on Esc.
  - With udiskie not installed (temp `PATH=/usr/bin:/bin` lacking
    /usr/sbin): `M` shows status "udiskie not available", no modal.

## Out of Scope (YAGNI)

- Eject / spin-down (`udiskie-umount -d`).
- Passphrase prompt for LUKS volumes (udiskie handles via its own
  prompt agent if running; without it the mount fails and we surface
  the error).
- Async refresh / dbus monitoring (re-run on `r`).
- `lsblk`-style tree of block-device children.
- Filtering / search inside the modal.
- Persistent mountpoint preference (always use udiskie default).
