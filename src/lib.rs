use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::{StreamExt, TryStreamExt};
use tokio::fs::{DirEntry, read_dir};
use tokio_stream::wrappers::ReadDirStream;

// #[cfg(feature = "test_support")]
pub mod test_support;

// TODO lock file to prevent two instances of this ever running
// - place own on top, then look for lock file in every dir. Abandon if present

pub async fn scan_clean_and_link(source: &Path, target: &Path) -> color_eyre::Result<()> {
    let read_source = read_dir(source)
        .await
        .wrap_err("Could not read source dir")?;
    let mut read_source = ReadDirStream::new(read_source);

    let mut dirs = Vec::new();
    let mut xmp_files = HashSet::new();
    while let Some(res) = read_source.next().await {
        let entry = res
            .wrap_err("Could not read source dir entry")
            .with_note(|| format!("dir: {}", source.display()))?;
        let ty = entry
            .file_type()
            .await
            .wrap_err("Could not resolve file type")
            .with_note(|| format!("entry: {}", entry.path().display()))?;

        if ty.is_dir() {
            dirs.push(entry.file_name());
        } else if ty.is_file() && entry.path().extension().is_some_and(|e| e == "xmp") {
            xmp_files.insert(entry.file_name());
        }
    }

    let mut links = HashSet::new();
    let read_target = read_dir(target)
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
        if ty.is_symlink() && entry.path().extension().is_some_and(|e| e == "jpg") {
            links.insert(entry.file_name());
        }
    }

    // TODO check for lockfile here

    for dir in dirs {
        // TODO spawn task
        scan_clean_and_link(&source.join(dir), &target.join(dir)).await;
    }

    clean_stale_links(&xmp_files, &links).await;
    create_new_links(&xmp_files, &links).await;

    Ok(())
}

async fn clean_stale_links(xmp_files: &HashSet<OsString>, links: &HashSet<OsString>) -> color_eyre::Result<()> {
    links.difference(xmp_files)).await?;
    links.union(xmp_files).filter) but xmp_files no rating
    // - no xmp file corresponds to link
    // - xmp exists but has no rating anymore
    Ok(())
}

async fn update_jpg_preview(xmp_files: &[OsString], links: &[OsString]) -> color_eyre::Result<()> {
    // - jpg is older then xmp file
    Ok(())
}

async fn create_new_links(xmp_files: &[DirEntry], links: &[DirEntry]) -> color_eyre::Result<()> {
    // - no link exists for xmp file with rating
    //
    Ok(())
}
