# SMB Network Storage — Design

**Date:** 2026-05-29
**Status:** Approved (pending final spec review)
**Extends:** `2026-05-21-devices-modal-design.md`

## Goal

Let the user add an SMB network share, persist it, auto-mount it in the
background, and browse it like a local directory — surfaced in the existing
devices modal the way Nautilus surfaces network shares.

## Approach

gio/GVFS performs the mount (identical mechanism to Nautilus; no root). Saved
shares persist in a dedicated config file. Credentials live in the system
keyring — gio owns them; explorenova stores zero passwords. Saved shares merge
into the existing `M` devices modal under a separate "Network" section.
Browsing reuses the existing arena over the gvfs FUSE mount path. Auto-mount
runs in a background thread so startup stays instant.

### Why gio/GVFS

- Exactly what Nautilus uses; matches the user's mental model.
- No root, no sudo, no fstab edits.
- Mounts to a real FUSE path under `$XDG_RUNTIME_DIR/gvfs/` that the existing
  `Arena` browses via `std::fs` with no special-casing.
- gio integrates with secret-service (GNOME keyring / KWallet) for credential
  storage, giving "remember password" and silent re-mount for free.

### Why keyring delegation

- No plaintext password ever written by explorenova.
- "Remember forever" via gio's own prompt persists creds in secret-service;
  subsequent mounts pull silently from the unlocked keyring (unlocked at login
  on a desktop session). True unattended auto-mount.
- Guest/anonymous shares need no credentials at all.

## Persistence

New file: `XDG_CONFIG_HOME/explorenova/shares.toml`
(fallback `~/.config/explorenova/shares.toml`).

A dedicated file — NOT `config.toml` — because the app rewrites it on every
add/delete, and re-serializing the user-authored `config.toml` would strip its
comments. `shares.toml` is app-managed but remains hand-editable.

```toml
[[share]]
name = "media"                       # display name; derived from share, editable
url  = "smb://user@nas.local/media"  # full gio URL; username embedded; NO password
```

Password is never stored here. Keyring only.

## New module: `src/smb.rs`

```rust
pub struct SmbShare {
    pub name: String,
    pub url: String,
}

pub struct Parsed {
    pub host: String,
    pub share: String,
    pub user: Option<String>,
}

/// Validate + decompose an smb:// URL. Rejects non-smb:// schemes and URLs
/// missing a host or share component.
pub fn parse_url(raw: &str) -> anyhow::Result<Parsed>;

/// Display name = last path component (the share name).
pub fn derive_name(p: &Parsed) -> String;

/// Load saved shares. Parse error => empty Vec (never panics).
pub fn load(path: &Path) -> Vec<SmbShare>;

/// Persist shares (TOML). Creates parent dir if missing.
pub fn save(path: &Path, shares: &[SmbShare]) -> anyhow::Result<()>;

/// Resolve the local FUSE mount path by scanning the gvfs dir for an entry
/// whose name contains both `server=<host>` and `share=<share>` tokens.
/// Robust against gvfs key ordering. The dir to scan is injectable so tests
/// can point at a tempdir.
pub fn mountpoint(host: &str, share: &str) -> Option<PathBuf>;
fn mountpoint_in(gvfs_dir: &Path, host: &str, share: &str) -> Option<PathBuf>; // testable core

/// Some(path) if currently mounted, else None.
pub fn is_mounted(share: &SmbShare) -> Option<PathBuf>;

/// Interactive mount: suspend the TUI via shell::around_terminal and run
/// `gio mount <url>` with inherited stdio. The user answers gio's own
/// password prompt on the real TTY and may choose "remember forever".
pub fn mount_interactive(url: &str) -> std::io::Result<()>;

/// Background mount: `gio mount <url>` with stdin nulled. Succeeds silently if
/// the keyring has creds; fails quietly otherwise (cannot prompt). Never
/// blocks the UI.
pub fn mount_background(url: &str);

/// `gio mount -u <url>`.
pub fn unmount(url: &str) -> anyhow::Result<()>;
```

Mountpoint resolution scans `$XDG_RUNTIME_DIR/gvfs/` (fallback
`/run/user/<getuid()>/gvfs/`) for a directory entry beginning `smb-share:`
whose name contains `server=<host>` and `share=<share>` (case-insensitive
token match). This avoids fragile assumptions about the order gvfs joins keys.

## Devices modal merge

Extend the existing `Device`:

```rust
pub enum DeviceKind {
    Block,                 // physical/udiskie device (default)
    Smb { url: String },   // saved network share
}

pub struct Device {
    // ...existing fields...
    pub kind: DeviceKind,
}
```

`devices::list()` stays block-only (udiskie). The `ShowDevices` action builds
the modal list as: block devices first, then a "Network" section of saved SMB
shares mapped into `Device` rows (`fstype = "smb"`, `mountpoint` = live result
of `smb::mountpoint()`, `kind = Smb { url }`). One unified list, one selection
model, existing render reused. SMB rows render with a distinct network glyph
and sit under a "Network" sub-header.

### Modal keybindings (additions)

| Key | Action |
|-----|--------|
| `a` | Add SMB share — opens single-line URL input |
| `d` | Delete selected saved share (SMB rows only; unmounts first) |
| `Enter` (SMB, mounted) | cd into mountpoint (reroot arena — existing path) |
| `Enter` (SMB, unmounted) | `mount_interactive`, then refresh |
| `u` / `r` / `j` / `k` | unchanged, now SMB-aware |

`d` on a block-device row is a no-op (status: "not a saved share").

## Add flow

`a` reuses the existing single-line input (`Mode::Input`) with a new pending
tag `AddSmb`. On Enter (submit):

1. `parse_url` validates. Bad URL → status error, abort (nothing saved).
2. Append `SmbShare { name: derive_name(..), url }`; `smb::save`.
3. `smb::mount_interactive(url)` — gio prompts for the password on the real
   TTY; the user picks "remember forever" → keyring.
4. Refresh modal (share now shows mounted).

No custom password widget — gio's own prompt is the password UI.

## Startup auto-mount (instant-startup safe)

`main.rs` loads `shares.toml`, then spawns one background thread that calls
`smb::mount_background(url)` for each saved share. Fire-and-forget; never
blocks the first draw (same model as the existing git-scan and fs-watch
threads). Unreachable server or locked keyring → quiet failure; the share
simply shows unmounted, and the user can `Enter` it to mount interactively.
The modal always reflects live status via `smb::mountpoint()` when opened.

## Error handling

- gio not installed → status "gvfs-smb not installed".
- Mount failure (host down / auth) → status message; share stays unmounted; no
  crash.
- Mountpoint unresolved after a reported-successful mount → error status.
- `shares.toml` parse error → start with empty list + status warning.
- Invalid URL at add time → reject before saving.

PKGBUILD `depends` gains `gvfs` and `gvfs-smb`.

## Testing

**Unit (no NAS / no gio required):**
- `parse_url`: accepts `smb://host/share`, `smb://user@host/share`; rejects
  non-smb scheme, missing host, missing share.
- `derive_name`: share name extraction.
- `shares.toml` save→load round-trip.
- `mountpoint_in`: crafted tempdir with gvfs-style entry names; matches the
  right entry regardless of key order; returns None when absent.
- Device merge: saved shares append as `DeviceKind::Smb` rows with correct
  fields and mount status.
- Bad-URL rejection at add.

**Manual (needs real NAS + gio — cannot unit-test):**
- Add a real share, enter password, "remember forever".
- Verify mount, browse, file open, unmount, delete.
- Restart app → background auto-mount succeeds silently from keyring.

## Files touched

- `src/smb.rs` — new module (struct, parse, load/save, mountpoint, mount/unmount).
- `src/devices.rs` — add `DeviceKind` enum + `kind` field.
- `src/dispatch.rs` — merge block+SMB into the modal list; modal keys `a`/`d`;
  SMB-aware `Enter`/`u`; `AddSmb` input submit handling.
- `src/app.rs` — hold `shares: Vec<SmbShare>` + shares-file path; `AddSmb`
  pending tag.
- `src/main.rs` — load shares; spawn background auto-mount thread.
- `src/ui/overlays.rs` — "Network" sub-header, SMB row glyph, footer hints
  (`a` add, `d` delete).
- `PKGBUILD` — add `gvfs` + `gvfs-smb` to `depends`.

## Out of scope (YAGNI)

- Per-share `automount` toggle (all saved shares auto-mount in background).
- Multi-field add form (single URL field only).
- Protocols other than SMB (NFS, FTP, sftp) — gio supports them, but not now.
- Editing a saved share in-place (delete + re-add).
- Plaintext credential fallback for headless/no-keyring environments.
