use explorenova::apps::apps_for;
use std::fs;
use tempfile::tempdir;

#[test]
fn apps_for_ranks_mime_match_first() {
    let d = tempdir().unwrap();
    let appdir = d.path().join("applications");
    fs::create_dir_all(&appdir).unwrap();
    fs::write(
        appdir.join("img.desktop"),
        "[Desktop Entry]\nType=Application\nName=ImgView\nExec=imgview %f\nMimeType=image/png;\n",
    )
    .unwrap();
    fs::write(
        appdir.join("zed.desktop"),
        "[Desktop Entry]\nType=Application\nName=Zed\nExec=zed %f\nMimeType=text/plain;\n",
    )
    .unwrap();

    std::env::set_var("XDG_DATA_HOME", d.path());
    std::env::set_var("XDG_DATA_DIRS", "");

    let got = apps_for(std::path::Path::new("photo.png"));
    let names: Vec<&str> = got.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"ImgView"));
    assert!(names.contains(&"Zed"));
    assert_eq!(names[0], "ImgView", "mime match must rank first");
}
