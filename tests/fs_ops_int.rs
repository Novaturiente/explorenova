use explorenova::fs_ops::{create, permanent_delete, rename};
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
fn rename_changes_name() {
    let d = tempdir().unwrap();
    let p = d.path().join("old.txt");
    fs::write(&p, "1").unwrap();
    let np = rename(&p, "new.txt").unwrap();
    assert!(np.exists());
    assert!(!p.exists());
}

#[test]
fn rename_refuses_existing_target() {
    let d = tempdir().unwrap();
    let a = d.path().join("a.txt");
    let b = d.path().join("b.txt");
    fs::write(&a, "1").unwrap();
    fs::write(&b, "2").unwrap();
    let r = rename(&a, "b.txt");
    assert!(r.is_err(), "renaming onto an existing name must error");
    assert_eq!(fs::read_to_string(&b).unwrap(), "2", "target must be untouched");
}

#[test]
fn rename_refuses_path_separator() {
    let d = tempdir().unwrap();
    let p = d.path().join("a.txt");
    fs::write(&p, "1").unwrap();
    assert!(rename(&p, "sub/a.txt").is_err());
    assert!(rename(&p, "").is_err());
    assert!(rename(&p, "..").is_err());
}

#[cfg(unix)]
#[test]
fn pasting_a_symlink_to_a_file_keeps_it_a_symlink() {
    // `src.is_dir()` follows links, so a selected symlink-to-file used to
    // take the plain-file branch: the copy opened the TARGET and wrote its
    // contents into a regular file, and on a cross-device move the source
    // link was then deleted.
    use explorenova::fs_ops::PasteMode;
    use std::os::unix::fs::symlink;
    let d = tempdir().unwrap();
    let target = d.path().join("target.txt");
    fs::write(&target, "payload").unwrap();
    let link = d.path().join("link.txt");
    symlink(&target, &link).unwrap();
    let dst_dir = d.path().join("dst");
    fs::create_dir(&dst_dir).unwrap();

    let mut op = explorenova::ops::spawn_paste(vec![link], dst_dir.clone(), PasteMode::Copy);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while op.finished.is_none() && std::time::Instant::now() < deadline {
        op.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(op.finished.is_some(), "paste never finished");

    let copied = dst_dir.join("link.txt");
    let meta = fs::symlink_metadata(&copied).expect("destination entry");
    assert!(meta.file_type().is_symlink(), "the copy must still be a symlink");
    assert_eq!(fs::read_link(&copied).unwrap(), target);
}
