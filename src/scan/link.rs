use std::collections::HashSet;
use std::future;
use std::io::ErrorKind;
use std::path::Path;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::TryStreamExt;
use futures::stream::FuturesUnordered;

use crate::fs::{DirFileStem, SourceDir, TargetDir, ThrottledFs};
use crate::watcher::EyreWithPath;
use crate::xmp;

/// Should remove if link:
/// - is not pointing to a file
/// - the symlink does not point to a jpg
/// - the corresponding_xmp does not exist
/// - the corresponding_xmp does not have a rating for the image
pub async fn should_remove_link(
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

    let corresponding_xmp = source_dir.join(link.file_stem()).with_extension("xmp");
    let xmp = match xmps.get_or_read_and_parse(&corresponding_xmp, fs).await {
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

pub async fn remove_link_if_stale(
    source_dir: &SourceDir,
    link: &DirFileStem,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
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
    .note_path(link)
}

pub async fn remove_stale(
    source_dir: &SourceDir,
    links: impl Iterator<Item = &DirFileStem>,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    links
        .map(|link| remove_link_if_stale(source_dir, link, xmps, fs))
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

pub async fn create_link(
    xmp_path: &Path,
    source_dir: &SourceDir,
    target_dir: &TargetDir,
) -> color_eyre::Result<()> {
    // remove .RAW.xmp (with .RAW some raw format like .NEF or .DNG)
    // TODO refactor, make this nice
    let mut xmp_path = xmp_path.with_extension("");
    xmp_path.set_extension("");
    let name = xmp_path.file_name().expect("DirEntry has a file name");

    let preview = source_dir.join(name).with_extension("jpg");
    let link = target_dir.join(name).with_extension("jpg");

    tokio::fs::symlink(dbg!(&preview), dbg!(&link))
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
        .note_path(xmp_file)?;
    if xmp.rating.is_some() {
        Ok(true)
    } else {
        Ok(false)
    }
}

pub async fn create_new(
    xmp_files: &[DirFileStem],
    links: &HashSet<DirFileStem>,
    target_dir: &TargetDir,
    source_dir: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .filter(|xmp| dbg!(not_already_linked(xmp, links)))
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
