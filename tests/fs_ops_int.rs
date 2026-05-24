use explorenova::fs_ops::{create, paste, permanent_delete, rename, PasteMode};
use std::fs;
use tempfile::tempdir;

#[test]
fn permanent_delete_removes_dir_with_contents() {
    let d = tempdir().unwrap();
    let sub = d.path().join("sub");
    fs::create_dir_all(sub.join("nested")).unwrap();
    fs::write(sub.join("a.txt"), "x").unwrap();
    fs::write(sub.join("nested/b.txt"), "y").unwrap();
    permanent_delete(&sub).unwrap();
    assert!(!sub.exists(), "dir and contents should be removed");
}

#[test]
fn permanent_delete_removes_file() {
    let d = tempdir().unwrap();
    let f = d.path().join("a.txt");
    fs::write(&f, "x").unwrap();
    permanent_delete(&f).unwrap();
    assert!(!f.exists());
}

#[test]
fn permanent_delete_nonexistent_path_errors() {
    let d = tempdir().unwrap();
    let nope = d.path().join("does_not_exist");
    let r = permanent_delete(&nope);
    assert!(r.is_err(), "deleting a missing path must error, not succeed");
}

#[cfg(unix)]
#[test]
fn permanent_delete_unlinks_symlink_not_target() {
    use std::os::unix::fs::symlink;
    let d = tempdir().unwrap();
    let target_dir = d.path().join("target");
    fs::create_dir(&target_dir).unwrap();
    fs::write(target_dir.join("keep.txt"), "x").unwrap();
    let link = d.path().join("link");
    symlink(&target_dir, &link).unwrap();
    permanent_delete(&link).unwrap();
    assert!(!link.exists(), "symlink itself should be removed");
    assert!(target_dir.exists(), "target dir must remain");
    assert!(
        target_dir.join("keep.txt").exists(),
        "target contents must be preserved"
    );
}

#[test]
fn create_file_and_nested_dir() {
    let d = tempdir().unwrap();
    create(&d.path().join("a/b/c.txt")).unwrap();
    assert!(d.path().join("a/b/c.txt").is_file());
    create(&d.path().join("x/y/")).unwrap();
    assert!(d.path().join("x/y").is_dir());
}

#[test]
fn create_treats_extensionless_name_as_directory() {
    // Per user preference: a name with no extension and no trailing
    // slash creates a folder. A name with an extension stays a file.
    let d = tempdir().unwrap();
    create(&d.path().join("notes")).unwrap();
    assert!(d.path().join("notes").is_dir());
    create(&d.path().join("readme.md")).unwrap();
    assert!(d.path().join("readme.md").is_file());
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
