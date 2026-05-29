# SMB Network Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add, persist, auto-mount, and browse SMB network shares from explorenova's devices modal, the way Nautilus does.

**Architecture:** A new `src/smb.rs` module owns share records, `shares.toml` persistence, gvfs mountpoint resolution, and `gio mount/-u` invocation. Mounting delegates to gio/GVFS (no root); credentials live in the system keyring (gio's "remember forever"), never in explorenova. Saved shares merge into the existing `Dialog::Devices` modal as `DeviceKind::Smb` rows under a "Network" sub-header. A background thread auto-mounts saved shares on startup so first draw stays instant.

**Tech Stack:** Rust, ratatui/crossterm TUI, `gio` CLI (gvfs-smb), `toml`/`serde`, `libc` (getuid), existing `shell::around_terminal` for TUI suspend.

**Spec:** `docs/superpowers/specs/2026-05-29-smb-network-storage-design.md`

---

## Conventions for the implementer

- This crate keeps source lines ~70 columns wide. Match the surrounding style; do NOT run `cargo fmt` across the repo (it has broken the narrow style before).
- Run tests with `cargo test` (workspace). Lint with `cargo clippy --all-targets -- -D warnings` — the codebase is kept at **zero** clippy warnings; keep it there.
- The crate is named `explorenova`; library modules are referenced as `crate::` inside `src/` and `explorenova::` from `src/main.rs`.
- `Device` derives `Debug, Clone, PartialEq, Eq` — any new field/enum must derive the same.
- Commit after each task with the message shown in its final step.

---

## File Structure

- **Create `src/smb.rs`** — share record type, URL parse/validate, `shares.toml` load/save, gvfs mountpoint resolution, mount/unmount command wrappers, share→Device row mapping. One responsibility: everything SMB-specific.
- **Modify `src/lib.rs`** — register `pub mod smb;`.
- **Modify `src/devices.rs`** — add `DeviceKind` enum + `kind` field on `Device`; default existing constructions to `DeviceKind::Block`.
- **Modify `src/shell.rs`** — expose `pub fn run_interactive(prog, args)` reusing the existing private `around_terminal` (TUI suspend/restore) so `smb` can run `gio mount` interactively without duplicating terminal teardown.
- **Modify `src/keymap.rs`** — add `Action::AddSmb` (pending sentinel only, no keybinding).
- **Modify `src/app.rs`** — add `shares: Vec<smb::SmbShare>` and `shares_path: PathBuf` fields, initialized empty in `App::new`.
- **Modify `src/dispatch.rs`** — `build_device_rows` helper; merge in `ShowDevices`; `a`/`d` keys + smb-aware `Enter`/`u` in `handle_devices_key`; `AddSmb` arm in `text_input_enter`.
- **Modify `src/main.rs`** — compute shares path, load shares into `app`, spawn background auto-mount thread.
- **Modify `src/ui/overlays.rs`** — "Network" sub-header, network glyph for smb rows, footer hints `a add  d delete`.
- **Modify `PKGBUILD`** — add `gvfs` and `gvfs-smb` to `depends`.

---

## Task 1: SMB share type + URL parse/validate + name derivation

**Files:**
- Create: `src/smb.rs`
- Modify: `src/lib.rs` (register module)

- [ ] **Step 1: Register the module**

In `src/lib.rs`, add the line in alphabetical position (between `shell` and `sort`):

```rust
pub mod shell;
pub mod smb;
pub mod sort;
```

- [ ] **Step 2: Write the failing tests**

Create `src/smb.rs` with only the types, a stub `parse_url`/`derive_name`, and tests:

```rust
use serde::{Deserialize, Serialize};

/// A persisted SMB share. Password is NEVER stored here — it lives in the
/// system keyring via gio. `url` carries any username (`smb://user@host/share`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmbShare {
    pub name: String,
    pub url: String,
}

/// Decomposed smb:// URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub host: String,
    pub share: String,
    pub user: Option<String>,
}

/// Validate and decompose an `smb://[user@]host/share[/...]` URL.
/// The share is the FIRST path segment after the host. Errors on a
/// non-`smb://` scheme, an empty host, or a missing share segment.
pub fn parse_url(raw: &str) -> anyhow::Result<Parsed> {
    unimplemented!()
}

/// Display name = the share segment.
pub fn derive_name(p: &Parsed) -> String {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_url() {
        let p = parse_url("smb://nas.local/media").unwrap();
        assert_eq!(p.host, "nas.local");
        assert_eq!(p.share, "media");
        assert_eq!(p.user, None);
    }

    #[test]
    fn parse_url_with_user() {
        let p = parse_url("smb://alice@nas.local/media").unwrap();
        assert_eq!(p.host, "nas.local");
        assert_eq!(p.share, "media");
        assert_eq!(p.user.as_deref(), Some("alice"));
    }

    #[test]
    fn parse_url_with_trailing_path_keeps_first_segment_as_share() {
        let p = parse_url("smb://nas.local/media/movies").unwrap();
        assert_eq!(p.share, "media");
    }

    #[test]
    fn parse_url_trims_trailing_slash() {
        let p = parse_url("smb://nas.local/media/").unwrap();
        assert_eq!(p.share, "media");
    }

    #[test]
    fn reject_non_smb_scheme() {
        assert!(parse_url("nfs://nas.local/media").is_err());
        assert!(parse_url("https://nas.local/media").is_err());
        assert!(parse_url("nas.local/media").is_err());
    }

    #[test]
    fn reject_missing_host() {
        assert!(parse_url("smb:///media").is_err());
    }

    #[test]
    fn reject_missing_share() {
        assert!(parse_url("smb://nas.local").is_err());
        assert!(parse_url("smb://nas.local/").is_err());
    }

    #[test]
    fn derive_name_is_share_segment() {
        let p = parse_url("smb://alice@nas.local/media").unwrap();
        assert_eq!(derive_name(&p), "media");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib smb::`
Expected: panics from `unimplemented!()` / compile of stubs — tests FAIL.

- [ ] **Step 4: Implement `parse_url` and `derive_name`**

Replace the two stub bodies:

```rust
pub fn parse_url(raw: &str) -> anyhow::Result<Parsed> {
    let rest = raw
        .strip_prefix("smb://")
        .ok_or_else(|| anyhow::anyhow!("URL must start with smb://"))?;
    // Split optional `user@` off the front of the authority.
    let (user, hostpart) = match rest.split_once('@') {
        Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h),
        _ => (None, rest),
    };
    // hostpart = "host/share/...". Host is up to the first '/'.
    let (host, tail) = match hostpart.split_once('/') {
        Some((h, t)) => (h, t),
        None => (hostpart, ""),
    };
    if host.is_empty() {
        anyhow::bail!("missing host in smb URL");
    }
    let share = tail.trim_matches('/').split('/').next().unwrap_or("");
    if share.is_empty() {
        anyhow::bail!("missing share name in smb URL");
    }
    Ok(Parsed {
        host: host.to_string(),
        share: share.to_string(),
        user,
    })
}

pub fn derive_name(p: &Parsed) -> String {
    p.share.clone()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib smb::`
Expected: all `smb::tests` PASS.

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

```bash
git add src/lib.rs src/smb.rs
git commit -m "feat(smb): share type + smb:// URL parse/validate"
```

---

## Task 2: shares.toml load/save

**Files:**
- Modify: `src/smb.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/smb.rs` `tests` module (and `use std::path::Path;` near the top of the file alongside other imports):

```rust
    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shares.toml");
        let shares = vec![
            SmbShare { name: "media".into(), url: "smb://nas/media".into() },
            SmbShare { name: "backup".into(), url: "smb://u@nas/backup".into() },
        ];
        save(&path, &shares).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded, shares);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        assert!(load(&path).is_empty());
    }

    #[test]
    fn load_garbage_is_empty_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shares.toml");
        std::fs::write(&path, "this = = not toml").unwrap();
        assert!(load(&path).is_empty());
    }

    #[test]
    fn save_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/sub/shares.toml");
        save(&path, &[SmbShare { name: "m".into(), url: "smb://h/m".into() }])
            .unwrap();
        assert!(path.exists());
    }
```

- [ ] **Step 2: Add the file-format struct + function stubs**

Add near the top of `src/smb.rs` (after the `SmbShare` definition):

```rust
use std::path::{Path, PathBuf};

/// On-disk wrapper so the file is `[[share]]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SharesFile {
    #[serde(default, rename = "share")]
    shares: Vec<SmbShare>,
}

/// Load saved shares. A missing or unparseable file yields an empty Vec —
/// never panics, so a corrupt file can't brick startup.
pub fn load(path: &Path) -> Vec<SmbShare> {
    unimplemented!()
}

/// Persist shares as TOML, creating the parent directory if needed.
pub fn save(path: &Path, shares: &[SmbShare]) -> anyhow::Result<()> {
    unimplemented!()
}
```

(If `use std::path::...` already exists from a later task, keep a single import line.)

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib smb::`
Expected: round-trip / load tests FAIL via `unimplemented!()`.

- [ ] **Step 4: Implement `load` and `save`**

```rust
pub fn load(path: &Path) -> Vec<SmbShare> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    toml::from_str::<SharesFile>(&s)
        .map(|f| f.shares)
        .unwrap_or_default()
}

pub fn save(path: &Path, shares: &[SmbShare]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = SharesFile { shares: shares.to_vec() };
    let body = toml::to_string_pretty(&f)?;
    std::fs::write(path, format!("# explorenova SMB shares\n{body}"))?;
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib smb::`
Expected: all PASS.

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy --all-targets -- -D warnings`

```bash
git add src/smb.rs
git commit -m "feat(smb): shares.toml load/save (corrupt file => empty)"
```

---

## Task 3: gvfs mountpoint resolution

**Files:**
- Modify: `src/smb.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    fn touch_dir(base: &Path, name: &str) {
        std::fs::create_dir_all(base.join(name)).unwrap();
    }

    #[test]
    fn mountpoint_in_matches_server_and_share_any_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=nas.local,share=media");
        let mp = mountpoint_in(g, "nas.local", "media").unwrap();
        assert_eq!(mp, g.join("smb-share:server=nas.local,share=media"));
    }

    #[test]
    fn mountpoint_in_matches_with_user_and_domain_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:domain=WG,server=nas.local,share=media,user=alice");
        let mp = mountpoint_in(g, "nas.local", "media");
        assert!(mp.is_some());
    }

    #[test]
    fn mountpoint_in_is_case_insensitive_on_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=NAS.LOCAL,share=Media");
        assert!(mountpoint_in(g, "nas.local", "media").is_some());
    }

    #[test]
    fn mountpoint_in_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=other,share=media");
        assert!(mountpoint_in(g, "nas.local", "media").is_none());
    }

    #[test]
    fn mountpoint_in_none_when_gvfs_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("does-not-exist");
        assert!(mountpoint_in(&g, "nas.local", "media").is_none());
    }
```

- [ ] **Step 2: Add stubs**

Add to `src/smb.rs`:

```rust
/// Directory gvfs FUSE-mounts live under: `$XDG_RUNTIME_DIR/gvfs`,
/// falling back to `/run/user/<uid>/gvfs`.
fn gvfs_dir() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(rt).join("gvfs");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/run/user/{uid}/gvfs"))
}

/// Scan `gvfs_dir` for an `smb-share:` entry whose name contains both
/// `server=<host>` and `share=<share>` (case-insensitive). Returns its
/// absolute path. Split out from [`mountpoint`] so tests can point at a
/// temp dir.
fn mountpoint_in(gvfs_dir: &Path, host: &str, share: &str) -> Option<PathBuf> {
    unimplemented!()
}

/// Resolve the live mount path for a host/share, or None if not mounted.
pub fn mountpoint(host: &str, share: &str) -> Option<PathBuf> {
    mountpoint_in(&gvfs_dir(), host, share)
}

/// Some(path) if the share is currently mounted.
pub fn is_mounted(share: &SmbShare) -> Option<PathBuf> {
    let p = parse_url(&share.url).ok()?;
    mountpoint(&p.host, &p.share)
}
```

Confirm `libc` is a dependency (it is — see `Cargo.toml`).

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib smb::`
Expected: `mountpoint_in_*` tests FAIL via `unimplemented!()`.

- [ ] **Step 4: Implement `mountpoint_in`**

```rust
fn mountpoint_in(gvfs_dir: &Path, host: &str, share: &str) -> Option<PathBuf> {
    let want_server = format!("server={}", host.to_lowercase());
    let want_share = format!("share={}", share.to_lowercase());
    let entries = std::fs::read_dir(gvfs_dir).ok()?;
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy().to_lowercase();
        if name.starts_with("smb-share:")
            && name.contains(&want_server)
            && name.contains(&want_share)
        {
            return Some(ent.path());
        }
    }
    None
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib smb::`
Expected: all PASS.

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy --all-targets -- -D warnings`

```bash
git add src/smb.rs
git commit -m "feat(smb): gvfs mountpoint resolution (order-robust token scan)"
```

---

## Task 4: mount / unmount command wrappers

**Files:**
- Modify: `src/shell.rs` (expose interactive runner)
- Modify: `src/smb.rs` (mount/unmount)

- [ ] **Step 1: Expose an interactive command runner in `src/shell.rs`**

Add after `run_once` (the private `around_terminal` already exists in this file):

```rust
/// Suspend the TUI, run `prog args` with inherited stdio (so the child can
/// prompt the user directly), wait for Enter, restore the TUI. Used for
/// `gio mount`, whose own auth prompt is the password UI.
pub fn run_interactive(prog: &str, args: &[&str]) -> io::Result<()> {
    around_terminal(|| Command::new(prog).args(args).status().map(|_| ()))
}
```

- [ ] **Step 2: Write the failing test (argv builders)**

The actual `gio` exec can't be unit-tested (needs a server). Test only the argv builders. Add to `src/smb.rs` tests:

```rust
    #[test]
    fn mount_argv_is_gio_mount_url() {
        assert_eq!(
            mount_argv("smb://nas/media"),
            vec!["mount".to_string(), "smb://nas/media".to_string()]
        );
    }

    #[test]
    fn unmount_argv_is_gio_mount_u_url() {
        assert_eq!(
            unmount_argv("smb://nas/media"),
            vec![
                "mount".to_string(),
                "-u".to_string(),
                "smb://nas/media".to_string()
            ]
        );
    }
```

- [ ] **Step 3: Add stubs**

Add to `src/smb.rs`:

```rust
use std::process::{Command, Stdio};

fn mount_argv(url: &str) -> Vec<String> {
    unimplemented!()
}

fn unmount_argv(url: &str) -> Vec<String> {
    unimplemented!()
}

/// Interactive mount: gio prompts for credentials on the real TTY (and can
/// "remember forever" into the keyring). Suspends the TUI for the duration.
pub fn mount_interactive(url: &str) -> std::io::Result<()> {
    let args = mount_argv(url);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::shell::run_interactive("gio", &refs)
}

/// Background mount: no TTY. Succeeds silently if the keyring already holds
/// the credentials; fails quietly otherwise (cannot prompt). Safe to call
/// from a worker thread; never touches the terminal.
pub fn mount_background(url: &str) {
    let _ = Command::new("gio")
        .args(mount_argv(url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Unmount via gio. Surfaces a clear error if gio is absent or the unmount
/// fails.
pub fn unmount(url: &str) -> anyhow::Result<()> {
    let out = Command::new("gio")
        .args(unmount_argv(url))
        .output()
        .map_err(|e| anyhow::anyhow!("gio not available: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test --lib smb::`
Expected: `mount_argv` / `unmount_argv` tests FAIL via `unimplemented!()`.

- [ ] **Step 5: Implement the argv builders**

```rust
fn mount_argv(url: &str) -> Vec<String> {
    vec!["mount".into(), url.into()]
}

fn unmount_argv(url: &str) -> Vec<String> {
    vec!["mount".into(), "-u".into(), url.into()]
}
```

- [ ] **Step 6: Run tests + clippy**

Run: `cargo test --lib smb::`
Expected: PASS.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/shell.rs src/smb.rs
git commit -m "feat(smb): gio mount/unmount wrappers (interactive + background)"
```

---

## Task 5: DeviceKind + share→Device row mapping

**Files:**
- Modify: `src/devices.rs` (add `DeviceKind` + `kind` field)
- Modify: `src/smb.rs` (`share_to_row`, `to_device_rows`)

- [ ] **Step 1: Add `DeviceKind` to `src/devices.rs`**

At the top of `src/devices.rs`, add the enum and a `kind` field. Replace the existing `Device` struct definition (currently lines 4-17):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    /// Physical/removable block device discovered via udiskie.
    Block,
    /// Saved SMB network share. `url` is the gio mount URL.
    Smb { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub device: String,
    pub label: Option<String>,
    pub size: Option<String>,
    pub fstype: Option<String>,
    pub mountpoint: Option<PathBuf>,
    pub kind: DeviceKind,
}

impl Device {
    pub fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }
    pub fn is_smb(&self) -> bool {
        matches!(self.kind, DeviceKind::Smb { .. })
    }
}
```

- [ ] **Step 2: Set `kind: DeviceKind::Block` on the udiskie parser**

In `parse_line` (the `Some(Device { ... })` near the end), add the field:

```rust
    Some(Device {
        device,
        label: opt(cols[4]),
        size: opt(cols[3]),
        fstype: opt(cols[5]),
        mountpoint,
        kind: DeviceKind::Block,
    })
```

- [ ] **Step 3: Fix the existing devices tests to construct `kind`**

The `parse_line_*` tests build `Device` via `parse_line` (no change needed). No test constructs `Device` literally, so none break. Run to confirm:

Run: `cargo test --lib devices::`
Expected: existing devices tests PASS (build now compiles with the new field because `parse_line` sets it).

- [ ] **Step 4: Write the failing test for `share_to_row` in `src/smb.rs`**

Add to `src/smb.rs` tests:

```rust
    use crate::devices::DeviceKind;

    #[test]
    fn share_to_row_unmounted() {
        let s = SmbShare { name: "media".into(), url: "smb://nas/media".into() };
        let row = share_to_row(&s, None);
        assert_eq!(row.device, "media");
        assert_eq!(row.fstype.as_deref(), Some("smb"));
        assert_eq!(row.label.as_deref(), Some("smb://nas/media"));
        assert_eq!(row.mountpoint, None);
        assert!(!row.is_mounted());
        assert!(row.is_smb());
        assert_eq!(row.kind, DeviceKind::Smb { url: "smb://nas/media".into() });
    }

    #[test]
    fn share_to_row_mounted() {
        let s = SmbShare { name: "media".into(), url: "smb://nas/media".into() };
        let mp = std::path::PathBuf::from("/run/user/1000/gvfs/smb-share:server=nas,share=media");
        let row = share_to_row(&s, Some(mp.clone()));
        assert_eq!(row.mountpoint, Some(mp));
        assert!(row.is_mounted());
    }
```

- [ ] **Step 5: Add stubs to `src/smb.rs`**

```rust
use crate::devices::{Device, DeviceKind};

/// Map a saved share + its (optional) live mount path to a devices-modal row.
/// Pure — `mountpoint` is injected so this is unit-testable.
pub fn share_to_row(share: &SmbShare, mountpoint: Option<PathBuf>) -> Device {
    unimplemented!()
}

/// Build a devices-modal row per saved share, resolving live mount status.
pub fn to_device_rows(shares: &[SmbShare]) -> Vec<Device> {
    shares
        .iter()
        .map(|s| share_to_row(s, is_mounted(s)))
        .collect()
}
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `cargo test --lib smb::`
Expected: `share_to_row_*` FAIL via `unimplemented!()`.

- [ ] **Step 7: Implement `share_to_row`**

```rust
pub fn share_to_row(share: &SmbShare, mountpoint: Option<PathBuf>) -> Device {
    Device {
        device: share.name.clone(),
        label: Some(share.url.clone()),
        size: None,
        fstype: Some("smb".to_string()),
        mountpoint,
        kind: DeviceKind::Smb { url: share.url.clone() },
    }
}
```

- [ ] **Step 8: Run tests + clippy**

Run: `cargo test --lib`
Expected: all PASS.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add src/devices.rs src/smb.rs
git commit -m "feat(smb): DeviceKind + share->Device row mapping"
```

---

## Task 6: App fields + AddSmb action

**Files:**
- Modify: `src/keymap.rs` (add `Action::AddSmb`)
- Modify: `src/app.rs` (add `shares`, `shares_path` fields + init)

- [ ] **Step 1: Add the `AddSmb` action variant**

In `src/keymap.rs`, add to the `Action` enum (after `ShowDevices` on line 54):

```rust
    ShowDevices,
    /// Pending sentinel for the SMB-add text input. No keybinding —
    /// triggered from inside the devices modal.
    AddSmb,
```

Do NOT add it to `from_name` (it has no config-bindable name).

- [ ] **Step 2: Add fields to `App` in `src/app.rs`**

In the `pub struct App { ... }` block, add (after `search_cands` on line 106):

```rust
    pub search_cands: Option<Vec<PathBuf>>,
    /// Saved SMB shares (loaded from shares.toml at startup, mutated on
    /// add/delete in the devices modal).
    pub shares: Vec<crate::smb::SmbShare>,
    /// Path to shares.toml; written on every add/delete.
    pub shares_path: PathBuf,
```

- [ ] **Step 3: Initialize the fields in `App::new`**

Find the `App { ... }` construction inside `App::new` and add the two fields (init empty — `main.rs` overwrites them after construction, mirroring how `state.apply_to` is applied post-construction):

```rust
            search_cands: None,
            shares: Vec::new(),
            shares_path: PathBuf::new(),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles (PathBuf already imported in app.rs — line 8).

- [ ] **Step 5: Run full test suite + clippy**

Run: `cargo test`
Expected: all PASS.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/keymap.rs src/app.rs
git commit -m "feat(smb): App.shares state + AddSmb action sentinel"
```

---

## Task 7: dispatch wiring (modal merge, a/d keys, smb-aware enter/unmount, add submit)

**Files:**
- Modify: `src/dispatch.rs`

- [ ] **Step 1: Add a `build_device_rows` helper**

Add near `refresh_devices` (around `src/dispatch.rs:296`):

```rust
/// Build the full devices-modal row list: physical block devices first,
/// then saved SMB shares with live mount status.
fn build_device_rows(app: &App) -> Vec<crate::devices::Device> {
    let mut items = crate::devices::list().unwrap_or_default();
    items.extend(crate::smb::to_device_rows(&app.shares));
    items
}
```

- [ ] **Step 2: Use it in `ShowDevices`**

Replace the `Action::ShowDevices` arm (`src/dispatch.rs:585-591`) with:

```rust
        Action::ShowDevices => {
            let items = build_device_rows(app);
            app.dialog = Dialog::Devices { items, sel: 0 };
            app.mode = Mode::Input;
        }
```

(This drops the previous `Err` arm; `devices::list` failures now degrade to block-less list rather than refusing to open — SMB shares still show. `unwrap_or_default` handles it.)

- [ ] **Step 3: Use it in `refresh_devices`**

Replace the body of `refresh_devices` (`src/dispatch.rs:296-309`) with:

```rust
fn refresh_devices(app: &mut App) {
    let items = build_device_rows(app);
    let new_len = items.len();
    if let Dialog::Devices { items: it, sel } = &mut app.dialog {
        *it = items;
        if *sel >= new_len {
            *sel = new_len.saturating_sub(1);
        }
    }
}
```

- [ ] **Step 4: Add `a` and `d` keys to `handle_devices_key`**

In `handle_devices_key` (`src/dispatch.rs:282-293`), add two arms before the `_ => {}`:

```rust
        KeyCode::Char('r') => refresh_devices(app),
        KeyCode::Char('u') => devices_unmount_selected(app),
        KeyCode::Char('a') => devices_begin_add(app),
        KeyCode::Char('d') => devices_delete_selected(app),
        KeyCode::Enter => devices_enter_selected(app),
        _ => {}
```

- [ ] **Step 5: Implement `devices_begin_add`**

Add after `handle_devices_key`. This mirrors the AppPick "custom command" branch (`src/dispatch.rs:407-412`): it must set `Dialog::Input` explicitly, because main.rs's dialog-sync would otherwise keep `Dialog::Devices` while `mode == Input`.

```rust
fn devices_begin_add(app: &mut App) {
    app.input.clear();
    app.pending = Some(Action::AddSmb);
    app.mode = Mode::Input;
    app.dialog = Dialog::Input;
    app.set_status("smb url (e.g. smb://user@host/share)");
}
```

- [ ] **Step 6: Implement `devices_delete_selected`**

```rust
fn devices_delete_selected(app: &mut App) {
    let dev = match &app.dialog {
        Dialog::Devices { items, sel } => items.get(*sel).cloned(),
        _ => None,
    };
    let Some(d) = dev else { return };
    let DeviceKind::Smb { url } = &d.kind else {
        app.set_status("not a saved share");
        return;
    };
    // Best-effort unmount before forgetting it.
    if d.is_mounted() {
        let _ = crate::smb::unmount(url);
    }
    app.shares.retain(|s| s.url != *url);
    if let Err(e) = crate::smb::save(&app.shares_path, &app.shares) {
        app.set_status(format!("save failed: {e}"));
    } else {
        app.set_status(format!("removed {}", d.device));
    }
    refresh_devices(app);
}
```

Add the import for `DeviceKind` at the top of `src/dispatch.rs` (where other `crate::` types are used; if `use` lines exist, add it, otherwise reference it fully-qualified as `crate::devices::DeviceKind`). To keep it simple and match the file's style, reference it fully qualified:

```rust
    let crate::devices::DeviceKind::Smb { url } = &d.kind else {
        app.set_status("not a saved share");
        return;
    };
```

- [ ] **Step 7: Make `devices_unmount_selected` smb-aware**

Replace the unmount call section in `devices_unmount_selected` (`src/dispatch.rs:321-324`) so SMB rows use `smb::unmount`:

```rust
    let res = match &d.kind {
        crate::devices::DeviceKind::Smb { url } => crate::smb::unmount(url),
        crate::devices::DeviceKind::Block => crate::devices::unmount(&d.device),
    };
    match res {
        Ok(_) => app.set_status(format!("unmounted {}", d.device)),
        Err(e) => app.set_status(format!("umount failed: {e}")),
    }
```

- [ ] **Step 8: Make `devices_enter_selected` smb-aware**

In `devices_enter_selected` (`src/dispatch.rs:328-368`), the unmounted branch currently calls `crate::devices::mount`. Replace the final `else { ... }` block (the "unmounted → mount + refresh" part, `src/dispatch.rs:360-367`) with an smb-aware version:

```rust
    } else {
        // unmounted → mount + refresh, stay open
        let res = match &d.kind {
            crate::devices::DeviceKind::Smb { url } => {
                crate::smb::mount_interactive(url)
                    .map_err(|e| anyhow::anyhow!("{e}"))
            }
            crate::devices::DeviceKind::Block => crate::devices::mount(&d.device),
        };
        match res {
            Ok(_) => app.set_status(format!("mounted {}", d.device)),
            Err(e) => app.set_status(format!("mount failed: {e}")),
        }
        refresh_devices(app);
    }
```

(`anyhow` is already used throughout dispatch via `Result`; confirm `use anyhow::Result;` or fully-qualified `anyhow::anyhow!` works — it does, anyhow is a dependency.)

- [ ] **Step 9: Handle the `AddSmb` submit in `text_input_enter`**

In `text_input_enter` (`src/dispatch.rs:493`), add an arm to the `Mode::Input => match app.pending.take()` block, before the `_ => app.mode = Mode::Normal,` fallback:

```rust
            Some(Action::AddSmb) => {
                let raw = app.input.value();
                match crate::smb::parse_url(&raw) {
                    Ok(p) => {
                        let name = crate::smb::derive_name(&p);
                        app.shares.push(crate::smb::SmbShare {
                            name,
                            url: raw.clone(),
                        });
                        if let Err(e) =
                            crate::smb::save(&app.shares_path, &app.shares)
                        {
                            app.set_status(format!("save failed: {e}"));
                        }
                        // Interactive mount: gio prompts for the password on
                        // the real TTY and can remember it in the keyring.
                        if let Err(e) = crate::smb::mount_interactive(&raw) {
                            app.set_status(format!("mount failed: {e}"));
                        } else {
                            app.set_status(format!("added {}", raw));
                        }
                        // Reopen the devices modal with the new share.
                        let items = build_device_rows(app);
                        app.dialog = Dialog::Devices { items, sel: 0 };
                        app.mode = Mode::Input;
                    }
                    Err(e) => {
                        app.set_status(format!("bad smb url: {e}"));
                        app.mode = Mode::Normal;
                    }
                }
            }
```

- [ ] **Step 10: Build, test, clippy**

Run: `cargo build`
Expected: compiles.
Run: `cargo test`
Expected: all PASS.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 11: Commit**

```bash
git add src/dispatch.rs
git commit -m "feat(smb): devices modal add/delete + smb-aware mount/enter/unmount"
```

---

## Task 8: main.rs — load shares + background auto-mount

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add a `shares_path` helper**

In `src/main.rs`, after `config_path()` (around line 47), add:

```rust
fn shares_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("explorenova").join("shares.toml")
}
```

- [ ] **Step 2: Load shares into the app + spawn background auto-mount**

In `main()`, after `let mut app = App::new(arena, config, state.clone());` (line 141), add:

```rust
    let sharesp = shares_path();
    app.shares = explorenova::smb::load(&sharesp);
    app.shares_path = sharesp;
    // Background auto-mount: fire `gio mount` per saved share off-thread so
    // the first draw is never blocked by network I/O. Unreachable servers /
    // locked keyring fail quietly; the share just shows unmounted.
    {
        let urls: Vec<String> =
            app.shares.iter().map(|s| s.url.clone()).collect();
        std::thread::spawn(move || {
            for url in urls {
                explorenova::smb::mount_background(&url);
            }
        });
    }
```

- [ ] **Step 3: Build + run smoke test**

Run: `cargo build --release`
Expected: compiles.

Run: `cargo test`
Expected: all PASS.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(smb): load shares.toml + background auto-mount on startup"
```

---

## Task 9: overlays — Network sub-header, glyph, footer hints

**Files:**
- Modify: `src/ui/overlays.rs`

- [ ] **Step 1: Render a "Network" sub-header before the first SMB row**

In `render_devices` (`src/ui/overlays.rs:264`), inside the `for (i, d) in items.iter().enumerate()` loop, before pushing each row, insert a one-line "Network" separator the first time an smb row appears. Add a `let mut net_header_done = false;` before the loop, then at the top of the loop body:

```rust
        for (i, d) in items.iter().enumerate() {
            if d.is_smb() && !net_header_done {
                net_header_done = true;
                body_lines.push(Line::from(Span::styled(
                    "  Network",
                    Style::default()
                        .fg(theme.header)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            // ...existing row rendering...
```

NOTE: the header is a non-selectable visual line inserted into `body_lines`. Because selection indexing is by `items` index and `viewport_around` operates on `body_lines`, inserting an extra line shifts the visual mapping. To avoid a selection/highlight mismatch, render the header as part of the SAME push by prepending it only when it does not break the `sel`↔row correspondence. SIMPLER + SAFE approach used here: keep `body_lines` 1:1 with `items` (no inserted header line) and instead mark SMB rows with a network glyph (Step 2). Skip the sub-header insertion — do NOT implement the snippet above.

(If a visible section divider is wanted later, it requires teaching `viewport_around`/selection about non-row lines; that is out of scope for this plan.)

- [ ] **Step 2: Give SMB rows a network glyph**

In the same loop, the `mounted_span` currently shows `●`/`○`. Keep that, but change the per-row marker/icon so SMB rows are visually distinct. Replace the `marker` line:

```rust
            let marker = if i == sel { "▸ " } else { "  " };
```

with:

```rust
            let marker = if i == sel {
                "▸ "
            } else if d.is_smb() {
                "  "
            } else {
                "  "
            };
            let kind_glyph = if d.is_smb() { "🖧 " } else { "   " };
```

and add `kind_glyph` into the `Line::from(vec![...])` for the row, right after `marker`:

```rust
            body_lines.push(Line::from(vec![
                Span::styled(marker, style),
                Span::styled(kind_glyph, style),
                mounted_span,
                Span::styled(
                    format!(
                        "{:<14} {:<8} {:<8} {:<14} {}",
                        d.device, size, fs, label, mp
                    ),
                    style,
                ),
            ]));
```

(If the Nerd Font network glyph `🖧` renders poorly in the target terminal, substitute the Nerd Font code point used elsewhere in `src/icons.rs` for network/remote; check `icons.rs` for an existing glyph constant and prefer it. A plain `[N]` is an acceptable ASCII fallback.)

- [ ] **Step 3: Update footer hints**

In `render_devices`, the footer block (`src/ui/overlays.rs:365-371`) currently reads:

```rust
    let footer = vec![
        Line::from(""),
        Line::from(Span::styled(
            " enter cd/mount · u umount · r refresh · esc close",
            modal::dim_style(theme),
        )),
    ];
```

Change only the literal string to include the new keys:

```rust
            " enter cd/mount · a add · d delete · u umount · r refresh · esc close",
```

- [ ] **Step 3b: Add an input-modal title for AddSmb**

In `render_input` (`src/ui/overlays.rs:375`), the title `match app.pending` block (lines 378-385) maps each pending action to a title. Add an `AddSmb` arm before the `_ => "Input",` fallback:

```rust
            Some(Action::OpenWith) => "Open with (command)",
            Some(Action::ShellOnce) => "Shell",
            Some(Action::AddSmb) => "Add SMB share (smb://user@host/share)",
            _ => "Input",
```

- [ ] **Step 4: Build + visual check**

Run: `cargo build`
Expected: compiles.

Run: `cargo test`
Expected: all PASS.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/ui/overlays.rs
git commit -m "feat(smb): devices modal network glyph + add/delete footer hints"
```

---

## Task 10: PKGBUILD deps + manual verification

**Files:**
- Modify: `PKGBUILD`

- [ ] **Step 1: Add gvfs deps**

In `PKGBUILD`, in the `depends=(...)` array, add a "Network mounts" group (after the `udiskie` line):

```bash
  # Devices modal mount/unmount/list.
  'udiskie'
  # SMB network shares (gio mount smb://...).
  'gvfs' 'gvfs-smb'
```

- [ ] **Step 2: Commit**

```bash
git add PKGBUILD
git commit -m "build: depend on gvfs + gvfs-smb for SMB shares"
```

- [ ] **Step 3: Full suite + clippy (final gate)**

Run: `cargo test`
Expected: all PASS (unit tests for smb:: + existing suite green).

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Manual verification (requires a real NAS + gio installed)**

These cannot be unit-tested. Perform by hand against the user's NAS:

1. Launch `explorenova`. Press `M` → devices modal opens, shows physical devices.
2. Press `a` → URL prompt. Type `smb://<user>@<host>/<share>`, Enter.
3. The TUI suspends; gio prompts for the password on the terminal. Enter it, choose **remember forever**. Press Enter to return.
4. Modal reopens; the new share appears with a network glyph and a `●` mounted indicator.
5. Press `Enter` on it → arena reroots into the share; browse files; open one.
6. Re-open `M`, select the share, press `u` → unmounts (`○`).
7. Press `Enter` again → remounts silently (keyring) without a password prompt.
8. Press `d` on the share → removed from the list and from `~/.config/explorenova/shares.toml`.
9. Re-add it, quit, relaunch → it auto-mounts in the background (startup still instant; `●` shows once the background mount lands; re-open `M` to see status).
10. Edge: add `smb://bad-host/x` → background/interactive mount fails with a status message, no crash; share persists and can be deleted.

Report results. If gvfs glyph rendering or the gio prompt sequence misbehaves on CachyOS, note it for a follow-up fix.

---

## Self-Review (completed by plan author)

**Spec coverage:**
- gio/GVFS mount → Tasks 4, 7, 8. ✓
- keyring-delegated creds (no plaintext) → Task 4 (`mount_interactive` via gio TTY prompt), Task 7 submit. ✓
- shares.toml persistence (dedicated file) → Tasks 2, 8. ✓
- mountpoint scan (order-robust) → Task 3. ✓
- DeviceKind merge into modal → Tasks 5, 7. ✓
- add (single URL field) → Task 7 (`devices_begin_add` + `AddSmb` submit). ✓
- delete saved share → Task 7 (`devices_delete_selected`). ✓
- background auto-mount, instant startup → Task 8. ✓
- error handling (gio missing, bad URL, corrupt shares.toml, mount fail) → Tasks 2, 4, 7. ✓
- Network section / glyph → Task 9 (section divider deferred as documented; glyph implemented). ✓
- PKGBUILD gvfs deps → Task 10. ✓
- testing (unit + manual) → per-task tests + Task 10 manual checklist. ✓

**Placeholder scan:** No TBD/TODO. Task 9 Step 1 explicitly instructs NOT to implement the divider (selection-mapping hazard) and documents why — that is a decision, not a placeholder.

**Type consistency:** `SmbShare{name,url}`, `Parsed{host,share,user}`, `DeviceKind::{Block,Smb{url}}`, `Device.kind`, `Action::AddSmb`, `App.shares/shares_path`, `smb::{parse_url,derive_name,load,save,mountpoint,is_mounted,mount_interactive,mount_background,unmount,share_to_row,to_device_rows,mount_argv,unmount_argv}`, `shell::run_interactive` — all referenced consistently across tasks.

**Known scope note:** The "Network" sub-header divider is intentionally deferred (Task 9 Step 1) because the modal's selection model is 1:1 with `items`; a non-row visual line would desync the highlight without refactoring `viewport_around`. SMB rows are distinguished by glyph instead. Flagged for the user.
