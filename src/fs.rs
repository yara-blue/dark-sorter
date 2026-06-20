use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use color_eyre::eyre::{Context, OptionExt};
use tokio::fs::DirEntry;
use tokio::io;
use tokio::sync::Semaphore;

/// Limit concurrent fs access so we do not exceed the open file handle limit.
#[derive(Clone)]
pub struct ThrottledFs {
    file_limit: Arc<Semaphore>,
}

impl ThrottledFs {
    pub fn new() -> color_eyre::Result<Self> {
        let limit_plus_one = rlimit::Resource::NOFILE
            .get_soft()
            .wrap_err("Could not get max number of file handles form the OS")?;
        let limit = limit_plus_one
            .checked_sub(10) // I know makes now sense but mrrow :3
            .ok_or_eyre("OS file handle limit too low")?;
        Ok(Self {
            file_limit: Arc::new(Semaphore::new(limit as usize)),
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
            #[allow(dead_code)]
            pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
                self.0.join(path)
            }
        }
        impl AsRef<$wraps> for $name {
            fn as_ref(&self) -> &$wraps {
                &self.0
            }
        }
        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                &self.0.0
            }
        }
        impl AsRef<OsStr> for $name {
            fn as_ref(&self) -> &OsStr {
                &self.0.0.as_ref()
            }
        }
        impl FromStr for $name {
            type Err = <PathBuf as FromStr>::Err;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                PathBuf::from_str(s).map(Dir).map($name)
            }
        }
    };
}
dir_wrapper! {TargetDir, Dir}
dir_wrapper! {SourceDir, Dir}

macro_rules! path_wrapper {
    ($name:ident) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
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
    };
}
path_wrapper! {PreviewLink}
path_wrapper! {PreviewFile}
path_wrapper! {XmpFile}
path_wrapper! {Dir}

impl XmpFile {
    pub fn link_path(&self, target: &TargetDir) -> PreviewLink {
        let mut xmp_path = self.0.with_extension("");
        xmp_path.set_extension("");
        let name = xmp_path.file_name().expect("DirEntry has a file name");

        let link = target.join(name).with_extension("jpg");
        PreviewLink(link)
    }

    pub fn preview_path(&self, source: &SourceDir) -> PreviewFile {
        let mut xmp_path = self.0.with_extension("");
        xmp_path.set_extension("");
        let name = xmp_path.file_name().expect("DirEntry has a file name");

        let preview = source.join(name).with_extension("jpg");
        PreviewFile(preview)
    }
}

/// A path that behaves like a file stem in HashSets and when compared
pub struct DirFileStem(PathBuf);

impl AsRef<Path> for DirFileStem {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}

impl From<DirEntry> for DirFileStem {
    fn from(e: DirEntry) -> Self {
        let path = e.path();
        assert!(
            path.file_stem().is_some(),
            "dir entries always have a file stem"
        );
        DirFileStem(path)
    }
}

impl DirFileStem {
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn file_stem(&self) -> &OsStr {
        self.0.file_stem().expect("checked")
    }
}

impl std::hash::Hash for DirFileStem {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.file_stem().expect("checked").hash(state);
    }
}

impl PartialEq for DirFileStem {
    fn eq(&self, other: &DirFileStem) -> bool {
        self.0.file_stem().expect("checked") == other.0.file_stem().expect("checked")
    }
}

impl Eq for DirFileStem {}
