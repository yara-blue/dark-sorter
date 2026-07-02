use std::hint::cold_path;
use std::io::ErrorKind;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use caps::{CapSet, Capability};
use color_eyre::Section;
use color_eyre::eyre::{Context, eyre};
use fanotify_fid::Fanotify;
use fanotify_fid::consts::{
    FAN_DELETE, FAN_MARK_ADD, FAN_MARK_FILESYSTEM, FAN_MOVED_FROM, FAN_MOVED_TO,
};
use fanotify_fid::types::FidEvent;
use libc::FAN_CLOSE_WRITE;
use tracing::{debug, instrument};

use crate::fs::{PreviewFile, PreviewLink, ThrottledFs, XmpFile};
use crate::xmp::Xmp;
use crate::{BaseSourceDir, BaseTargetDir, Db, ImageExporter};

pub fn start(dir: BaseSourceDir) -> color_eyre::Result<Receiver<Kitty>> {
    if !caps::has_cap(None, CapSet::Permitted, Capability::CAP_SYS_ADMIN)
        .wrap_err("Could not check capabilities")?
    {
        return Err(eyre!("Must have capability CAP_SYS_ADMIN to run watcher"))
            .suggestion("Try running as root using sudo");
    }

    let fan = Fanotify::new()
        .report_fid()
        .report_dir_fid()
        .report_target_fid()
        .report_name()
        .init()
        .wrap_err("Could not initialize fanotify")?;

    fan.mark(
        FAN_MARK_ADD | FAN_MARK_FILESYSTEM,
        FAN_CLOSE_WRITE | FAN_DELETE | FAN_MOVED_FROM | FAN_MOVED_TO,
        &dir,
    )
    .wrap_err("Could not mark filesystem for watching")
    .note_path(&dir)?;

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mount_fds = [fanotify_fid::open_mount(&dir).unwrap()];
        loop {
            let mut buf = vec![0u8; 4096];
            for event in fan
                .read_events(&mount_fds, &mut buf, None)
                .expect("could not read fanotify events")
            {
                handle_event(&event, &dir, &tx);
            }
        }
    });
    Ok(rx)
}

#[instrument(skip(source, target, fs))]
pub async fn handle_kitty_fs_change<Exporter: ImageExporter>(
    event: Kitty,
    source: &BaseSourceDir,
    target: &BaseTargetDir,
    fs: &ThrottledFs,
    db: &Db,
) -> color_eyre::Result<()> {
    let xmp_file = event.xmp_file;
    let xmp = Xmp::read_from_file(&xmp_file, fs)
        .await
        .wrap_err("Could not read xmp file")
        .note_path(&xmp_file)?;
    let link = xmp_file.link_path(target);
    let preview = xmp_file.preview_path(source);

    debug!("got one");
    match event.event {
        KittyKind::FileDeleted | KittyKind::FileMovedFrom => {
            clean_up(&link, &preview)?;
        }
        // Some tools move the changed file over the existing one
        // instead of opening it for writing. So a move to can actually edit
        // the rating.
        KittyKind::FileModificationComplete | KittyKind::FileMovedTo => {
            if xmp.rated() {
                if xmp.preview_missing(source).await? || db.get(&xmp_file) != xmp.edits {
                    Exporter::export(&xmp, &xmp_file, source, fs)
                        .await
                        .wrap_err("failed to export image")?;
                }
                fs.symlink(&preview, &link)
                    .await
                    .wrap_err("Could not create link")
                    .with_note(|| format!("link: {link} -> {preview}"))?;
            } else {
                clean_up(&link, &preview)?;
            }
        }
    }

    Ok(())
}

pub trait ResultExt<T, E> {
    #[must_use]
    fn ignore_err_if(self, filter: impl FnOnce(&E) -> bool, val: T) -> Self;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn ignore_err_if(self, filter: impl FnOnce(&E) -> bool, val: T) -> Self {
        match self {
            Ok(v) => Ok(v),
            Err(e) if filter(&e) => Ok(val),
            Err(e) => Err(e),
        }
    }
}

pub trait EyreWithPath {
    #[must_use]
    fn note_path(self, path: impl AsRef<std::path::Path>) -> Self;
}

impl<T> EyreWithPath for color_eyre::Result<T> {
    fn note_path(self, path: impl AsRef<std::path::Path>) -> Self {
        self.with_note(|| format!("path: {}", path.as_ref().display()))
    }
}

#[instrument]
fn clean_up(link: &PreviewLink, preview: &PreviewFile) -> Result<(), color_eyre::eyre::Error> {
    debug!("removing file and symlink");
    std::fs::remove_file(link)
        .ignore_err_if(|e| e.kind() == ErrorKind::NotFound, ())
        .wrap_err("Could not remove link to preview")
        .note_path(link)?;
    std::fs::remove_file(preview)
        .ignore_err_if(|e| e.kind() == ErrorKind::NotFound, ())
        .wrap_err("Could not remove preview jpg")
        .note_path(preview)?;
    Ok(())
}

#[derive(Debug)]
pub struct Kitty {
    xmp_file: XmpFile,
    event: KittyKind,
    pub overflow: bool,
}

#[derive(Debug)]
pub enum KittyKind {
    FileModificationComplete,
    FileDeleted,
    FileMovedTo,
    FileMovedFrom,
}

fn handle_event(event: &FidEvent, dir: &BaseSourceDir, tx: &Sender<Kitty>) {
    // Must run fast, gets ran for each file on the mount
    if let Some(ext) = event.path.extension()
        && ext == "xmp"
    {
        cold_path();

        if event.path.starts_with(dir) && event.path.is_file() {
            if event.mask & FAN_CLOSE_WRITE > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileModificationComplete,
                    overflow: event.is_overflow(),
                })
                .expect("could not send");
            }
            if event.mask & FAN_DELETE > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileDeleted,
                    overflow: event.is_overflow(),
                })
                .expect("could not send");
            }
            if event.mask & FAN_MOVED_FROM > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileMovedFrom,
                    overflow: event.is_overflow(),
                })
                .expect("could not send");
            }
            if event.mask & FAN_MOVED_TO > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileMovedTo,
                    overflow: event.is_overflow(),
                })
                .expect("could not send");
            }
        }
    }
}
