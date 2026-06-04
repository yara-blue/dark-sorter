use std::fs;
use std::path::Path;

use tempfile::TempDir;

const RATED: &str = "rated_picture";
const SUBDIR: &str = "some_even/some_day";
const RATED_JPEG_CONTENT: &str = "this is a rated photo I swear /s.";

pub fn test_dir_with_rated() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join(SUBDIR);
    fs::create_dir_all(&subdir).unwrap();

    fs::write(
        subdir.join(RATED).with_extension(".jpg"),
        RATED_JPEG_CONTENT,
    )
    .unwrap();
    fs::write(
        subdir
            .join(RATED)
            .with_extension(".jpg")
            .with_added_extension(".xmp"),
        include_str!("test_support/rated_picture.xmp"),
    )
    .unwrap();

    dir
}

pub fn empty_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

pub fn assert_rated_symlinked(dir: &Path) {
    let subdir = dir.join(SUBDIR);
    let file = subdir.join(RATED).with_extension("jpg");
    let rated_meta = fs::metadata(subdir.join(RATED).with_extension("jpg"))
        .expect("After running there should be a symlink");

    assert!(rated_meta.file_type().is_symlink());

    let symlink_target = fs::canonicalize(file).unwrap();
    assert!(symlink_target.is_file());
    assert!(fs::read_to_string(symlink_target).unwrap() == RATED_JPEG_CONTENT);
}
