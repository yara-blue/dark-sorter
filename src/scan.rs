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

use crate::fs::{Dir, DirFileStem, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::watcher::{EyreWithPath, ResultExt};
use crate::xmp::{EditHash, ParsedXmps, Xmp};
use crate::{ImageExporter, database};

mod link;

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

        let entry = DirFileStem::from(entry);

        if meta.is_dir() {
            dirs.push(entry);
        } else if meta.is_file() && entry.path().extension().is_some_and(|e| e == "xmp") {
            xmp_files.push(entry);
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
        .map(|dir| recurse_into_subdir::<Exporter>(dir, &target_dir, &fs, &previously_exported))
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
fn recurse_into_subdir<Exporter: ImageExporter>(
    dir: DirFileStem,
    target: &TargetDir,
    fs: &ThrottledFs,
    previously_exported: &database::Db,
) -> JoinHandle<color_eyre::Result<()>> {
    let source = SourceDir(Dir(dir.path().to_path_buf()));
    let target = TargetDir(Dir(target.join(dir.file_stem())));
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
    xmp_files: &[DirFileStem],
    xmps: &ParsedXmps,
    source: &SourceDir,
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
                let xmp_file = XmpFile(entry.as_ref().to_path_buf());
                Exporter::export(&xmp, &xmp_file, source)
                    .await
                    .wrap_err("failed to update preview")?;
                previously_exported.insert(entry.path().to_path_buf(), current_edits);
            } else if preview_missing(&xmp, source).await? {
                let xmp_file = XmpFile(entry.as_ref().to_path_buf());
                Exporter::export(&xmp, &xmp_file, source)
                    .await
                    .wrap_err("failed to create preview")?;
                previously_exported.insert(
                    entry.path().to_path_buf(),
                    xmp.edits.unwrap_or(EditHash::NO_EDITS),
                );
            }
            Ok(())
        })
        .collect::<FuturesUnordered<_>>()
        .try_for_each(|()| future::ready(Ok(())))
        .await
}

async fn preview_missing(xmp: &Xmp, source: &SourceDir) -> color_eyre::Result<bool> {
    let input_file = source.join(&*xmp.raw);
    let preview_path = input_file.with_extension("jpg");
    let preview_exists = tokio::fs::try_exists(dbg!(&preview_path))
        .await
        .wrap_err("Could not check if jpeg exists")
        .note_path(preview_path)?;
    Ok(!preview_exists)
}
