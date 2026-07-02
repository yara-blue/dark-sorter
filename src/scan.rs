use std::collections::HashSet;
use std::future;
use std::io::ErrorKind;

use color_eyre::Section;
use color_eyre::eyre::Context;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use futures_concurrency::future::TryJoin;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReadDirStream;

use crate::fs::{
    BaseSourceDir, BaseTargetDir, DirName, PreviewFile, SourceDir, TargetDir, ThrottledFs, XmpFile,
};
use crate::watcher::{EyreWithPath, ResultExt};
use crate::xmp::ParsedXmps;
use crate::{ImageExporter, database, immich};

pub mod preview;

pub async fn scan_clean_and_link<Exporter: ImageExporter>(
    source_dir: BaseSourceDir,
    target_dir: BaseTargetDir,
    fs: ThrottledFs,
    previously_exported: database::Db,
    immich: Option<mpsc::Sender<immich::Event>>,
) -> color_eyre::Result<()> {
    let parsed_xmps = ParsedXmps::default();
    scan_clean_and_link_dir::<Exporter>(
        source_dir.into(),
        target_dir.into(),
        fs,
        previously_exported,
        parsed_xmps,
        immich,
    )
    .await
}

#[tracing::instrument(skip(fs, previously_exported))]
async fn scan_clean_and_link_dir<Exporter: ImageExporter>(
    source_dir: SourceDir,
    target_dir: TargetDir,
    fs: ThrottledFs,
    previously_exported: database::Db,
    parsed_xmps: ParsedXmps,
    immich: Option<mpsc::Sender<immich::Event>>,
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

    let mut previews = HashSet::new();
    tokio::fs::create_dir(&target_dir)
        .await
        .ignore_err_if(|e| e.kind() == ErrorKind::AlreadyExists, ())
        .wrap_err("Could not create missing target dir")
        .note_path(&source_dir)?;
    let read_target = fs
        .read_dir(&target_dir)
        .await
        .wrap_err("Could not read target dir")
        .note_path(&target_dir)?;
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
        if ty.is_file()
            && let Ok(preview) = PreviewFile::try_from(entry)
        {
            previews.insert(preview);
        }
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
                &parsed_xmps,
                &immich,
            )
        })
        .collect::<FuturesUnordered<_>>()
        .map(|join_result| join_result.wrap_err("A panic occurred").flatten())
        .try_for_each(|()| future::ready(Ok(())));

    let (_, n_preview_links_created, _) = (
        preview::remove_stale(&source_dir, previews.iter(), &parsed_xmps, &fs),
        preview::create_update_or_clean::<Exporter>(
            &xmp_files,
            &parsed_xmps,
            &source_dir,
            &target_dir,
            &fs,
            &previously_exported,
        ),
        recursive_scans,
    )
        .try_join()
        .await?;

    if previews.len() + n_preview_links_created == 0 {
        match tokio::fs::remove_dir(&target_dir).await {
            Ok(()) => {
                if let Some(tx) = immich {
                    tx.send(immich::Event::EmptyDir(target_dir)).await?
                }
            }
            Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => (),
            Err(e) => Err(e)?,
        }
    }
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
    parsed_xmps: &ParsedXmps,
    immich: &Option<mpsc::Sender<immich::Event>>,
) -> JoinHandle<color_eyre::Result<()>> {
    let source = source.subdir(dir);
    let target = target.subdir(dir);
    let previously_exported = previously_exported.clone();
    let fs = fs.clone();
    let parsed_xmps = parsed_xmps.clone();
    let immich = immich.clone();
    tokio::spawn(scan_clean_and_link_dir::<Exporter>(
        source,
        target,
        fs,
        previously_exported,
        parsed_xmps,
        immich,
    ))
}
