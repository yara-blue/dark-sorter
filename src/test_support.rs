use color_eyre::Section;
use color_eyre::eyre::{Context, eyre};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, Once};
use tempfile::TempDir;

use crate::ImageExporter;
use crate::fs::{Dir, SourceDir, TargetDir, XmpFile};
use crate::watcher::EyreWithPath;
use crate::xmp::Xmp;

/// an initially rated picture
const SUBDIR: &str = "some_event/some_day";
const RATED_PREVIEW_JPEG_CONTENT: &str = "this is totally a preview jpg of a rated photo /s.";
const RATED_RAW_CONTENT: &str = "this is a raw photo that is rated, I swear! /s.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestFile {
    A,
}

impl TestFile {
    fn name(self) -> &'static str {
        match self {
            TestFile::A => "a",
        }
    }
}

impl AsRef<Path> for TestFile {
    fn as_ref(&self) -> &Path {
        Path::new(self.name())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceDirBuilder {
    rated: HashSet<TestFile>,
    unrated: HashSet<TestFile>,
    preview: HashSet<TestFile>,
}

impl SourceDirBuilder {
    #[must_use]
    pub fn with_rated(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.rated.extend(files);
        self
    }
    #[must_use]
    pub fn with_preview(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.preview.extend(files);
        self
    }
    #[must_use]
    pub fn with_unrated(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.unrated.extend(files);
        self
    }
    #[must_use]
    pub fn build(self) -> (TempDir, SourceDir) {
        assert_eq!(self.unrated.intersection(&self.rated).count(), 0);

        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join(SUBDIR);
        let source = SourceDir(Dir(dir.path().to_path_buf()));
        fs::create_dir_all(&subdir).unwrap();

        for test_file in self.unrated.union(&self.rated) {
            fs::write(
                subdir.join(test_file).with_extension("NEF"), // needs to match xmp file content
                RATED_RAW_CONTENT,
            )
            .unwrap();
        }
        for test_file in self.rated.union(&self.unrated) {
            fs::write(
                subdir
                    .join(test_file)
                    .with_extension("NEF")
                    .with_added_extension("xmp"),
                include_str!("test_support/rated_picture.xmp")
                    .replace("<FILE_NAME>", test_file.name()),
            )
            .unwrap();
        }
        for test_file in self.unrated {
            remove_rating(&source, test_file);
        }
        for test_file in self.preview {
            fs::write(
                subdir.join(test_file).with_extension("jpg"),
                RATED_PREVIEW_JPEG_CONTENT,
            )
            .unwrap();
        }

        (dir, source)
    }
}

#[must_use]
pub fn empty_dir() -> (TempDir, TargetDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    (dir, TargetDir(Dir(path)))
}

pub fn assert_symlinked(dir: &TargetDir, test_file: TestFile) {
    let file = dir.join(SUBDIR).join(test_file).with_extension("jpg");
    let rated_meta = fs::symlink_metadata(&file).expect("After running there should be a symlink");

    assert!(rated_meta.file_type().is_symlink());

    let symlink_target = fs::canonicalize(dbg!(file)).unwrap();

    assert!(symlink_target.is_file());
    assert!(fs::read_to_string(symlink_target).unwrap() == RATED_PREVIEW_JPEG_CONTENT);
}

pub fn assert_not_symlinked(dir: &TargetDir, test_file: TestFile) {
    let file = dir.join(SUBDIR).join(test_file).with_extension("jpg");
    let res = fs::symlink_metadata(&file).unwrap_err();
    assert_eq!(res.kind(), ErrorKind::NotFound);
}

pub fn remove_rating(source: &SourceDir, test_file: TestFile) {
    fs::write(
        source
            .join(SUBDIR)
            .join(test_file)
            .with_extension("NEF")
            .with_added_extension("xmp"),
        include_str!("test_support/rated_picture.xmp")
            .replace("xmp:Rating=\"3\"", "xmp:Rating=\"0\""),
    )
    .unwrap();
}

pub fn add_rating(source: &SourceDir, test_file: TestFile) {
    fs::write(
        source
            .join(SUBDIR)
            .join(test_file)
            .with_extension("NEF")
            .with_added_extension("xmp"),
        include_str!("test_support/rated_picture.xmp"),
    )
    .unwrap();
}

/// Puts a file ending in `.jpg` next to the raw file.
pub struct FakeJpgExporter;

impl ImageExporter for FakeJpgExporter {
    async fn export(
        xmp: &Xmp,
        _: &XmpFile,
        source: &SourceDir,
        fs: &crate::fs::ThrottledFs,
    ) -> color_eyre::Result<()> {
        let input_file = source.join(&*xmp.raw);
        let output_file = input_file.with_extension("jpg");
        std::fs::write(&output_file, RATED_PREVIEW_JPEG_CONTENT)
            .wrap_err("Failed to write fake jpeg")
            .note_path(&output_file)?;
        std::os::unix::fs::chown(output_file, Some(fs.user), Some(fs.group))
            .wrap_err("Failed to set user and group for fake jpg file")
    }
}

pub fn single_threaded_sudo_test_setup() -> MutexGuard<'static, ()> {
    static FORCE_SINGLE_THREADED: Mutex<()> = Mutex::new(());

    let _ = color_eyre::install();

    if caps::has_cap(
        None,
        caps::CapSet::Permitted,
        caps::Capability::CAP_SYS_ADMIN,
    )
    .unwrap()
    {
        Ok(())
    } else {
        Err(eyre!("this test must be run using sudo")).with_suggestion(|| {
            format!(
                "try running: `sudo {}`",
                std::env::current_exe().unwrap().display()
            )
        })
    }
    .unwrap();

    match FORCE_SINGLE_THREADED.lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    }
}

pub fn test_setup() {
    static INIT_ERR_REPORTING: Once = Once::new();
    INIT_ERR_REPORTING.call_once(|| color_eyre::install().unwrap());
}
