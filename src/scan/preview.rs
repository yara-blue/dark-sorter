pub(crate) use std::future;
use std::io::ErrorKind;

use color_eyre::eyre::Context;
use futures::TryStreamExt;
use futures::stream::FuturesUnordered;
use tracing::{debug, instrument};

use crate::fs::{PreviewFile, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::watcher::{EyreWithPath, ResultExt};
use crate::xmp::{EditHash, ParsedXmps, Xmp};
use crate::{ImageExporter, database, xmp};

/// Should remove if link:
/// - is not pointing to a file
/// - the symlink does not point to a jpg
/// - the corresponding xmp does not exist
/// - the corresponding xmp does not have a rating for the image
#[instrument(skip(source_dir, fs))]
pub async fn should_remove(
    preview: &PreviewFile,
    source_dir: &SourceDir,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<bool> {
    let xmp = match xmps
        .get_cached_or_read_from_file(&preview.xmp_path(source_dir), fs)
        .await
    {
        Ok(xmp) => xmp,
        Err(xmp::XmpError::NotFound(_)) => {
            debug!("No known xmp corresponding with preview");
            return Ok(true);
        }
        Err(e) => return Err(e).wrap_err("Could not read xmp"),
    };

    if xmp.rated() {
        debug!("preview of unrated file");
        Ok(false)
    } else {
        Ok(true)
    }
}

#[tracing::instrument(skip(xmps, fs))]
pub async fn remove_if_stale(
    source_dir: &SourceDir,
    preview: &PreviewFile,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    if should_remove(preview, source_dir, xmps, fs)
        .await
        .wrap_err("Could not determine whether link should be removed")?
    {
        debug!("removing stale link");
        tokio::fs::remove_file(&preview)
            .await
            .wrap_err("Could not remove preview file")
            .note_path(preview)
    } else {
        Ok(())
    }
}

pub async fn remove_stale(
    source_dir: &SourceDir,
    previews: impl Iterator<Item = &PreviewFile>,
    xmps: &xmp::ParsedXmps,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    previews
        .map(|preview| remove_if_stale(source_dir, preview, xmps, fs))
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

pub(crate) async fn create_update_or_clean<Exporter: ImageExporter>(
    xmp_files: &[XmpFile],
    xmps: &ParsedXmps,
    source: &SourceDir,
    target: &TargetDir,
    fs: &ThrottledFs,
    previously_exported: &database::Db,
) -> color_eyre::Result<usize> {
    xmp_files
        .iter()
        .cloned()
        .map(|xmp_file| async move {
            let xmp = xmps.get_cached_or_read_from_file(&xmp_file, fs).await?;
            create_update_or_clean_one::<Exporter>(
                xmp,
                &xmp_file,
                source,
                target,
                fs,
                previously_exported,
            )
            .await
        })
        .collect::<FuturesUnordered<_>>()
        .try_fold(0, |sum, i| future::ready(Ok(sum + i)))
        .await
}

pub(crate) async fn create_update_or_clean_one<Exporter: ImageExporter>(
    xmp: Xmp,
    xmp_file: &XmpFile,
    source: impl AsRef<SourceDir>,
    target: impl AsRef<TargetDir>,
    fs: &ThrottledFs,
    previously_exported: &database::Db,
) -> color_eyre::Result<usize> {
    let input_file = xmp.raw_file(source);
    let output_file = xmp.preview_file(&target);
    if let Some(current_edits) = xmp.edits
        && let Some(exported_edits) = previously_exported.get(&xmp_file)
        && current_edits != exported_edits
        && xmp.rated()
    {
        Exporter::export(&xmp_file, &input_file, &output_file, fs)
            .await
            .wrap_err("failed to update preview")?;
        previously_exported.insert(xmp_file.clone(), current_edits);
        Ok(1)
    } else if xmp.rated() && xmp.preview_missing(target).await? {
        Exporter::export(&xmp_file, &input_file, &output_file, fs)
            .await
            .wrap_err("failed to create preview")?;
        previously_exported.insert(xmp_file.clone(), xmp.edits.unwrap_or(EditHash::NO_EDITS));
        Ok(1)
    } else {
        clean_up(&output_file)?;
        Ok(0)
    }
}

#[instrument]
pub fn clean_up(preview: &PreviewFile) -> Result<(), color_eyre::eyre::Error> {
    debug!("removing preview file");
    std::fs::remove_file(preview)
        .ignore_err_if(|e| e.kind() == ErrorKind::NotFound, ())
        .wrap_err("Could not remove preview jpg")
        .note_path(preview)?;
    Ok(())
}
