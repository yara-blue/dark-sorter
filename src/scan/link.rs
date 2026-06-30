use std::collections::HashSet;
use std::future;
use std::io::ErrorKind;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::TryStreamExt;
use futures::stream::FuturesUnordered;
use tracing::{debug, instrument};

use crate::fs::{PreviewLink, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::watcher::EyreWithPath;
use crate::xmp;

/// Should remove if link:
/// - is not pointing to a file
/// - the symlink does not point to a jpg
/// - the corresponding xmp does not exist
/// - the corresponding xmp does not have a rating for the image
#[instrument(skip(source_dir, fs))]
pub async fn should_remove_link(
    link: &PreviewLink,
    source_dir: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<bool> {
    let link_target = match tokio::fs::read_link(link).await {
        Ok(link_target) => link_target,
        // Link points to nothing, remove it.
        Err(e) if e.kind() == ErrorKind::NotFound => {
            debug!("Link is stale");
            return Ok(true);
        }
        Err(e) => return Err(e).wrap_err("Could not resolve link"),
    };

    // do not remove symlinks that where probably not placed by us
    if !link_target.is_file() && link_target.extension().is_some_and(|e| e == "jpg") {
        return Ok(true);
    }

    let xmp = match xmps
        .get_cached_or_read_from_file(&link.xmp_path(source_dir), fs)
        .await
    {
        Ok(xmp) => xmp,
        Err(xmp::XmpError::NotFound(_)) => {
            debug!("No known xmp corresponding with link");
            return Ok(true);
        }
        Err(e) => return Err(e).wrap_err("Could not read xmp"),
    };

    if xmp.rated() {
        debug!("Link to unrated file");
        Ok(false)
    } else {
        Ok(true)
    }
}

#[tracing::instrument(skip(xmps, fs))]
pub async fn remove_link_if_stale(
    source_dir: &SourceDir,
    link: &PreviewLink,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    if should_remove_link(link, source_dir, xmps, fs)
        .await
        .wrap_err("Could not determine whether link should be removed")?
    {
        debug!("removing stale link");
        tokio::fs::remove_file(link)
            .await
            .wrap_err("Could not remove symlink")
    } else {
        Ok(())
    }
    .note_path(link)
}

pub async fn remove_stale(
    source_dir: &SourceDir,
    links: impl Iterator<Item = &PreviewLink>,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    links
        .map(|link| remove_link_if_stale(source_dir, link, xmps, fs))
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

#[instrument(skip_all)]
pub async fn create_link(
    xmp: &XmpFile,
    source_dir: &SourceDir,
    target_dir: &TargetDir,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    let preview = xmp.preview_path(source_dir);
    let link = xmp.link_path(target_dir);

    fs.symlink(&preview, &link)
        .await
        .wrap_err("Could not create link")
        .with_note(|| format!("link: {} -> {}", link.display(), preview.display()))
}

async fn should_be_linked(
    xmp_file: &XmpFile,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<bool> {
    let xmp = xmps
        .get_cached_or_read_from_file(xmp_file, fs)
        .await
        .wrap_err("Could not read xmp")
        .note_path(xmp_file)?;
    if xmp.rated() { Ok(true) } else { Ok(false) }
}

#[instrument(skip_all)]
pub async fn create_new(
    xmp_files: &[XmpFile],
    links: &HashSet<PreviewLink>,
    target: &TargetDir,
    source: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .filter(|xmp| not_already_linked(xmp, target, links))
        .map(|xmp| async {
            if should_be_linked(xmp, xmps, fs)
                .await
                .wrap_err("Could not determine whether link should be added")?
            {
                create_link(xmp, source, target, fs).await
            } else {
                Ok(())
            }
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

fn not_already_linked(xmp: &XmpFile, target: &TargetDir, links: &HashSet<PreviewLink>) -> bool {
    !links.contains(&xmp.link_path(target))
}
