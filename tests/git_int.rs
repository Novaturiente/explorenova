use explorenova::git::scan_status;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {:?} failed", args);
}

#[test]
fn untracked_and_modified_are_reported() {
    let d = tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    fs::write(d.path().join("tracked.txt"), "v1").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "init"]);
    fs::write(d.path().join("tracked.txt"), "v2").unwrap();
    fs::write(d.path().join("new.txt"), "n").unwrap();

    let map = scan_status(d.path()).unwrap();
    use explorenova::tree::GitStatus;
    assert_eq!(map.get(&d.path().join("tracked.txt")), Some(&GitStatus::Modified));
    assert_eq!(map.get(&d.path().join("new.txt")), Some(&GitStatus::Untracked));
}
