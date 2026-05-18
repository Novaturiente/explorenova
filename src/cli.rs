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

/// Resolve start dir: explicit arg > last cwd > $PWD.
pub fn resolve_start_dir(
    arg: Option<PathBuf>,
    last_cwd: Option<PathBuf>,
    pwd: PathBuf,
) -> PathBuf {
    if let Some(p) = arg {
        if p.is_dir() {
            return p;
        }
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
    fn parses_tree_only_flag() {
        let a = Args::try_parse_from(["explorenova", "-t"]).unwrap();
        assert!(a.tree_only);
    }
}
