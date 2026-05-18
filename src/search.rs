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
