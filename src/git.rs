use crate::tree::GitStatus;
use git2::{Repository, Status, StatusOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Given the first received root plus any others already queued, return
/// the last one (most recent) — a burst of triggers coalesces to one scan.
pub fn coalesce_latest(first: PathBuf, rest: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    rest.into_iter().last().unwrap_or(first)
}

/// Map every changed path (absolute) to its GitStatus, plus aggregate the
/// worst status onto each ancestor directory up to the repo workdir.
pub fn scan_status(root: &Path) -> anyhow::Result<HashMap<PathBuf, GitStatus>> {
    let mut out: HashMap<PathBuf, GitStatus> = HashMap::new();
    let repo = match Repository::discover(root) {
        Ok(r) => r,
        Err(_) => return Ok(out), // not a repo: empty map
    };
    let workdir = match repo.workdir() {
        Some(w) => w.to_path_buf(),
        None => return Ok(out),
    };
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    for entry in repo.statuses(Some(&mut opts))?.iter() {
        let st = entry.status();
        let s = classify(st);
        if let Ok(rel) = entry.path() {
            let abs = workdir.join(rel);
            bump(&mut out, abs.clone(), s);
            // aggregate onto ancestors up to workdir
            let mut p = abs.parent().map(Path::to_path_buf);
            while let Some(dir) = p {
                if !dir.starts_with(&workdir) || dir == workdir {
                    bump(&mut out, workdir.clone(), s);
                    break;
                }
                bump(&mut out, dir.clone(), s);
                p = dir.parent().map(Path::to_path_buf);
            }
        }
    }
    Ok(out)
}

fn bump(map: &mut HashMap<PathBuf, GitStatus>, key: PathBuf, s: GitStatus) {
    map.entry(key)
        .and_modify(|cur| {
            if s.rank() > cur.rank() {
                *cur = s;
            }
        })
        .or_insert(s);
}

fn classify(st: Status) -> GitStatus {
    if st.is_conflicted() {
        GitStatus::Conflicted
    } else if st.is_wt_new() || st.is_index_new() {
        if st.is_index_new() {
            GitStatus::Added
        } else {
            GitStatus::Untracked
        }
    } else if st.is_wt_deleted() || st.is_index_deleted() {
        GitStatus::Deleted
    } else if st.is_wt_renamed() || st.is_index_renamed() {
        GitStatus::Renamed
    } else if st.is_ignored() {
        GitStatus::Ignored
    } else if st.is_wt_modified() || st.is_index_modified() {
        GitStatus::Modified
    } else {
        GitStatus::Clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn non_repo_returns_empty_map() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan_status(d.path()).unwrap().is_empty());
    }

    #[test]
    fn coalesce_takes_most_recent() {
        let r = coalesce_latest(
            PathBuf::from("/a"),
            vec![PathBuf::from("/b"), PathBuf::from("/c")],
        );
        assert_eq!(r, PathBuf::from("/c"));
    }

    #[test]
    fn coalesce_single_keeps_first() {
        let r = coalesce_latest(PathBuf::from("/a"), Vec::<PathBuf>::new());
        assert_eq!(r, PathBuf::from("/a"));
    }
}
