use serde::{Deserialize, Serialize};

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
}
