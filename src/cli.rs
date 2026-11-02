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
    /// On quit, write the final directory to this file (for a shell
    /// wrapper that cd's there). See README "Shell integration".
    #[arg(long = "cwd-file")]
    pub cwd_file: Option<PathBuf>,
}

/// Resolve start dir. Precedence: explicit arg (with `~`/`$VAR` expansion),
/// then the launch directory (`$PWD`), then persisted `last_cwd` (last
/// resort, only when `$PWD` is not a usable directory).
///
/// The second element is a message for the status bar. An unusable path
/// argument used to fall through in silence, so `explorenova /typo` opened
/// the current directory and looked like it had simply ignored you.
pub fn resolve_start_dir(
    arg: Option<PathBuf>,
    last_cwd: Option<PathBuf>,
    pwd: PathBuf,
) -> (PathBuf, Option<String>) {
    let mut warning = None;
    if let Some(p) = arg {
        let expanded = PathBuf::from(crate::nav::expand(&p.to_string_lossy()));
        if expanded.is_dir() {
            return (expanded, None);
        }
        warning = Some(if expanded.is_file() {
            format!("{} is a file, not a directory", expanded.display())
        } else {
            format!("{} not found", expanded.display())
        });
    }
    if pwd.is_dir() {
        return (pwd, warning);
    }
    if let Some(p) = last_cwd {
        if p.is_dir() {
            return (p, warning);
        }
    }
    (pwd, warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_wins_when_dir_exists() {
        let d = std::env::temp_dir();
        let (got, warn) = resolve_start_dir(Some(d.clone()), None, PathBuf::from("/nope"));
        assert_eq!(got, d);
        assert!(warn.is_none());
    }

    #[test]
    fn falls_back_to_pwd_when_nothing_valid() {
        let pwd = std::env::temp_dir();
        let (got, warn) = resolve_start_dir(
            Some(PathBuf::from("/does/not/exist")),
            Some(PathBuf::from("/also/not")),
            pwd.clone(),
        );
        assert_eq!(got, pwd);
        assert!(warn.unwrap().contains("not found"), "a bad path must say so");
    }

    #[test]
    fn pwd_wins_over_last_cwd_when_no_arg() {
        // Regression: launching `cd ~/X && explorenova` must open X,
        // not the persisted last_cwd.
        let pwd = std::env::temp_dir();
        let (got, _) = resolve_start_dir(None, Some(PathBuf::from("/")), pwd.clone());
        assert_eq!(got, pwd);
    }

    #[test]
    fn tilde_arg_is_expanded() {
        let home = std::env::temp_dir();
        std::env::set_var("HOME", &home);
        let (got, _) = resolve_start_dir(Some(PathBuf::from("~")), None, PathBuf::from("/nope"));
        assert_eq!(got, home);
    }

    #[test]
    fn parses_tree_only_flag() {
        let a = Args::try_parse_from(["explorenova", "-t"]).unwrap();
        assert!(a.tree_only);
    }

    #[test]
    fn parses_cwd_file_flag() {
        let a = Args::try_parse_from(["explorenova", "--cwd-file", "/tmp/x"]).unwrap();
        assert_eq!(a.cwd_file, Some(PathBuf::from("/tmp/x")));
    }
}
