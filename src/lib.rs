use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::future::try_join4;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use tokio::fs::{self, DirEntry, read_dir};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReadDirStream;

// #[cfg(feature = "test_support")]
pub mod test_support;

mod darktable_cli;
mod xmp;

// TODO work through modification date based state to skip dir and files that have not changed

pub async fn scan_clean_and_link(
    source: PathBuf,
    target: PathBuf,
    previously_exported: Arc<HashMap<PathBuf, xmp::EditHash>>,
) -> color_eyre::Result<()> {
    let read_source = read_dir(&source)
        .await
        .wrap_err("Could not read source dir")?;
    let mut read_source = ReadDirStream::new(read_source);

    let mut dirs = Vec::new();
    let mut xmp_files = Vec::new();
    while let Some(res) = read_source.next().await {
        let entry = res
            .wrap_err("Could not read source dir entry")
            .with_note(|| format!("dir: {}", source.display()))?;
        let ty = entry
            .file_type()
            .await
            .wrap_err("Could not resolve file type")
            .with_note(|| format!("entry: {}", entry.path().display()))?;

        let entry = DirFileStem::from(entry);
        if ty.is_dir() {
            dirs.push(entry);
        } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "xmp") {
            let entry = DirFileStem::from(entry);
            xmp_files.push(entry);
        }
    }

    let mut links = HashSet::new();
    let read_target = read_dir(&target)
        .await
        .wrap_err("Could not read source dir")?;
    let mut read_target = ReadDirStream::new(read_target);
    while let Some(res) = read_target.next().await {
        let entry = res
            .wrap_err("Could not read target dir entry")
            .with_note(|| format!("dir: {}", source.display()))?;
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
        .map(|dir| recurse_into_subdir(dir, &target, &previously_exported))
        .collect::<FuturesUnordered<_>>()
        .map(|join_result| join_result.wrap_err("A panic occurred").flatten())
        .try_for_each(|()| future::ready(Ok(())));

    let parsed_xmps = xmp::ParsedXmps::default();
    try_join4(
        clean_stale_links(&source, links.iter(), &parsed_xmps),
        create_new_links(&xmp_files, &links, &target, &parsed_xmps),
        update_jpg_preview(&xmp_files, &parsed_xmps, &source, previously_exported),
        recursive_scans,
    )
    .await?;
    Ok(())
}

// dear rustc gets into a cycle trying to figure out the return type of the tokio::spawn.
// this little wrapper works around that.
fn recurse_into_subdir(
    dir: DirFileStem,
    target: &Path,
    previously_exported: &Arc<HashMap<PathBuf, xmp::EditHash>>,
) -> JoinHandle<color_eyre::Result<()>> {
    let source = dir.path().to_path_buf();
    let target = target.join(&dir.file_stem());
    let previously_exported = Arc::clone(previously_exported);
    tokio::spawn(scan_clean_and_link(source, target, previously_exported))
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
    source_dir: &Path,
    xmps: &xmp::ParsedXmps,
) -> color_eyre::Result<bool> {
    let link_target = match fs::read_link(link.path()).await {
        Ok(link_target) => link_target,
        // link already got removed
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).wrap_err("Could not resolve link"),
    };

    // do not remove symlinks that where probably not placed by us
    if !link_target.is_file() && link_target.extension().is_some_and(|e| e == "jpg") {
        return Ok(true);
    }

    // TODO: modification time optimization
    // ^ use another "get_or_read_and_parse" like structure

    let corresponding_xmp = source_dir.join(&link.file_stem()).with_extension("xmp");
    let xmp = match xmps.get_or_read_and_parse(&corresponding_xmp).await {
        Ok(xmp) => xmp,
        Err(xmp::ReadParseError::NotFound) => return Ok(true),
        Err(e) => return Err(e).wrap_err("Could not read xmp"),
    };

    if xmp.rating.is_none() {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn clean_stale_links(
    source_dir: &Path,
    links: impl Iterator<Item = &DirFileStem>,
    xmps: &xmp::ParsedXmps,
) -> color_eyre::Result<()> {
    links
        .map(|link| async {
            if should_remove_link(link, source_dir, xmps)
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
    source_dir: &Path,
    previously_exported: Arc<HashMap<PathBuf, xmp::EditHash>>,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .map(|entry| async {
            let xmp = xmps.get_or_read_and_parse(entry.path()).await?;
            if let Some(current_edits) = xmp.edits
                && let Some(exported_edits) = previously_exported.get(entry.path())
                && current_edits != *exported_edits
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

async fn create_link(xmp_file: &Path, target: &Path) -> color_eyre::Result<()> {
    let jpg = xmp_file.with_extension("jpg");
    let target = target.join(
        jpg.file_name()
            .expect("TODO!(yara) move this check to collection"),
    );

    fs::symlink(&jpg, &target)
        .await
        .wrap_err("Could not create link")
        .with_note(|| format!("link: {} -> {}", target.display(), jpg.display()))
}

async fn should_be_linked(xmp_file: &Path, xmps: &xmp::ParsedXmps) -> color_eyre::Result<bool> {
    let xmp = xmps
        .get_or_read_and_parse(xmp_file)
        .await
        .wrap_err("Could not read xmp")?;
    if xmp.rating.is_none() {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn create_new_links(
    xmp_files: &[DirFileStem],
    links: &HashSet<DirFileStem>,
    target: &Path,
    xmps: &xmp::ParsedXmps,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .filter(|xmp| not_already_linked(*xmp, &links))
        .map(|xmp| async {
            if should_be_linked(xmp.path(), xmps)
                .await
                .wrap_err("Could not determine whether link should be added")?
            {
                create_link(&xmp.path(), target).await
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
