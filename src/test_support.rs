use std::fs;
use std::path::Path;

use tempfile::TempDir;

const RATED: &str = "rated_picture"; // needs to match test file
const SUBDIR: &str = "some_event/some_day";
const RATED_PREVIEW_JPEG_CONTENT: &str = "this is totally a preview jpg of a rated photo /s.";
const RATED_RAW_CONTENT: &str = "this is a raw photo that is rated, I swear! /s.";

pub fn test_dir_with_rated() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join(SUBDIR);
    fs::create_dir_all(&subdir).unwrap();

    fs::write(
        subdir.join(RATED).with_extension("jpg"),
        RATED_PREVIEW_JPEG_CONTENT,
    )
    .unwrap();

    fs::write(
        subdir.join(RATED).with_extension("NEF"), // needs to match xmp file content
        RATED_RAW_CONTENT,
    )
    .unwrap();

    fs::write(
        subdir
            .join(RATED)
            .with_extension("NEF")
            .with_added_extension("xmp"),
        include_str!("test_support/rated_picture.xmp"),
    )
    .unwrap();

    dir
}

pub fn empty_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

pub fn assert_rated_symlinked(dir: &Path) {
    let file = dir.join(SUBDIR).join(RATED).with_extension("jpg");
    let rated_meta = fs::symlink_metadata(&file).expect("After running there should be a symlink");

    assert!(rated_meta.file_type().is_symlink());

    let symlink_target = fs::canonicalize(file).unwrap();
    assert!(symlink_target.is_file());
    assert!(fs::read_to_string(symlink_target).unwrap() == RATED_PREVIEW_JPEG_CONTENT);
}
