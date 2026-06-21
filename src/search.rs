use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use std::path::{Path, PathBuf};

/// Max entries / depth a search-or-filter walk will read before stopping.
/// ponytail: the walk runs on the UI thread, so an unbounded walk of a huge
/// tree freezes the app. These caps keep the freeze short; matches deeper
/// than `WALK_MAX_DEPTH` or past `WALK_MAX_ENTRIES` are missed until search
/// moves off-thread.
const WALK_MAX_ENTRIES: usize = 50_000;
const WALK_MAX_DEPTH: usize = 32;

/// Collect candidate paths under `root`. `include_all` => ignore .gitignore + show hidden.
/// Capped (depth + entry count) so a giant tree can't freeze the UI thread.
pub fn collect(root: &Path, include_all: bool) -> Vec<PathBuf> {
    collect_capped(root, include_all, WALK_MAX_ENTRIES, WALK_MAX_DEPTH)
}

/// Like [`collect`] with explicit caps (exposed for testing). Stops after
/// `max_entries` results and never descends past `max_depth`.
pub fn collect_capped(
    root: &Path,
    include_all: bool,
    max_entries: usize,
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut wb = WalkBuilder::new(root);
    wb.hidden(!include_all)
        .git_ignore(!include_all)
        .git_global(!include_all)
        .git_exclude(!include_all)
        .require_git(false)
        .max_depth(Some(max_depth));
    let mut out = Vec::new();
    for d in wb.build().filter_map(Result::ok) {
        let p = d.into_path();
        if p == root {
            continue;
        }
        out.push(p);
        if out.len() >= max_entries {
            break;
        }
    }
    out
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
        // Bound the walk so a huge tree doesn't freeze the UI thread (rg
        // runs synchronously here). ponytail: matches deeper than this are
        // missed until content search moves off-thread.
        .arg("--max-depth")
        .arg(WALK_MAX_DEPTH.to_string())
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
    fn collect_capped_respects_depth_and_entry_caps() {
        let d = fixture(); // root has README.md, src/ (depth 1) and src/main.rs (depth 2)
        // Depth cap 1: src/main.rs (depth 2) must be excluded; src/ (depth 1) kept.
        let shallow = collect_capped(d.path(), true, 10_000, 1);
        assert!(shallow.iter().any(|p| p.ends_with("src")), "depth-1 entry kept");
        assert!(
            !shallow.iter().any(|p| p.ends_with("main.rs")),
            "depth-2 entry excluded by depth cap"
        );
        // Entry cap: never returns more than the cap.
        let capped = collect_capped(d.path(), true, 2, 32);
        assert!(capped.len() <= 2, "entry cap bounds the result count");
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
