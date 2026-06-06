use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::Metadata;
use std::future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt};
use futures::future::try_join4;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use tokio::fs::DirEntry;
use tokio::io;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReadDirStream;

// #[cfg(feature = "test_support")]
pub mod test_support;

mod darktable_cli;
mod database;
pub use database::Db;
mod background_scanner;
mod xmp;
// mod watcher;

// TODO work through modification date based state to skip files that have not changed
// TODO: modification time optimization
// ^ use another "get_or_read_and_parse" like structure

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

    async fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let _permit = self.file_limit.acquire().await;
        tokio::fs::read_to_string(path).await
    }

    async fn read_dir(&self, dir: impl AsRef<Dir>) -> io::Result<tokio::fs::ReadDir> {
        let _permit = self.file_limit.acquire().await;
        tokio::fs::read_dir(&dir.as_ref().0).await
    }
}

async fn create_dir_ignore_exists(dir: &TargetDir) -> io::Result<()> {
    match tokio::fs::create_dir(&dir.0.0).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

macro_rules! dir_wrapper {
    ($name:ident, $wraps:ident) => {
        #[derive(Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
        struct $name($wraps);

        impl $name {
            fn display(&self) -> std::path::Display<'_> {
                self.0.display()
            }
            fn join(&self, path: impl AsRef<Path>) -> PathBuf {
                self.0.join(path)
            }
        }
        impl AsRef<$wraps> for $name {
            fn as_ref(&self) -> &$wraps {
                &self.0
            }
        }
    };
}
dir_wrapper! {TargetDir, Dir}
dir_wrapper! {SourceDir, Dir}

macro_rules! path_wrapper {
    ($name:ident) => {
        #[derive(Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
        struct $name(PathBuf);

        impl $name {
            fn display(&self) -> std::path::Display<'_> {
                self.0.display()
            }
            fn join(&self, path: impl AsRef<Path>) -> PathBuf {
                self.0.join(path)
            }
        }
        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                &self.0
            }
        }
    };
}
path_wrapper! {XmpFile}
path_wrapper! {Dir}

/// Heuristic for how often a directory should be re-scanned based on how
/// "active" it is. Use `DirScore::get` to get the current score. Higher is more
/// active
struct DirActivity {
    ts_sum: u64,
    count: u64,
}

// globally track to 100 most recently changed files

// determine dir with most recently changed or added files

// determine dir with most recently added dir

// re-scan that dir more often

// scan
// global top 100 files every n-seconds
// top 5 dirs with most recent changes, n-seconds * scan cost / 1000
//   -- note: non recursive
// dir with most recently added dirs (ignore files), n-seconds * scan cost /
// 1000
//
// everything else once every not that often.

//
// age of the youngest 5 files.
//      - paths to do targeted scan
// cost of the scan
//

// for each file
//      updated previous time since last change
//          previous "score" + elapsed since that "scan"
//      time "since" last change, lower better
//
//
//      - more files that are old increases the "cost" and lowers score
//      - file appearing is as important as one changing both have big impact
//      - whole thing needs to be divided by count/number of files

// TODO integrate dir scores into Db? do we flush to disk? yes right?

pub async fn scan_clean_and_link(
    source_dir: SourceDir,
    target_dir: TargetDir,
    fs: ThrottledFs,
    previously_exported: database::Db,
    dir_score: DirScores,
) -> color_eyre::Result<()> {
    let read_source = fs
        .read_dir(&source_dir)
        .await
        .wrap_err("Could not read source dir")
        .with_note(|| format!("path: {}", source_dir.display()))?;
    let mut read_source = ReadDirStream::new(read_source);

    let mut dirs = Vec::new();
    let mut xmp_files = Vec::new();
    while let Some(res) = read_source.next().await {
        let entry = res
            .wrap_err("Could not read source dir entry")
            .with_note(|| format!("dir: {}", source_dir.display()))?;
        let meta = entry
            .metadata()
            .await
            .wrap_err("Could not get dir entry metadata")
            .with_note(|| format!("entry: {}", entry.path().display()))?;

        let entry = DirFileStem::from(entry);

        if meta.is_dir() {
            dirs.push(entry);
        } else if meta.is_file() && entry.path().extension().is_some_and(|e| e == "xmp") {
            dir_score.update(&source_dir, &meta);
            let entry = DirFileStem::from(entry);
            xmp_files.push(entry);
        }
    }

    let mut links = HashSet::new();
    create_dir_ignore_exists(&target_dir)
        .await
        .wrap_err("Could not create missing target dir")
        .with_note(|| format!("path: {}", source_dir.display()))?;
    let read_target = fs
        .read_dir(&target_dir)
        .await
        .wrap_err("Could not read target dir")
        .with_note(|| format!("path: {}", source_dir.display()))?;
    let mut read_target = ReadDirStream::new(read_target);
    while let Some(res) = read_target.next().await {
        let entry = res
            .wrap_err("Could not read target dir entry")
            .with_note(|| format!("dir: {}", source_dir.display()))?;
        let ty = entry
            .file_type()
            .await
            .wrap_err("Could not resolve file type")
            .with_note(|| format!("entry: {}", entry.path().display()))?;
        let entry = DirFileStem::from(entry);
        if ty.is_symlink() && entry.path().extension().is_some_and(|e| e == "jpg") {
            links.insert(entry);
        }
    }

    let recursive_scans = dirs
        .into_iter()
        .map(|dir| recurse_into_subdir(dir, &target_dir, &fs, &previously_exported))
        .collect::<FuturesUnordered<_>>()
        .map(|join_result| join_result.wrap_err("A panic occurred").flatten())
        .try_for_each(|()| future::ready(Ok(())));

    let parsed_xmps = xmp::ParsedXmps::default();
    try_join4(
        clean_stale_links(&source_dir, links.iter(), &parsed_xmps, &fs),
        create_new_links(
            &xmp_files,
            &links,
            &target_dir,
            &source_dir,
            &parsed_xmps,
            &fs,
        ),
        update_jpg_preview(
            &xmp_files,
            &parsed_xmps,
            &source_dir,
            previously_exported,
            &fs,
        ),
        recursive_scans,
    )
    .await?;
    Ok(())
}

// dear rustc gets into a cycle trying to figure out the return type of the tokio::spawn.
// this little wrapper works around that.
fn recurse_into_subdir(
    dir: DirFileStem,
    target: &TargetDir,
    fs: &ThrottledFs,
    previously_exported: &database::Db,
) -> JoinHandle<color_eyre::Result<()>> {
    let source = dir.path().to_path_buf();
    let target = target.join(&dir.file_stem());
    let previously_exported = previously_exported.clone();
    let fs = fs.clone();
    tokio::spawn(scan_clean_and_link(source, target, fs, previously_exported))
}

/// A path that behaves like a file stem in HashSets and when compared
struct DirFileStem(PathBuf);

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
    fn path(&self) -> &Path {
        &self.0
    }
    fn file_stem(&self) -> &OsStr {
        &self.0.file_stem().expect("checked")
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

/// Should remove if link:
/// - is not pointing to a file
/// - the symlink does not point to a jpg
/// - the corresponding_xmp does not exist
/// - the corresponding_xmp does not have a rating for the image
async fn should_remove_link(
    link: &DirFileStem,
    source_dir: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<bool> {
    let link_target = match tokio::fs::read_link(link.path()).await {
        Ok(link_target) => link_target,
        // link already got removed
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).wrap_err("Could not resolve link"),
    };

    // do not remove symlinks that where probably not placed by us
    if !link_target.is_file() && link_target.extension().is_some_and(|e| e == "jpg") {
        return Ok(true);
    }

    let corresponding_xmp = source_dir.join(&link.file_stem()).with_extension("xmp");
    let xmp = match xmps.get_or_read_and_parse(&corresponding_xmp, &fs).await {
        Ok(xmp) => xmp,
        Err(xmp::ReadParseError::NotFound(_)) => return Ok(true),
        Err(e) => return Err(e).wrap_err("Could not read xmp"),
    };

    if xmp.rating.is_none() {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn clean_stale_links(
    source_dir: &SourceDir,
    links: impl Iterator<Item = &DirFileStem>,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    links
        .map(|link| async {
            if should_remove_link(link, source_dir, xmps, fs)
                .await
                .wrap_err("Could not determine whether link should be removed")?
            {
                tokio::fs::remove_file(link.path())
                    .await
                    .wrap_err("Could not remove symlink")
            } else {
                Ok(())
            }
            .with_note(|| format!("path: {}", link.path().display()))
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

async fn update_jpg_preview(
    xmp_files: &[DirFileStem],
    xmps: &xmp::ParsedXmps,
    source_dir: &SourceDir,
    previously_exported: database::Db,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .map(|entry| async {
            let xmp = xmps.get_or_read_and_parse(entry.path(), fs).await?;
            if let Some(current_edits) = xmp.edits
                && let Some(exported_edits) = previously_exported.get(entry.path())
                && current_edits != exported_edits
            {
                darktable_cli::export(xmp, entry.path(), source_dir)
                    .await
                    .wrap_err("failed to export image")?;
                todo!("update database");
            }
            Ok(())
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

async fn create_link(
    xmp_path: &Path,
    source_dir: &SourceDir,
    target_dir: &TargetDir,
) -> color_eyre::Result<()> {
    // remove .RAW.xmp (with .RAW some raw format like .NEF or .DNG)
    // TODO refactor, make this nice
    let mut xmp_path = xmp_path.with_extension("");
    xmp_path.set_extension("");
    let name = xmp_path.file_name().expect("DirEntry has a file name");

    let preview = source_dir.join(&name).with_extension("jpg");
    let link = target_dir.join(&name).with_extension("jpg");

    tokio::fs::symlink(&preview, &link)
        .await
        .wrap_err("Could not create link")
        .with_note(|| format!("link: {} -> {}", link.display(), preview.display()))
}

async fn should_be_linked(
    xmp_file: &Path,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<bool> {
    let xmp = xmps
        .get_or_read_and_parse(xmp_file, fs)
        .await
        .wrap_err("Could not read xmp")
        .with_note(|| format!("Path: {}", xmp_file.display()))?;
    if xmp.rating.is_some() {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn create_new_links(
    xmp_files: &[DirFileStem],
    links: &HashSet<DirFileStem>,
    target_dir: &TargetDir,
    source_dir: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .filter(|xmp| not_already_linked(*xmp, &links))
        .map(|xmp| async {
            if dbg!(should_be_linked(xmp.path(), xmps, fs).await)
                .wrap_err("Could not determine whether link should be added")?
            {
                create_link(xmp.path(), source_dir, target_dir).await
            } else {
                Ok(())
            }
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

fn not_already_linked(xmp_file: &DirFileStem, links: &HashSet<DirFileStem>) -> bool {
    !links.contains(xmp_file)
}
