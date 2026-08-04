use std::ffi::OsStr;
use std::fmt::Display;
use std::fs::Permissions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use color_eyre::eyre::{Context, OptionExt};
use std::fs::Metadata;
use tokio::fs::DirEntry;
use tokio::io;
use tokio::sync::Semaphore;

use crate::scan::preview;
use crate::watcher::EyreWithPath;

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

    pub async fn copy_file(
        &self,
        raw: &InputFile,
        preview: &PreviewFile,
    ) -> Result<(), color_eyre::eyre::Error> {
        tokio::fs::copy(raw, preview)
            .await
            .wrap_err("Failed to copy jpg source to target dir")
            .note_path(raw)
            .note_path(preview)?;
        self.take_ownership(preview)
    }

    pub fn take_ownership(&self, path: impl AsRef<Path>) -> color_eyre::Result<()> {
        std::os::unix::fs::chown(&path, Some(self.user), Some(self.group))
            .wrap_err("Could not change ownership to our user and group")
            .note_path(path)
    }

    pub async fn allow_anyone_read_owner_write(
        &self,
        path: impl AsRef<Path>,
    ) -> color_eyre::Result<()> {
        tokio::fs::set_permissions(&path, Permissions::from_mode(0o644))
            .await
            .wrap_err(
                "failed to set permissions to allow anyone to \
                read and the owner to also write",
            )
            .note_path(path)
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

impl BaseTargetDir {
    pub fn subdir(&self, dir: &DirName) -> TargetDir {
        self.0.subdir(dir)
    }
}
impl BaseSourceDir {
    pub fn subdir(&self, dir: &DirName) -> SourceDir {
        self.0.subdir(dir)
    }
}

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
    pub fn subdir(&self, dir: &DirName) -> Self {
        Self(self.0.subdir(dir))
    }
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

impl SourceDir {
    pub fn subdir(&self, dir: &DirName) -> Self {
        Self(self.0.subdir(dir))
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

path_wrapper! {InputFile}
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

impl PreviewFile {
    pub fn file_stem(&self) -> &OsStr {
        self.0
            .file_stem()
            .expect("A preview has a file name so a link to it has one too")
    }
    /// something.<unknown>.xmp
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
    /// The dir the preview file is in
    pub fn dir(&self) -> TargetDir {
        TargetDir(Dir(self
            .0
            .parent()
            .expect("a preview is always in a target dir")
            .to_path_buf()))
    }
    pub async fn exists(&self) -> bool {
        tokio::fs::metadata(&self.0)
            .await
            .map(|meta| meta.is_file())
            .unwrap_or(false)
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

impl InputFile {
    pub fn needs_no_export(&self) -> bool {
        const NOT_NEEDING_EXPORT: [&'static str; 4] = ["jpeg", "jpg", "gif", "png"];
        let ext = self
            .0
            .extension()
            .expect("the raw listed in the xmp always has an extension");
        NOT_NEEDING_EXPORT
            .iter()
            .any(|e| OsStr::new(e).eq_ignore_ascii_case(ext))
    }
}

impl XmpFile {
    /// ```
    /// # use std::path::Path;
    /// # use dark_sorter::{PreviewFile, XmpFile};
    /// let test_cases = [
    ///     ("hi/test.NEF.xmp", "by/test.jpg"),
    ///     ("hi/test.DNG.xmp", "by/test.jpg"),
    ///     ("hi/test.JPG.xmp", "by/test.JPG"),
    ///     ("hello/hi/test.png.xmp", "goodby/see you/test.png"),
    /// ];
    ///
    /// for (xmp, preview) in test_cases {
    ///     let xmp = XmpFile(Path::new(xmp).to_path_buf());
    ///     let preview = PreviewFile(Path::new(preview).to_path_buf());
    ///
    ///     assert!(xmp.corresponds_to(&preview))
    /// }
    /// ```
    pub fn corresponds_to(&self, preview: &PreviewFile) -> bool {
        let xmp = self.file_stem().as_encoded_bytes();
        let xmp_name = xmp
            .split(|c| *c == '.' as u8)
            .next()
            .expect("split gives at least one element");
        let preview_name = preview.file_stem().as_encoded_bytes();
        xmp_name == preview_name
    }

    pub fn preview_path(&self, source: &BaseSourceDir, target: &BaseTargetDir) -> PreviewFile {
        let relative = self
            .0
            .strip_prefix(source)
            .expect("XmpFile is always inside source");
        let in_target = target.0.0.join(relative);
        let jpg = in_target.with_extension("").with_extension("jpg");
        PreviewFile(jpg)
    }

    pub fn input_file(&self) -> InputFile {
        InputFile(self.0.with_extension(""))
    }

    /// Includes the extension
    pub fn file_stem(&self) -> &OsStr {
        self.0.file_stem().expect("A raw file always has a name")
    }

    pub fn parent_dir(&self) -> SourceDir {
        SourceDir(Dir(self
            .0
            .parent()
            .expect("an xmp file is always in a source dir")
            .to_path_buf()))
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

#[cfg(test)]
mod tests {
    use super::*;
}
