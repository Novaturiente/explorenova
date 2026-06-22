use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::devices::{Device, DeviceKind};

use serde::{Deserialize, Serialize};

/// On-disk wrapper so the file is `[[share]]` tables.
///
/// ponytail: module is named `smb` for history, but it now handles every
/// gio-mountable network scheme (smb/ftp/ftps/sftp) — the scheme lives in
/// each share's `url`, so the on-disk format never had to change.
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
        format!("# explorenova network shares (smb/ftp/ftps/sftp)\n{body}"),
    )?;
    Ok(())
}

/// A persisted network share. Password is NEVER stored here — it lives
/// in the system keyring via gio. `url` carries the scheme and any
/// username (`smb://user@host/share`, `ftp://user@host`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmbShare {
    pub name: String,
    pub url: String,
}

/// A gio-mountable network scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Smb,
    Ftp,
    Ftps,
    Sftp,
}

impl Scheme {
    /// Strip a recognized `scheme://` prefix, returning the scheme and the
    /// remainder. None for any other scheme.
    fn split_prefix(raw: &str) -> Option<(Scheme, &str)> {
        // ftps:// before ftp:// is harmless (neither is a prefix of the
        // other) but keep both explicit so the intent is clear.
        const TABLE: [(&str, Scheme); 4] = [
            ("smb://", Scheme::Smb),
            ("ftps://", Scheme::Ftps),
            ("ftp://", Scheme::Ftp),
            ("sftp://", Scheme::Sftp),
        ];
        TABLE
            .iter()
            .find_map(|(p, s)| raw.strip_prefix(p).map(|rest| (*s, rest)))
    }

    /// Only SMB addresses a named share; ftp/sftp are host (+ path) only.
    fn needs_share(self) -> bool {
        matches!(self, Scheme::Smb)
    }

    /// Short label shown in the devices modal's fstype column.
    pub fn label(self) -> &'static str {
        match self {
            Scheme::Smb => "smb",
            Scheme::Ftp => "ftp",
            Scheme::Ftps => "ftps",
            Scheme::Sftp => "sftp",
        }
    }

    /// gvfs FUSE mount-dir name prefixes for this scheme. ftp and ftps
    /// share the `gvfsd-ftp` backend, which may name the dir either way.
    fn gvfs_prefixes(self) -> &'static [&'static str] {
        match self {
            Scheme::Smb => &["smb-share:"],
            Scheme::Ftp | Scheme::Ftps => &["ftp:", "ftps:"],
            Scheme::Sftp => &["sftp:"],
        }
    }
}

/// Decomposed network URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub scheme: Scheme,
    pub host: String,
    /// First path segment after the host. Required for smb (the share
    /// name); empty for ftp/sftp when the URL is host-only.
    pub share: String,
    pub user: Option<String>,
}

/// Validate and decompose a `scheme://[user@]host[/share[/...]]` URL where
/// scheme is one of smb/ftp/ftps/sftp. Errors on an unknown scheme, an
/// empty host, or (smb only) a missing share segment.
pub fn parse_url(raw: &str) -> anyhow::Result<Parsed> {
    let (scheme, rest) = Scheme::split_prefix(raw).ok_or_else(|| {
        anyhow::anyhow!("URL must start with smb://, ftp://, ftps:// or sftp://")
    })?;
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
        anyhow::bail!("missing host in URL");
    }
    let share = tail.trim_matches('/').split('/').next().unwrap_or("");
    if scheme.needs_share() && share.is_empty() {
        anyhow::bail!("missing share name in smb URL");
    }
    Ok(Parsed {
        scheme,
        host: host.to_string(),
        share: share.to_string(),
        user,
    })
}

/// Display name: the share segment for smb, else the host.
pub fn derive_name(p: &Parsed) -> String {
    if p.share.is_empty() {
        p.host.clone()
    } else {
        p.share.clone()
    }
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

/// Scan `gvfs_dir` for the FUSE mount-dir matching `p`. Matching is
/// scheme-aware and case-insensitive: smb needs both `server=<host>` and
/// `share=<share>`; ftp/ftps/sftp need only `host=<host>`. Returns the
/// absolute path. Split out from [`mountpoint`] so tests can point at a
/// temp dir.
fn mountpoint_in(gvfs_dir: &Path, p: &Parsed) -> Option<PathBuf> {
    // gvfs strips any `:port` suffix from the host token, so match on the
    // host without it (e.g. `nas:445` → `server=nas` / `host=nas`).
    let host_no_port =
        p.host.split(':').next().unwrap_or(&p.host).to_lowercase();
    let entries = std::fs::read_dir(gvfs_dir).ok()?;
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy().to_lowercase();
        // Pick off the scheme prefix; skip entries from other backends.
        let Some(rest) = p
            .scheme
            .gvfs_prefixes()
            .iter()
            .find_map(|pre| name.strip_prefix(pre))
        else {
            continue;
        };
        // gvfs joins key=value pairs with ','. Match whole tokens so
        // `server=nas.local` can't match `server=nas.local2`.
        let host_ok = rest.split(',').any(|tok| {
            tok == format!("server={host_no_port}")
                || tok == format!("host={host_no_port}")
        });
        if !host_ok {
            continue;
        }
        // smb additionally keys on the share name; ftp/sftp don't have one.
        if p.scheme.needs_share() {
            let want_share = format!("share={}", p.share.to_lowercase());
            if !rest.split(',').any(|tok| tok == want_share) {
                continue;
            }
        }
        return Some(ent.path());
    }
    None
}

/// Resolve the live mount path for a parsed URL, or None if not mounted.
pub fn mountpoint(p: &Parsed) -> Option<PathBuf> {
    mountpoint_in(&gvfs_dir(), p)
}

/// Some(path) if the share is currently mounted.
pub fn is_mounted(share: &SmbShare) -> Option<PathBuf> {
    let p = parse_url(&share.url).ok()?;
    mountpoint(&p)
}

/// Friendly header label for a gvfs network mount directory:
/// `smb-share:server=nas.local,share=media` -> "media on nas.local";
/// `ftp:host=nas.local,user=bob` -> "nas.local". Returns None when
/// `path` is not a recognized gvfs entry (so callers fall back to the
/// raw path).
pub fn gvfs_display_label(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if let Some(rest) = name.strip_prefix("smb-share:") {
        let mut share = None;
        let mut server = None;
        for tok in rest.split(',') {
            if let Some(v) = tok.strip_prefix("share=") {
                share = Some(v);
            } else if let Some(v) = tok.strip_prefix("server=") {
                server = Some(v);
            }
        }
        let share = share?;
        return Some(match server {
            Some(s) => format!("{share} on {s}"),
            None => share.to_string(),
        });
    }
    // ftp/ftps/sftp: dir is `<scheme>:host=...[,user=...]`. Label = host.
    for pre in ["ftps:", "ftp:", "sftp:"] {
        if let Some(rest) = name.strip_prefix(pre) {
            for tok in rest.split(',') {
                if let Some(v) = tok.strip_prefix("host=") {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// A URL with no `user@` means "connect anonymously". gio's interactive
/// prompt can't send a null/guest session — it always authenticates as the
/// default user — so anonymous shares need the explicit `-a` flag.
fn wants_anonymous(url: &str) -> bool {
    parse_url(url).map(|p| p.user.is_none()).unwrap_or(false)
}

fn mount_argv(url: &str, anon: bool) -> Vec<String> {
    let mut a = vec!["mount".to_string()];
    if anon {
        a.push("-a".into());
    }
    a.push(url.into());
    a
}

fn unmount_argv(url: &str) -> Vec<String> {
    vec!["mount".into(), "-u".into(), url.into()]
}

/// Mount a saved share. With a username in the URL, gio prompts for the
/// password on the real TTY (suspending the TUI) and can remember it in
/// the keyring. With no username, mounts anonymously via `gio -a` — which
/// needs no prompt, so the TUI is left untouched.
pub fn mount_interactive(url: &str) -> std::io::Result<()> {
    if wants_anonymous(url) {
        // `-a` never prompts; run it inline and surface any error.
        let out = Command::new("gio")
            .args(mount_argv(url, true))
            .output()
            .map_err(|e| {
                std::io::Error::new(e.kind(), format!("gio: {e}"))
            })?;
        if !out.status.success() {
            let err =
                String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(std::io::Error::other(err));
        }
        return Ok(());
    }
    let args = mount_argv(url, false);
    let refs: Vec<&str> =
        args.iter().map(String::as_str).collect();
    crate::shell::run_interactive("gio", &refs)
}

/// Background mount: no TTY. For anonymous URLs passes `-a` (always works,
/// no creds needed); for credentialed URLs succeeds only if the keyring
/// already holds the password, else fails quietly (cannot prompt). Safe to
/// call from a worker thread; never touches the terminal.
pub fn mount_background(url: &str) {
    let _ = Command::new("gio")
        .args(mount_argv(url, wants_anonymous(url)))
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
    // fstype column = the URL scheme (smb/ftp/ftps/sftp); fall back to
    // "net" for a share whose URL no longer parses.
    let fstype = parse_url(&share.url)
        .map(|p| p.scheme.label().to_string())
        .unwrap_or_else(|_| "net".to_string());
    Device {
        device: share.name.clone(),
        label: Some(share.url.clone()),
        size: None,
        fstype: Some(fstype),
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

    /// Build a parsed smb URL for the mountpoint tests.
    fn smb(host: &str, share: &str) -> Parsed {
        Parsed {
            scheme: Scheme::Smb,
            host: host.to_string(),
            share: share.to_string(),
            user: None,
        }
    }

    #[test]
    fn mountpoint_in_matches_server_and_share_any_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=nas.local,share=media");
        let mp =
            mountpoint_in(g, &smb("nas.local", "media")).unwrap();
        assert_eq!(
            mp,
            g.join("smb-share:server=nas.local,share=media")
        );
    }

    #[test]
    fn mountpoint_in_matches_when_host_has_port() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        // gvfs drops the :445 from the server token.
        touch_dir(g, "smb-share:server=nas,share=media");
        let mp = mountpoint_in(g, &smb("nas:445", "media")).unwrap();
        assert_eq!(mp, g.join("smb-share:server=nas,share=media"));
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
        let mp = mountpoint_in(g, &smb("nas.local", "media"));
        assert!(mp.is_some());
    }

    #[test]
    fn mountpoint_in_is_case_insensitive_on_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=NAS.LOCAL,share=Media");
        assert!(
            mountpoint_in(g, &smb("nas.local", "media")).is_some()
        );
    }

    #[test]
    fn mountpoint_in_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "smb-share:server=other,share=media");
        assert!(
            mountpoint_in(g, &smb("nas.local", "media")).is_none()
        );
    }

    #[test]
    fn mountpoint_in_none_when_gvfs_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("does-not-exist");
        assert!(
            mountpoint_in(&g, &smb("nas.local", "media")).is_none()
        );
    }

    #[test]
    fn mountpoint_in_matches_ftp_by_host_only() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "ftp:host=nas.local,user=bob");
        let p = parse_url("ftp://bob@nas.local").unwrap();
        assert_eq!(
            mountpoint_in(g, &p).unwrap(),
            g.join("ftp:host=nas.local,user=bob")
        );
    }

    #[test]
    fn mountpoint_in_ftps_matches_ftp_backend_dir() {
        // gvfsd-ftp serves ftps too; the dir may be named `ftp:`.
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "ftp:host=files.example.com");
        let p = parse_url("ftps://files.example.com").unwrap();
        assert!(mountpoint_in(g, &p).is_some());
    }

    #[test]
    fn mountpoint_in_sftp_matches_host() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "sftp:host=dev.box,user=alice");
        let p = parse_url("sftp://alice@dev.box/srv").unwrap();
        assert!(mountpoint_in(g, &p).is_some());
    }

    #[test]
    fn mountpoint_in_ftp_does_not_match_sftp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path();
        touch_dir(g, "sftp:host=nas.local");
        let p = parse_url("ftp://nas.local").unwrap();
        assert!(mountpoint_in(g, &p).is_none());
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
            mount_argv("smb://nas/media", false),
            vec![
                "mount".to_string(),
                "smb://nas/media".to_string()
            ]
        );
    }

    #[test]
    fn mount_argv_anonymous_adds_dash_a() {
        assert_eq!(
            mount_argv("smb://nas/media", true),
            vec![
                "mount".to_string(),
                "-a".to_string(),
                "smb://nas/media".to_string()
            ]
        );
    }

    #[test]
    fn wants_anonymous_when_no_user_in_url() {
        assert!(wants_anonymous("smb://novahome/homeshare"));
        assert!(wants_anonymous("ftp://files.example.com"));
        assert!(!wants_anonymous("smb://nova@novahome/homeshare"));
        assert!(!wants_anonymous("sftp://alice@dev.box/srv"));
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
            mountpoint_in(g, &smb("nas.local", "media")).is_none()
        );
    }

    #[test]
    fn parse_ftp_host_only() {
        let p = parse_url("ftp://files.example.com").unwrap();
        assert_eq!(p.scheme, Scheme::Ftp);
        assert_eq!(p.host, "files.example.com");
        assert_eq!(p.share, "");
        assert_eq!(p.user, None);
    }

    #[test]
    fn parse_ftps_and_sftp_schemes() {
        assert_eq!(
            parse_url("ftps://h").unwrap().scheme,
            Scheme::Ftps
        );
        assert_eq!(
            parse_url("sftp://u@h/path").unwrap().scheme,
            Scheme::Sftp
        );
    }

    #[test]
    fn parse_ftp_with_user_and_path() {
        let p = parse_url("ftp://bob@nas.local/pub/files").unwrap();
        assert_eq!(p.user.as_deref(), Some("bob"));
        assert_eq!(p.host, "nas.local");
        assert_eq!(p.share, "pub"); // first path segment, used for naming
    }

    #[test]
    fn ftp_allows_host_only_but_smb_still_needs_share() {
        assert!(parse_url("ftp://nas.local").is_ok());
        assert!(parse_url("sftp://nas.local").is_ok());
        assert!(parse_url("smb://nas.local").is_err());
    }

    #[test]
    fn derive_name_host_when_no_share() {
        let p = parse_url("ftp://files.example.com").unwrap();
        assert_eq!(derive_name(&p), "files.example.com");
    }

    #[test]
    fn share_to_row_fstype_reflects_scheme() {
        let s = SmbShare {
            name: "f".into(),
            url: "ftp://h".into(),
        };
        assert_eq!(share_to_row(&s, None).fstype.as_deref(), Some("ftp"));
    }

    #[test]
    fn gvfs_label_ftp_is_host() {
        let p = Path::new("/run/user/1000/gvfs/ftp:host=nas.local,user=bob");
        assert_eq!(gvfs_display_label(p).as_deref(), Some("nas.local"));
    }

    #[test]
    fn gvfs_label_share_on_server() {
        let p = Path::new(
            "/run/user/1000/gvfs/\
             smb-share:server=nas.local,share=media",
        );
        assert_eq!(
            gvfs_display_label(p).as_deref(),
            Some("media on nas.local")
        );
    }

    #[test]
    fn gvfs_label_token_order_independent() {
        let p = Path::new(
            "smb-share:share=media,server=nas,user=alice",
        );
        assert_eq!(
            gvfs_display_label(p).as_deref(),
            Some("media on nas")
        );
    }

    #[test]
    fn gvfs_label_share_only_when_no_server() {
        let p = Path::new("smb-share:share=media");
        assert_eq!(
            gvfs_display_label(p).as_deref(),
            Some("media")
        );
    }

    #[test]
    fn gvfs_label_none_for_non_smb_path() {
        assert!(
            gvfs_display_label(Path::new("/home/nova")).is_none()
        );
    }

    #[test]
    fn gvfs_label_none_when_no_share_token() {
        let p = Path::new("smb-share:server=nas");
        assert!(gvfs_display_label(p).is_none());
    }
}
