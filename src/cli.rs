use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "explorenova", version, about = "Terminal file manager")]
pub struct Args {
    /// Directory to start in
    pub path: Option<PathBuf>,
    /// Launch with the tree pane only (bookmarks + preview hidden this session)
    #[arg(short = 't', long = "tree-only")]
    pub tree_only: bool,
}

/// Resolve start dir. Precedence: explicit arg (with `~`/`$VAR` expansion),
/// then the launch directory (`$PWD`), then persisted `last_cwd` (last
/// resort, only when `$PWD` is not a usable directory).
pub fn resolve_start_dir(
    arg: Option<PathBuf>,
    last_cwd: Option<PathBuf>,
    pwd: PathBuf,
) -> PathBuf {
    if let Some(p) = arg {
        let expanded = PathBuf::from(crate::nav::expand(&p.to_string_lossy()));
        if expanded.is_dir() {
            return expanded;
        }
    }
    if pwd.is_dir() {
        return pwd;
    }
    if let Some(p) = last_cwd {
        if p.is_dir() {
            return p;
        }
    }
    pwd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_wins_when_dir_exists() {
        let d = std::env::temp_dir();
        let got = resolve_start_dir(Some(d.clone()), None, PathBuf::from("/nope"));
        assert_eq!(got, d);
    }

    #[test]
    fn falls_back_to_pwd_when_nothing_valid() {
        let pwd = std::env::temp_dir();
        let got = resolve_start_dir(
            Some(PathBuf::from("/does/not/exist")),
            Some(PathBuf::from("/also/not")),
            pwd.clone(),
        );
        assert_eq!(got, pwd);
    }

    #[test]
    fn pwd_wins_over_last_cwd_when_no_arg() {
        // Regression: launching `cd ~/X && explorenova` must open X,
        // not the persisted last_cwd.
        let pwd = std::env::temp_dir();
        let got = resolve_start_dir(None, Some(PathBuf::from("/")), pwd.clone());
        assert_eq!(got, pwd);
    }

    #[test]
    fn tilde_arg_is_expanded() {
        let home = std::env::temp_dir();
        std::env::set_var("HOME", &home);
        let got = resolve_start_dir(Some(PathBuf::from("~")), None, PathBuf::from("/nope"));
        assert_eq!(got, home);
    }

    #[test]
    fn parses_tree_only_flag() {
        let a = Args::try_parse_from(["explorenova", "-t"]).unwrap();
        assert!(a.tree_only);
    }
}
