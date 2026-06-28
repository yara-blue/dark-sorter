use std::hint::cold_path;
use std::io::ErrorKind;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use caps::{CapSet, Capability};
use color_eyre::Section;
use color_eyre::eyre::{Context, eyre};
use fanotify_fid::Fanotify;
use fanotify_fid::consts::{
    FAN_CREATE, FAN_DELETE, FAN_MARK_ADD, FAN_MARK_FILESYSTEM, FAN_MODIFY, FAN_MOVE,
    FAN_MOVED_FROM, FAN_MOVED_TO,
};
use fanotify_fid::types::FidEvent;

use crate::ImageExporter;
use crate::fs::{PreviewFile, PreviewLink, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::xmp::{self, Xmp};

pub fn start(dir: SourceDir) -> color_eyre::Result<Receiver<Kitty>> {
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
        FAN_CREATE | FAN_DELETE | FAN_MODIFY | FAN_MOVE,
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

fn should_be_linked(xmp: &Xmp) -> bool {
    xmp.rating.is_some()
}

pub async fn handle_kitty_fs_change<Exporter: ImageExporter>(
    event: Kitty,
    source: &SourceDir,
    target: &TargetDir,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    let xmp = fs
        .read_to_string(&event.xmp_file)
        .await
        .map_err(|e| xmp::ReadParseError::from_io(e, &event.xmp_file))?;
    let xmp = Xmp::from_str(&xmp).map_err(xmp::ReadParseError::Parse)?;
    let xmp_path = event.xmp_file;
    let link = xmp_path.link_path(target);
    let preview = xmp_path.preview_path(source);

    match event.event {
        KittyKind::FileCreated => {
            if should_be_linked(&xmp) {
                Exporter::export(&xmp, &xmp_path, source, fs)
                    .await
                    .wrap_err("failed to export image")?;
                fs.symlink(&preview, &link)
                    .await
                    .wrap_err("Could not create link")
                    .with_note(|| format!("link: {} -> {}", link.display(), preview.display()))?;
            }
        }
        KittyKind::FileDeleted | KittyKind::FileMovedFrom => {
            clean_up(&link, &preview)?;
        }
        KittyKind::FileModified => {
            if should_be_linked(&xmp) {
                Exporter::export(&xmp, &xmp_path, source, fs)
                    .await
                    .wrap_err("failed to export image")?;
                fs.symlink(&preview, &link)
                    .await
                    .wrap_err("Could not create link")
                    .with_note(|| format!("link: {} -> {}", link.display(), preview.display()))?;
            } else {
                clean_up(&link, &preview)?;
            }
        }
        KittyKind::FileMovedTo => {
            if should_be_linked(&xmp) {
                fs.symlink(&preview, &link)
                    .await
                    .wrap_err("Could not create link")
                    .with_note(|| format!("link: {} -> {}", link.display(), preview.display()))?;
            }
        }
    }

    Ok(())
}

pub trait ResultExt<T, E> {
    #[must_use]
    fn err_ok_if(self, filter: impl FnOnce(&E) -> bool, val: T) -> Self;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn err_ok_if(self, filter: impl FnOnce(&E) -> bool, val: T) -> Self {
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

fn clean_up(link: &PreviewLink, preview: &PreviewFile) -> Result<(), color_eyre::eyre::Error> {
    std::fs::remove_file(link)
        .err_ok_if(|e| e.kind() == ErrorKind::NotFound, ())
        .wrap_err("Could not remove link to preview")
        .note_path(link)?;
    std::fs::remove_file(link)
        .err_ok_if(|e| e.kind() == ErrorKind::NotFound, ())
        .wrap_err("Could not remove preview jpg")
        .note_path(preview)?;
    Ok(())
}

pub struct Kitty {
    xmp_file: XmpFile,
    event: KittyKind,
    pub overflow: bool,
}

pub enum KittyKind {
    FileCreated,
    FileDeleted,
    FileModified,
    FileMovedTo,
    FileMovedFrom,
}

fn handle_event(event: &FidEvent, dir: &SourceDir, tx: &Sender<Kitty>) {
    // Must run fast, gets ran for each file on the mount
    if let Some(ext) = event.path.extension()
        && ext == "xmp"
    {
        cold_path();

        if event.path.starts_with(dir) && event.path.is_file() {
            if event.mask & FAN_CREATE > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileCreated,
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
            if event.mask & FAN_MODIFY > 0 {
                tx.send(Kitty {
                    xmp_file: XmpFile(event.path.clone()),
                    event: KittyKind::FileModified,
                    overflow: event.is_overflow(),
                })
                .expect("could not send");
            } else {
                unreachable!("We only subscribed to the above listed events")
            }
        }
    }
}
