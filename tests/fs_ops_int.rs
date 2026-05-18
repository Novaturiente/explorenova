use explorenova::fs_ops::{create, paste, rename, trash_path, PasteMode};
use std::fs;
use tempfile::tempdir;

#[test]
fn create_file_and_nested_dir() {
    let d = tempdir().unwrap();
    create(&d.path().join("a/b/c.txt")).unwrap();
    assert!(d.path().join("a/b/c.txt").is_file());
    create(&d.path().join("x/y/")).unwrap();
    assert!(d.path().join("x/y").is_dir());
}

#[test]
fn paste_copy_collision_gets_suffix() {
    let d = tempdir().unwrap();
    let src = d.path().join("f.txt");
    fs::write(&src, "1").unwrap();
    let dst_dir = d.path().join("dst");
    fs::create_dir(&dst_dir).unwrap();
    fs::write(dst_dir.join("f.txt"), "old").unwrap();
    let out = paste(&src, &dst_dir, PasteMode::Copy).unwrap();
    assert_eq!(out.file_name().unwrap(), "f (1).txt");
    assert!(d.path().join("f.txt").exists()); // copy keeps source
}

#[test]
fn paste_move_removes_source() {
    let d = tempdir().unwrap();
    let src = d.path().join("m.txt");
    fs::write(&src, "1").unwrap();
    let dst = d.path().join("dst");
    fs::create_dir(&dst).unwrap();
    let out = paste(&src, &dst, PasteMode::Move).unwrap();
    assert!(out.exists());
    assert!(!src.exists());
}

#[test]
fn rename_changes_name() {
    let d = tempdir().unwrap();
    let p = d.path().join("old.txt");
    fs::write(&p, "1").unwrap();
    let np = rename(&p, "new.txt").unwrap();
    assert!(np.exists());
    assert!(!p.exists());
}
