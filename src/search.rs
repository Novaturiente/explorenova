use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use std::path::{Path, PathBuf};

/// Collect candidate paths under `root`. `include_all` => ignore .gitignore + show hidden.
pub fn collect(root: &Path, include_all: bool) -> Vec<PathBuf> {
    let mut wb = WalkBuilder::new(root);
    wb.hidden(!include_all)
        .git_ignore(!include_all)
        .git_global(!include_all)
        .git_exclude(!include_all)
        .require_git(false);
    wb.build()
        .filter_map(Result::ok)
        .map(|d| d.into_path())
        .filter(|p| p != root)
        .collect()
}

/// Rank `candidates` against `query`. Returns (path, score) descending,
/// plus matched char indices on the path string for highlighting.
pub fn rank(
    candidates: &[PathBuf],
    root: &Path,
    query: &str,
) -> Vec<(PathBuf, u32, Vec<u32>)> {
    if query.is_empty() {
        return candidates.iter().map(|p| (p.clone(), 0, vec![])).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pat = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(PathBuf, u32, Vec<u32>)> = Vec::new();
    for p in candidates {
        let rel = p.strip_prefix(root).unwrap_or(p);
        let hay = rel.to_string_lossy();
        let mut buf = Vec::new();
        let h = nucleo::Utf32Str::new(&hay, &mut buf);
        let mut indices = Vec::new();
        if let Some(score) = pat.indices(h, &mut matcher, &mut indices) {
            scored.push((p.clone(), score, indices));
        }
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
}

use std::collections::HashSet;
use std::process::Command;

/// Turn `rg --files-with-matches` stdout into absolute, de-duplicated
/// paths. Relative lines are joined onto `root`; blank lines dropped.
pub fn parse_rg(stdout: &str, root: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let p = Path::new(line);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        if seen.insert(abs.clone()) {
            out.push(abs);
        }
    }
    out
}

/// Run ripgrep for files containing `query` under `root`. Errors carry a
/// user-facing message (rg missing, or no matches).
///
/// ponytail: blocks the UI thread for the rg run. rg is fast; on a huge
/// tree this is a brief freeze. Move to a worker thread + channel if it bites.
pub fn content_search(root: &Path, query: &str) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("rg")
        .arg("--files-with-matches")
        .arg("--color=never")
        .arg("--")
        .arg(query)
        .current_dir(root)
        .output()
        .map_err(|e| format!("ripgrep (rg) not found: {e}"))?;
    // rg exit code 1 == no matches (not an error for us).
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "rg failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let paths = parse_rg(&String::from_utf8_lossy(&output.stdout), root);
    if paths.is_empty() {
        return Err("no matches".to_string());
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn fixture() -> tempfile::TempDir {
        let d = tempdir().unwrap();
        fs::create_dir(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/main.rs"), "").unwrap();
        fs::write(d.path().join("README.md"), "").unwrap();
        fs::write(d.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(d.path().join("ignored.txt"), "").unwrap();
        d
    }

    #[test]
    fn parse_rg_joins_relative_and_dedups_absolute() {
        let root = Path::new("/proj");
        let out = "src/main.rs\nsrc/main.rs\n/proj/README.md\n\n";
        let got = parse_rg(out, root);
        assert_eq!(
            got,
            vec![
                PathBuf::from("/proj/src/main.rs"),
                PathBuf::from("/proj/README.md"),
            ]
        );
    }

    #[test]
    fn collect_respects_gitignore_by_default() {
        let d = fixture();
        let got = collect(d.path(), false);
        let names: Vec<_> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn collect_all_includes_ignored() {
        let d = fixture();
        let got = collect(d.path(), true);
        let names: Vec<_> = got
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn rank_orders_better_match_first() {
        let d = fixture();
        let cands = collect(d.path(), false);
        let r = rank(&cands, d.path(), "mainrs");
        assert!(r[0].0.ends_with("main.rs"));
        assert!(!r[0].2.is_empty()); // has highlight indices
    }
}
