use std::collections::HashSet;
use std::future;
use std::io::ErrorKind;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::future::try_join4;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReadDirStream;

use crate::fs::{DirName, PreviewLink, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::watcher::{EyreWithPath, ResultExt};
use crate::xmp::{EditHash, ParsedXmps};
use crate::{ImageExporter, database};

mod link;

#[tracing::instrument(skip_all)]
pub async fn scan_clean_and_link<Exporter: ImageExporter>(
    source_dir: SourceDir,
    target_dir: TargetDir,
    fs: ThrottledFs,
    previously_exported: database::Db,
) -> color_eyre::Result<()> {
    let read_source = fs
        .read_dir(&source_dir)
        .await
        .wrap_err("Could not read source dir")
        .note_path(&source_dir)?;
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

        if meta.is_dir() {
            dirs.push(DirName(entry.file_name()));
        } else if meta.is_file()
            && let Ok(xmp) = XmpFile::try_from(entry)
        {
            xmp_files.push(xmp);
        }
    }

    let mut links = HashSet::new();
    tokio::fs::create_dir(&target_dir)
        .await
        .err_ok_if(|e| e.kind() == ErrorKind::AlreadyExists, ())
        .wrap_err("Could not create missing target dir")
        .note_path(&source_dir)?;
    let read_target = fs
        .read_dir(&target_dir)
        .await
        .wrap_err("Could not read target dir")
        .note_path(&target_dir)?;
    let mut read_target = ReadDirStream::new(read_target);
    while let Some(res) = read_target.next().await {
        dbg!(&res);
        let entry = res
            .wrap_err("Could not read target dir entry")
            .with_note(|| format!("dir: {}", source_dir.display()))?;
        let ty = entry
            .file_type()
            .await
            .wrap_err("Could not resolve file type")
            .with_note(|| format!("entry: {}", entry.path().display()))?;
        if ty.is_symlink()
            && let Ok(link) = PreviewLink::try_from(entry)
        {
            links.insert(link);
        }
        dbg!(&links);
    }

    let recursive_scans = dirs
        .iter()
        .map(|dir| {
            recurse_into_subdir::<Exporter>(
                dir,
                &target_dir,
                &source_dir,
                &fs,
                &previously_exported,
            )
        })
        .collect::<FuturesUnordered<_>>()
        .map(|join_result| join_result.wrap_err("A panic occurred").flatten())
        .try_for_each(|()| future::ready(Ok(())));

    let parsed_xmps = ParsedXmps::default();
    try_join4(
        link::remove_stale(&source_dir, links.iter(), &parsed_xmps, &fs),
        link::create_new(
            &xmp_files,
            &links,
            &target_dir,
            &source_dir,
            &parsed_xmps,
            &fs,
        ),
        update_jpg_preview::<Exporter>(
            &xmp_files,
            &parsed_xmps,
            &source_dir,
            &previously_exported,
            &fs,
        ),
        recursive_scans,
    )
    .await?;
    Ok(())
}

// dear rustc gets into a cycle trying to figure out the return type of the tokio::spawn.
// this little wrapper works around that.
fn recurse_into_subdir<Exporter: ImageExporter>(
    dir: &DirName,
    target: &TargetDir,
    source: &SourceDir,
    fs: &ThrottledFs,
    previously_exported: &database::Db,
) -> JoinHandle<color_eyre::Result<()>> {
    let source = source.subdir(dir);
    let target = target.subdir(dir);
    let previously_exported = previously_exported.clone();
    let fs = fs.clone();
    tokio::spawn(scan_clean_and_link::<Exporter>(
        source,
        target,
        fs,
        previously_exported,
    ))
}

async fn update_jpg_preview<Exporter: ImageExporter>(
    xmp_files: &[XmpFile],
    xmps: &ParsedXmps,
    source: &SourceDir,
    previously_exported: &database::Db,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    xmp_files
        .iter()
        .cloned()
        .map(|xmp_file| async move {
            let xmp = xmps.cached_or_parse(&xmp_file, fs).await?;
            if let Some(current_edits) = xmp.edits
                && let Some(exported_edits) = previously_exported.get(&xmp_file)
                && current_edits != exported_edits
                && xmp.rated()
            {
                Exporter::export(&xmp, &xmp_file, source, fs)
                    .await
                    .wrap_err("failed to update preview")?;
                previously_exported.insert(xmp_file.clone(), current_edits);
            } else if xmp.rated() && xmp.preview_missing(source).await? {
                Exporter::export(&xmp, &xmp_file, source, fs)
                    .await
                    .wrap_err("failed to create preview")?;
                previously_exported
                    .insert(xmp_file.clone(), xmp.edits.unwrap_or(EditHash::NO_EDITS));
            }
            Ok(())
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}
