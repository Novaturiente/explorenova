use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::devices::{Device, DeviceKind};

use serde::{Deserialize, Serialize};

/// On-disk wrapper so the file is `[[share]]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SharesFile {
    #[serde(default, rename = "share")]
    shares: Vec<SmbShare>,
}

/// Load saved shares. A missing or unparseable file yields an empty
/// Vec — never panics, so a corrupt file can't brick startup.
pub fn load(path: &Path) -> Vec<SmbShare> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    toml::from_str::<SharesFile>(&s)
        .map(|f| f.shares)
        .unwrap_or_default()
}

/// Persist shares as TOML, creating the parent directory if needed.
pub fn save(path: &Path, shares: &[SmbShare]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = SharesFile { shares: shares.to_vec() };
    let body = toml::to_string_pretty(&f)?;
    std::fs::write(
        path,
        format!("# explorenova SMB shares\n{body}"),
    )?;
    Ok(())
}

/// A persisted SMB share. Password is NEVER stored here — it lives
/// in the system keyring via gio. `url` carries any username
/// (`smb://user@host/share`).
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
    let rest = raw
        .strip_prefix("smb://")
        .ok_or_else(|| anyhow::anyhow!("URL must start with smb://"))?;
    // Split optional `user@` off the front of the authority.
    let (user, hostpart) = match rest.split_once('@') {
        Some((u, h)) if !u.is_empty() => {
            (Some(u.to_string()), h)
        }
        Some((_, h)) => (None, h), // empty user: drop the @
        None => (None, rest),
    };
    // hostpart = "host/share/...". Host is up to the first '/'.
    // host may include ":port" (e.g. "nas.local:445"); gio handles it.
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

/// Display name = the share segment.
pub fn derive_name(p: &Parsed) -> String {
    p.share.clone()
}

/// Directory gvfs FUSE-mounts live under: `$XDG_RUNTIME_DIR/gvfs`,
/// falling back to `/run/user/<uid>/gvfs`.
fn gvfs_dir() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(rt).join("gvfs");
    }
    // SAFETY: getuid() always succeeds; no invariants to
    // uphold.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/run/user/{uid}/gvfs"))
}

/// Scan `gvfs_dir` for an `smb-share:` entry whose name contains both
/// `server=<host>` and `share=<share>` (case-insensitive). Returns its
/// absolute path. Split out from [`mountpoint`] so tests can point at a
/// temp dir.
fn mountpoint_in(
    gvfs_dir: &Path,
    host: &str,
    share: &str,
) -> Option<PathBuf> {
    let want_server =
        format!("server={}", host.to_lowercase());
    let want_share =
        format!("share={}", share.to_lowercase());
    let entries = std::fs::read_dir(gvfs_dir).ok()?;
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy().to_lowercase();
        let Some(rest) = name.strip_prefix("smb-share:")
        else {
            continue;
        };
        // gvfs joins key=value pairs with ','. Match whole
        // tokens so `server=nas.local` can't match
        // `server=nas.local2`.
        let mut has_server = false;
        let mut has_share = false;
        for tok in rest.split(',') {
            if tok == want_server {
                has_server = true;
            } else if tok == want_share {
                has_share = true;
            }
        }
        if has_server && has_share {
            return Some(ent.path());
        }
    }
    None
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

fn mount_argv(url: &str) -> Vec<String> {
    vec!["mount".into(), url.into()]
}

fn unmount_argv(url: &str) -> Vec<String> {
    vec!["mount".into(), "-u".into(), url.into()]
}

/// Interactive mount: gio prompts for credentials on the real TTY
/// (and can "remember forever" into the keyring). Suspends the TUI
/// for the duration.
pub fn mount_interactive(url: &str) -> std::io::Result<()> {
    let args = mount_argv(url);
    let refs: Vec<&str> =
        args.iter().map(String::as_str).collect();
    crate::shell::run_interactive("gio", &refs)
}

/// Background mount: no TTY. Succeeds silently if the keyring
/// already holds the credentials; fails quietly otherwise (cannot
/// prompt). Safe to call from a worker thread; never touches the
/// terminal.
pub fn mount_background(url: &str) {
    let _ = Command::new("gio")
        .args(mount_argv(url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Unmount via gio. Surfaces a clear error if gio is absent or the
/// unmount fails.
pub fn unmount(url: &str) -> anyhow::Result<()> {
    let out = Command::new("gio")
        .args(unmount_argv(url))
        .output()
        .map_err(|e| anyhow::anyhow!("gio not available: {e}"))?;
    if !out.status.success() {
        let err =
            String::from_utf8_lossy(&out.stderr).into_owned();
        anyhow::bail!("{}", err.trim());
    }
    Ok(())
}

/// Map a saved share + its (optional) live mount path to a
/// devices-modal row. Pure — `mountpoint` is injected so this
/// is unit-testable.
pub fn share_to_row(
    share: &SmbShare,
    mountpoint: Option<PathBuf>,
) -> Device {
    Device {
        device: share.name.clone(),
        label: Some(share.url.clone()),
        size: None,
        fstype: Some("smb".to_string()),
        mountpoint,
        kind: DeviceKind::Smb { url: share.url.clone() },
    }
}

/// Build a devices-modal row per saved share, resolving live
/// mount status.
pub fn to_device_rows(shares: &[SmbShare]) -> Vec<Device> {
    shares
        .iter()
        .map(|s| share_to_row(s, is_mounted(s)))
        .collect()
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
    fn parse_url_with_empty_user_treats_as_no_user() {
        let p = parse_url("smb://@nas.local/media").unwrap();
        assert_eq!(p.host, "nas.local");
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

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shares.toml");
        let shares = vec![
            SmbShare {
                name: "media".into(),
                url: "smb://nas/media".into(),
            },
            SmbShare {
                name: "backup".into(),
                url: "smb://u@nas/backup".into(),
            },
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
        save(
            &path,
            &[SmbShare {
                name: "m".into(),
                url: "smb://h/m".into(),
            }],
        )
        .unwrap();
        assert!(path.exists());
    }

    fn touch_dir(base: &Path, name: &str) {
        std::fs::create_dir_all(base.join(name)).unwrap();
    }

    #[test]
    fn mountpoint_in_matches_server_and_share_any_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=nas.local,share=media");
        let mp =
            mountpoint_in(g, "nas.local", "media").unwrap();
        assert_eq!(
            mp,
            g.join("smb-share:server=nas.local,share=media")
        );
    }

    #[test]
    fn mountpoint_in_matches_with_user_and_domain_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(
            g,
            "smb-share:domain=WG,server=nas.local,\
             share=media,user=alice",
        );
        let mp = mountpoint_in(g, "nas.local", "media");
        assert!(mp.is_some());
    }

    #[test]
    fn mountpoint_in_is_case_insensitive_on_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=NAS.LOCAL,share=Media");
        assert!(
            mountpoint_in(g, "nas.local", "media").is_some()
        );
    }

    #[test]
    fn mountpoint_in_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=other,share=media");
        assert!(
            mountpoint_in(g, "nas.local", "media").is_none()
        );
    }

    #[test]
    fn mountpoint_in_none_when_gvfs_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("does-not-exist");
        assert!(
            mountpoint_in(&g, "nas.local", "media").is_none()
        );
    }

    #[test]
    fn share_to_row_unmounted() {
        let s = SmbShare {
            name: "media".into(),
            url: "smb://nas/media".into(),
        };
        let row = share_to_row(&s, None);
        assert_eq!(row.device, "media");
        assert_eq!(row.fstype.as_deref(), Some("smb"));
        assert_eq!(
            row.label.as_deref(),
            Some("smb://nas/media")
        );
        assert_eq!(row.mountpoint, None);
        assert!(!row.is_mounted());
        assert!(row.is_smb());
        assert_eq!(
            row.kind,
            DeviceKind::Smb {
                url: "smb://nas/media".into()
            }
        );
    }

    #[test]
    fn share_to_row_mounted() {
        let s = SmbShare {
            name: "media".into(),
            url: "smb://nas/media".into(),
        };
        let mp = std::path::PathBuf::from(
            "/run/user/1000/gvfs/smb-share:server=nas,share=media",
        );
        let row = share_to_row(&s, Some(mp.clone()));
        assert_eq!(row.mountpoint, Some(mp));
        assert!(row.is_mounted());
    }

    #[test]
    fn mount_argv_is_gio_mount_url() {
        assert_eq!(
            mount_argv("smb://nas/media"),
            vec![
                "mount".to_string(),
                "smb://nas/media".to_string()
            ]
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

    #[test]
    fn mountpoint_in_no_prefix_false_positive() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(
            g,
            "smb-share:server=nas.local2,share=media",
        );
        assert!(
            mountpoint_in(g, "nas.local", "media").is_none()
        );
    }
}
