use std::fs;
use std::path::{Path, PathBuf};

/// Expand leading `~` and `$VAR`/`${VAR}`.
pub fn expand(input: &str) -> String {
    let mut s = input.to_string();
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            s = format!("{}{}", home.to_string_lossy(), rest);
        }
    }
    while let Some(start) = s.find('$') {
        let bytes = &s[start + 1..];
        let (name, end) = if bytes.starts_with('{') {
            match bytes.find('}') {
                Some(i) => (&bytes[1..i], start + 1 + i + 1),
                None => break,
            }
        } else {
            let len = bytes
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(bytes.len());
            (&bytes[..len], start + 1 + len)
        };
        let val = std::env::var(name).unwrap_or_default();
        s.replace_range(start..end, &val);
    }
    s
}

/// Filesystem completions for `input`. Returns sorted matching full paths.
pub fn completions(input: &str) -> Vec<PathBuf> {
    let expanded = expand(input);
    let p = Path::new(&expanded);
    let (dir, prefix): (PathBuf, String) = if expanded.ends_with('/') {
        (p.to_path_buf(), String::new())
    } else {
        (
            p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        )
    };
    let mut out: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|c| {
                c.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Longest common prefix of candidate file-name strings, joined onto dir.
pub fn longest_common(input: &str, candidates: &[PathBuf]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    let names: Vec<String> = candidates
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();
    let mut lcp = names[0].clone();
    for n in &names[1..] {
        while !n.starts_with(&lcp) {
            lcp.pop();
            if lcp.is_empty() {
                return None;
            }
        }
    }
    let dir = candidates[0].parent()?;
    let _ = input;
    Some(dir.join(&lcp).to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn expand_tilde_and_env() {
        std::env::set_var("HOME", "/home/x");
        assert_eq!(expand("~/foo"), "/home/x/foo");
        std::env::set_var("EN_T", "bar");
        assert_eq!(expand("/a/$EN_T/b"), "/a/bar/b");
        assert_eq!(expand("/a/${EN_T}/b"), "/a/bar/b");
    }

    #[test]
    fn completions_filter_by_prefix() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("alpha")).unwrap();
        fs::create_dir(d.path().join("alpine")).unwrap();
        fs::create_dir(d.path().join("beta")).unwrap();
        let inp = format!("{}/al", d.path().display());
        let c = completions(&inp);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn longest_common_prefix_cycle_base() {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("alpha")).unwrap();
        fs::create_dir(d.path().join("alpine")).unwrap();
        let inp = format!("{}/al", d.path().display());
        let c = completions(&inp);
        let lcp = longest_common(&inp, &c).unwrap();
        assert!(lcp.ends_with("/alp"));
    }
}
