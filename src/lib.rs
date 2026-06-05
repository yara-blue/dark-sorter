use std::collections::HashSet;
use std::ffi::OsString;
use std::future;
use std::io::ErrorKind;
use std::path::Path;
use std::str::FromStr;

use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt, eyre};
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use tokio::fs::{self, DirEntry, ReadDir, read_dir};
use tokio::io;
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

// NOTE watchexec lib does actually not list all files.
// TODO use notify and rescan on event flag rescan

struct Xmp {
    rating: Option<u8>,
}

impl FromStr for Xmp {
    type Err = color_eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let start_pattern = r#""xmp:Rating=""#;
        let rating_start = s
            .find(start_pattern)
            .ok_or_eyre("there should be a rating")?
            + start_pattern.len();
        let rating_end = s[rating_start..]
            .find('"')
            .ok_or_eyre("rating should end with \"")?;
        let rating = s[rating_start..rating_start + rating_end]
            .parse()
            .wrap_err("rating should be a number")?;
        let rating = match rating {
            0 => None,
            1..=5 => Some(rating),
            _ => return Err(eyre!("darktable rating should be between 0 and 5")),
        };

        Ok(Self { rating })
    }
}

/// Should remove if link:
/// - is not pointing to a file
/// - the symlink does not point to a jpg
/// - the corresponding_xmp does not exist
/// - the corresponding_xmp does not have a rating for the image
async fn should_remove_link(link: &DirEntry, source_dir: &Path) -> color_eyre::Result<bool> {
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

    let corresponding_xmp = source_dir.join(&link.file_name()).with_extension("xmp");
    let xmp = match fs::read_to_string(corresponding_xmp).await {
        Ok(xmp) => xmp,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e).wrap_err("Could not read xmp"),
    };

    let xmp = Xmp::from_str(&xmp)?;
    if xmp.rating.is_none() {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn clean_stale_links(source_dir: &Path, links: &[DirEntry]) -> color_eyre::Result<()> {
    links
        .iter()
        .map(|link| async {
            if should_remove_link(link, source_dir)
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

async fn update_jpg_preview(xmp_files: &[OsString], links: &[OsString]) -> color_eyre::Result<()> {
    // - jpg is older then xmp file
    Ok(())
}

async fn create_new_links(xmp_files: &[DirEntry], links: &[DirEntry]) -> color_eyre::Result<()> {
    // - no link exists for xmp file with rating
    //
    Ok(())
}
