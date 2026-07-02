use std::ffi::OsStr;
use std::fmt::Display;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use color_eyre::eyre::{Context, OptionExt};
use std::fs::Metadata;
use tokio::fs::DirEntry;
use tokio::io;
use tokio::sync::Semaphore;
use tracing::debug;

/// Limit concurrent fs access so we do not exceed the open file handle limit.
#[derive(Clone)]
pub struct ThrottledFs {
    file_limit: Arc<Semaphore>,
    pub(crate) user: u32,
    pub(crate) group: u32,
}

impl ThrottledFs {
    // #[cfg(test_support)]
    pub fn for_testing() -> Result<ThrottledFs, color_eyre::eyre::Error> {
        Self::new(uzers::get_current_uid(), uzers::get_current_gid())
    }
    pub fn new(user: u32, group: u32) -> color_eyre::Result<Self> {
        let limit_plus_one = rlimit::Resource::NOFILE
            .get_soft()
            .wrap_err("Could not get max number of file handles form the OS")?;
        let limit = limit_plus_one
            .checked_sub(10) // I know makes now sense but mrrow :3
            .ok_or_eyre("OS file handle limit too low")?
            .try_into()
            .expect("file limit cannot be larger then usize");
        Ok(Self {
            file_limit: Arc::new(Semaphore::new(limit)),
            user,
            group,
        })
    }

    pub async fn read_to_string(&self, path: impl AsRef<Path>) -> io::Result<String> {
        let _permit = self.file_limit.acquire().await;
        tokio::fs::read_to_string(path.as_ref()).await
    }

    pub async fn read_dir(&self, dir: impl AsRef<Dir>) -> io::Result<tokio::fs::ReadDir> {
        let _permit = self.file_limit.acquire().await;
        tokio::fs::read_dir(&dir.as_ref().0).await
    }

    pub async fn metadata(&self, path: impl AsRef<Path>) -> io::Result<Metadata> {
        tokio::fs::metadata(path).await
    }

    pub async fn symlink(
        &self,
        original: impl AsRef<Path>,
        link: impl AsRef<Path>,
    ) -> io::Result<()> {
        debug!(
            "Creating symlink: {} -> {}",
            link.as_ref().display(),
            original.as_ref().display()
        );
        tokio::fs::symlink(original, &link).await?;
        std::os::unix::fs::lchown(link, Some(self.user), Some(self.group))
    }
}

pub struct DirName(pub std::ffi::OsString);

impl AsRef<Path> for DirName {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}

macro_rules! dir_wrapper {
    ($name:ident, $wraps:ident) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(pub $wraps);

        impl $name {
            #[allow(dead_code)]
            pub fn display(&self) -> std::path::Display<'_> {
                self.0.display()
            }
            pub fn subdir(&self, dir: &DirName) -> Self {
                Self(self.0.subdir(dir))
            }
        }
        impl AsRef<$wraps> for $name {
            fn as_ref(&self) -> &$wraps {
                &self.0
            }
        }
        impl AsRef<$name> for $name {
            fn as_ref(&self) -> &$name {
                &self
            }
        }
        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                &self.0.as_ref()
            }
        }
        impl AsRef<OsStr> for $name {
            fn as_ref(&self) -> &OsStr {
                &self.0.0.as_ref()
            }
        }
    };
}
dir_wrapper! {TargetDir, Dir}
dir_wrapper! {SourceDir, Dir}

dir_wrapper! {BaseTargetDir, TargetDir}
dir_wrapper! {BaseSourceDir, SourceDir}

impl FromStr for BaseTargetDir {
    type Err = <PathBuf as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PathBuf::from_str(s)
            .map(Dir)
            .map(TargetDir)
            .map(BaseTargetDir)
    }
}

impl FromStr for BaseSourceDir {
    type Err = <PathBuf as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PathBuf::from_str(s)
            .map(Dir)
            .map(SourceDir)
            .map(BaseSourceDir)
    }
}

impl From<BaseTargetDir> for TargetDir {
    fn from(base: BaseTargetDir) -> Self {
        base.0
    }
}

impl From<BaseSourceDir> for SourceDir {
    fn from(base: BaseSourceDir) -> Self {
        base.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Not a subdir of the base target dir")]
pub struct NotBaseSubDir;

impl TargetDir {
    pub fn try_new(path: impl AsRef<Path>, base: &BaseTargetDir) -> Result<Self, NotBaseSubDir> {
        let path = path.as_ref();
        if path.starts_with(base) {
            Ok(Self(Dir(path.to_path_buf())))
        } else {
            Err(NotBaseSubDir)
        }
    }
    pub fn relative_to_base(&self, base: &BaseTargetDir) -> &Path {
        self.0
            .0
            .strip_prefix(&base.0.0)
            .expect("There is only one base target dir and all target dirs have it as prefix")
    }
}

macro_rules! path_wrapper {
    ($(#[$docs:meta])? $name:ident) => {
        #[derive(
            Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
        )]
        pub struct $name(pub PathBuf);

        impl $name {
            #[allow(dead_code)]
            pub fn display(&self) -> std::path::Display<'_> {
                self.0.display()
            }
            #[allow(dead_code)]
            pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
                self.0.join(path)
            }
        }
        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                &self.0
            }
        }
        impl AsRef<OsStr> for $name {
            fn as_ref(&self) -> &OsStr {
                &self.0.as_ref()
            }
        }
        impl Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_fmt(format_args!("{}", self.display()))
            }
        }
    };
}

path_wrapper! {RawFile}
path_wrapper! {PreviewFile}
path_wrapper! {XmpFile}
path_wrapper! {
    /// A directory that is not the root
    Dir
}

impl Dir {
    pub fn subdir(&self, dir: &DirName) -> Self {
        Self(self.0.join(dir))
    }
}

impl RawFile {
    pub fn preview_file(&self) -> PreviewFile {
        PreviewFile(self.0.with_extension("jpg"))
    }
}

impl PreviewFile {
    pub fn file_stem(&self) -> &OsStr {
        self.0
            .file_stem()
            .expect("A preview has a file name so a link to it has one too")
    }
    /// something.NEF.xmp
    pub fn xmp_path(&self, source: &SourceDir) -> XmpFile {
        XmpFile(
            source
                .0
                .0
                .join(self.file_stem())
                .with_added_extension("NEF")
                .with_added_extension("xmp"),
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Not path ending in jpg")]
pub struct NotAPreviewFile;

impl TryFrom<DirEntry> for PreviewFile {
    type Error = NotAPreviewFile;

    fn try_from(entry: DirEntry) -> Result<Self, Self::Error> {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jpg") {
            Ok(Self(path))
        } else {
            Err(NotAPreviewFile)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Not a path ending in xmp")]
pub struct NotAnXmpFile;

impl TryFrom<DirEntry> for XmpFile {
    type Error = NotAnXmpFile;

    fn try_from(entry: DirEntry) -> Result<Self, Self::Error> {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "xmp") {
            Ok(Self(path))
        } else {
            Err(NotAnXmpFile)
        }
    }
}

impl XmpFile {
    pub fn preview_path(&self, source: impl AsRef<SourceDir>) -> PreviewFile {
        // TODO wtf refactor this
        let mut xmp_path = self.0.with_extension("");
        xmp_path.set_extension("");
        let name = xmp_path.file_name().expect("DirEntry has a file name");

        let preview = source.as_ref().0.0.join(name).with_extension("jpg");
        PreviewFile(preview)
    }

    /// Includes the extension
    pub fn file_stem(&self) -> &OsStr {
        self.0.file_stem().expect("A raw file always has a name")
    }
}

pub trait MetadataExtExt {
    fn anyone_can_read(&self) -> bool;
    fn anyone_can_write(&self) -> bool;
    fn user_can_read(&self, user_id: u32) -> bool;
    fn user_can_write(&self, user_id: u32) -> bool;
    fn group_can_read(&self, group_id: u32) -> bool;
    fn group_can_write(&self, group_id: u32) -> bool;
}

impl MetadataExtExt for Metadata {
    fn anyone_can_read(&self) -> bool {
        self.mode() & 0o004 == 0o004
    }

    fn anyone_can_write(&self) -> bool {
        self.mode() & 0o002 == 0o002
    }

    fn user_can_read(&self, user_id: u32) -> bool {
        self.anyone_can_read() || (self.mode() & 0o400 == 0o400 && self.uid() == user_id)
    }

    fn user_can_write(&self, user_id: u32) -> bool {
        self.anyone_can_write() || (self.mode() & 0o200 == 0o200 && self.uid() == user_id)
    }

    fn group_can_read(&self, group_id: u32) -> bool {
        self.anyone_can_read() || (self.mode() & 0o040 == 0o040 && self.gid() == group_id)
    }

    fn group_can_write(&self, group_id: u32) -> bool {
        self.anyone_can_write() || (self.mode() & 0o020 == 0o020 && self.gid() == group_id)
    }
}
