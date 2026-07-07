use color_eyre::eyre::Context;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Once};
use tempfile::TempDir;
use tokio::sync::{Notify, mpsc};
use tracing::info;

use crate::fs::{Dir, DirName, PreviewFile, RawFile, SourceDir, TargetDir, XmpFile};
use crate::watcher::EyreWithPath;
use crate::xmp::Rating;
use crate::{BaseSourceDir, BaseTargetDir, ImageExporter, Watcher};

pub use crate::watcher;

/// an initially rated picture
pub fn test_subdir() -> DirName {
    DirName(OsStr::new("some_event/some_day").to_owned())
}
const PREVIEW_JPEG_CONTENT: &str = "this is totally a preview jpg of a rated raw /s.";
const RATED_RAW_CONTENT: &str = "this is a raw photo that is rated, I swear! /s.";
const UNRATED_RAW_CONTENT: &str = "this is a raw photo that is not rated, I swear! /s.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestFile {
    A,
}

impl TestFile {
    const fn name(self) -> &'static str {
        match self {
            TestFile::A => "a",
        }
    }
    pub fn xmp_file(self, source: impl AsRef<SourceDir>) -> XmpFile {
        XmpFile(
            source
                .as_ref()
                .0
                .0
                .join(self.name())
                .with_added_extension("NEF")
                .with_added_extension("xmp"),
        )
    }
    fn jpg_preview(self, target: &TargetDir) -> PreviewFile {
        PreviewFile(target.0.0.join(self.name()).with_added_extension("jpg"))
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
}

impl SourceDirBuilder {
    #[must_use]
    pub fn with_rated(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.rated.extend(files);
        self
    }
    #[must_use]
    pub fn with_unrated(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.unrated.extend(files);
        self
    }
    #[must_use]
    pub fn build(self) -> (TempDir, BaseSourceDir) {
        assert_eq!(self.unrated.intersection(&self.rated).count(), 0);

        let dir = tempfile::tempdir().unwrap();
        let base_source = BaseSourceDir(SourceDir(Dir(dir.path().to_path_buf())));
        let source = base_source.subdir(&test_subdir());

        fs::create_dir_all(&source).unwrap();

        for test_file in self.unrated.union(&self.rated) {
            add_file(*test_file, Rating::Unrated, &source);
        }
        for test_file in self.rated.union(&self.unrated) {
            add_file(*test_file, Rating::Four, &source);
        }

        (dir, base_source)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TargetDirBuilder {
    preview: HashSet<TestFile>,
}

impl TargetDirBuilder {
    #[must_use]
    pub fn with_preview(mut self, files: impl IntoIterator<Item = TestFile>) -> Self {
        self.preview.extend(files);
        self
    }

    #[must_use]
    pub fn build(self) -> (TempDir, BaseTargetDir) {
        let dir = tempfile::tempdir().unwrap();
        let base_target = BaseTargetDir(TargetDir(Dir(dir.path().to_path_buf())));
        let target = base_target.subdir(&test_subdir());

        fs::create_dir_all(&target).unwrap();

        for test_file in self.preview {
            fs::write(test_file.jpg_preview(&target), PREVIEW_JPEG_CONTENT).unwrap();
        }

        (dir, base_target)
    }
}

#[track_caller]
pub fn assert_preview_in_place(target: &BaseTargetDir, test_file: TestFile) {
    let target = target.subdir(&test_subdir());
    let file = test_file.jpg_preview(target.as_ref());
    assert!(&file.0.is_file());
    let content = fs::read_to_string(file).unwrap();
    assert_eq!(content, PREVIEW_JPEG_CONTENT);
}

#[track_caller]
pub fn assert_preview_missing(target: &BaseTargetDir, test_file: TestFile) {
    let target = target.subdir(&test_subdir());
    let file = test_file.jpg_preview(target.as_ref());
    assert!(!file.0.exists());
}

pub fn remove_rating(source: &SourceDir, test_file: TestFile) {
    add_file(test_file, Rating::Unrated, source);
}

pub fn add_rating(source: &SourceDir, test_file: TestFile) {
    add_file(test_file, Rating::Four, source);
}

pub fn add_file(file: TestFile, rating: Rating, source: impl AsRef<SourceDir>) {
    info!("adding test file: {file:?}, with rating: {rating:?}");
    std::fs::write(
        // needs to match xmp file content
        file.xmp_file(&source).raw_file(),
        if rating.is_rated() {
            RATED_RAW_CONTENT
        } else {
            UNRATED_RAW_CONTENT
        },
    )
    .unwrap();
    std::fs::write(
        file.xmp_file(source),
        include_str!("../tests/assets/small_raw.NEF.xmp")
            .replace("<FILENAME>", file.name())
            .replace("<RATING>", &dbg!(dbg!(rating).number()).to_string()),
    )
    .unwrap();
}

pub fn remove_file(file: TestFile, source: impl AsRef<SourceDir>) {
    std::fs::remove_file(file.xmp_file(&source)).unwrap();
    std::fs::remove_file(file.xmp_file(&source).raw_file()).unwrap();
}

/// Puts a file ending in `.jpg` next to the raw file.
pub struct TestExporter;

impl ImageExporter for TestExporter {
    async fn export(
        _: &XmpFile,
        _: &RawFile,
        output_file: &PreviewFile,
        fs: &crate::fs::ThrottledFs,
    ) -> color_eyre::Result<()> {
        std::fs::write(output_file, PREVIEW_JPEG_CONTENT)
            .wrap_err("Failed to write fake jpeg")
            .note_path(output_file)?;
        std::os::unix::fs::chown(output_file, Some(fs.user), Some(fs.group))
            .wrap_err("Failed to set user and group for fake jpg file")
    }
}

pub fn test_setup() {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    static INIT_ERR_REPORTING: Once = Once::new();

    INIT_ERR_REPORTING.call_once(|| {
        color_eyre::install().unwrap();
        tracing_subscriber::registry()
            .with(fmt::layer().pretty())
            .with(LevelFilter::TRACE)
            .with(ErrorLayer::default())
            .init();
    });
    info!("Test started");
}

pub struct TestWatcher {
    in_next: Arc<Notify>,
    rx: mpsc::Receiver<watcher::Event>,
}

pub struct TestWatcherHandle {
    in_next: Arc<Notify>,
    tx: mpsc::Sender<watcher::Event>,
}

impl TestWatcher {
    pub fn new() -> (TestWatcherHandle, Self) {
        let (tx, rx) = mpsc::channel(5);
        let in_next = Arc::new(Notify::new());
        (
            TestWatcherHandle {
                in_next: Arc::clone(&in_next),
                tx,
            },
            Self { in_next, rx },
        )
    }
}

impl TestWatcherHandle {
    pub async fn wait_till_in_next(&self) {
        self.in_next.notified().await;
    }
    pub fn send_file_modified(&self, test_file: TestFile, source: &SourceDir) {
        self.tx
            .try_send(watcher::Event {
                xmp_file: test_file.xmp_file(source),
                kind: watcher::EventKind::FileModificationComplete,
            })
            .unwrap()
    }

    pub fn send_file_added(&self, test_file: TestFile, source: &SourceDir) {
        self.tx
            .try_send(watcher::Event {
                xmp_file: test_file.xmp_file(source),
                // Note: we do not care about the file being created
                // we want when the file is ready for reading.
                kind: watcher::EventKind::FileModificationComplete,
            })
            .unwrap()
    }
    pub fn send_file_removed(&self, test_file: TestFile, source: &SourceDir) {
        self.tx
            .try_send(watcher::Event {
                xmp_file: test_file.xmp_file(source),
                kind: watcher::EventKind::FileDeleted,
            })
            .unwrap()
    }
}

impl Watcher for TestWatcher {
    fn clear(&mut self) {}

    async fn next(&mut self) -> crate::watcher::Event {
        self.in_next.notify_one();
        self.rx.recv().await.unwrap()
    }

    fn overflown(&self) -> bool {
        false
    }
}
