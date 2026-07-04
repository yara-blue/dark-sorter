//! File watching.
//!
//! Watching a large collection of files costs a lot of ram since the kernel needs to
//! check for each file if it should be watched. The only way the kernel can do so is
//! by storing all the file descriptors. Those are ~1kb each.
//!
//! Instead we watch the entire filesystem the photo collection is on. This means we
//! get a lot of file events, we can quickly determine if those are relevant or not.
//! Downside is that it requires root privileges.
//!
//! # Overflow under extreme system load
//! The file watcher could start running behind and dropping events. In that case
//! we clear all the events in it's queue and do a full rescan.
//!
//! There is a subtle race in the rescan we got to be mindful of:
//!
//! 1. rescan starts meanwhile file events get queued
//! 2. scan passes `dirA`
//! 3. `dirA/file` gets added. Watcher queue: [add]
//! 4. `dirA/file` gets deleted. Watcher queue: [add deleted]
//! 5. scan is done, start processing queued events
//! 6. process add, try to read xmp file -> failure can't read file
//!
//!
use std::ffi::OsStr;
use std::hint::cold_path;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use caps::{CapSet, Capability};
use color_eyre::Section;
use color_eyre::eyre::{Context, eyre};
use fanotify_fid::Fanotify;
use fanotify_fid::consts::{
    FAN_DELETE, FAN_MARK_ADD, FAN_MARK_FILESYSTEM, FAN_MOVED_FROM, FAN_MOVED_TO,
};
use fanotify_fid::types::FidEvent;
use futures::{StreamExt, TryStreamExt};
use libc::FAN_CLOSE_WRITE;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tracing::{debug, instrument};

use crate::fs::{TargetDir, ThrottledFs, XmpFile};
use crate::immich::ImmichSync;
use crate::scan::preview::Change;
use crate::xmp::{Xmp, XmpError};
use crate::{BaseSourceDir, BaseTargetDir, Db, ImageExporter, Watcher};

pub struct FanotifyWatcher {
    overflown: Arc<AtomicBool>,
    rx: Receiver<Event>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FanotifyWatcher {
    pub fn start(dir: BaseSourceDir) -> color_eyre::Result<Self> {
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

        let overflown = Arc::new(AtomicBool::new(false));
        let overflown_copy = Arc::clone(&overflown);
        let (tx, rx) = mpsc::channel(4096);
        let handle = thread::spawn(move || {
            let mount_fds = [fanotify_fid::open_mount(&dir).unwrap()];
            loop {
                let mut buf = vec![0u8; 4096];
                for event in fan
                    .read_events(&mount_fds, &mut buf, None)
                    .expect("could not read fanotify events")
                {
                    if let Err(Exiting) =
                        forward_if_relevant(&event, &dir, &tx, overflown_copy.as_ref())
                    {
                        return;
                    }
                }
            }
        });
        Ok(Self {
            overflown,
            rx,
            handle: Some(handle),
        })
    }
}

impl Watcher for FanotifyWatcher {
    fn clear(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }

    async fn next(&mut self) -> Event {
        if let Some(event) = self.rx.recv().await {
            return event;
        } else {
            let handle = self
                .handle
                .take()
                .expect("only taken here and this diverges");
            for _ in 0..10 {
                if handle.is_finished() {
                    break;
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }

            if !handle.is_finished() {
                unreachable!("Once the tx drops the thread should finish quickly")
            }
            let panic = handle
                .join()
                .expect_err("Main thread still running so watcher can only exit if it panicked");
            std::panic::resume_unwind(panic)
        }
    }

    fn overflown(&self) -> bool {
        self.overflown.load(Ordering::Relaxed)
    }
}

#[instrument(skip(base_source, base_target, fs, immich))]
pub async fn handle_event<Exporter: ImageExporter>(
    event: Event,
    base_source: &BaseSourceDir,
    base_target: &BaseTargetDir,
    fs: &ThrottledFs,
    db: &Db,
    immich: Option<&ImmichSync>,
) -> color_eyre::Result<()> {
    debug!("Got relevant file event");

    let xmp_file = event.xmp_file;
    let Some(xmp) = Xmp::read_from_file(&xmp_file, fs)
        .await
        .map(Some)
        .ignore_err_if(|e| matches!(e, XmpError::NotFound(_)), None)
        .wrap_err("Could not read xmp file")
        .note_path(&xmp_file)?
    else {
        // Must have gotten deleted again before we got to this event
        return Ok(());
    };
    let preview = xmp_file.preview_path(base_source, base_target);
    let target = preview.parent_dir();
    let source = xmp_file.parent_dir();

    let change = match event.kind {
        EventKind::FileDeleted | EventKind::FileMovedFrom => {
            crate::scan::preview::clean_up(&preview)?;
            Change::Removed
        }
        // Some tools move the changed file over the existing one
        // instead of opening it for writing. So a move to can actually edit
        // the rating.
        EventKind::FileModificationComplete | EventKind::FileMovedTo => {
            crate::scan::preview::create_update_or_clean_one::<Exporter>(
                xmp,
                &xmp_file,
                &source,
                &target,
                fs,
                db,
            )
            .await?
        }
    };

    match change {
        Change::Added => {
            if let Some(im) = immich {
                im.set_dir_not_empty(preview.parent_dir())
            }
        }
        Change::Removed if no_previews_in(&preview.parent_dir()).await? => {
            if target != base_target.0 {
                tokio::fs::remove_dir(&preview.parent_dir())
                    .await
                    .ignore_err_if(|e| e.kind() == ErrorKind::DirectoryNotEmpty, ())
                    .wrap_err("Could not remove empty dir")
                    .note_path(&preview.parent_dir())?;
            }

            if let Some(im) = immich {
                im.set_dir_empty(preview.parent_dir());
            }
        }
        Change::None | Change::Removed => (),
    }

    Ok(())
}

pub async fn no_previews_in(dir: &TargetDir) -> color_eyre::Result<bool> {
    use std::future::ready;
    use tokio::fs;
    use tokio_stream::wrappers::ReadDirStream;

    let read_dir = fs::read_dir(dir)
        .await
        .wrap_err("Failed to start reading directory")
        .wrap_err("Could not check if directory has no more previews")
        .note_path(&dir)?;
    Ok(ReadDirStream::new(read_dir)
        .try_filter(|entry| ready(entry.path().extension() == Some(OsStr::new("jpg"))))
        .next()
        .await
        .transpose()
        .wrap_err("Failed to check directory entry")
        .wrap_err("Could not check if directory has no more previews")
        .note_path(&dir)?
        .is_none())
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

#[derive(Debug)]
pub struct Event {
    pub xmp_file: XmpFile,
    pub kind: EventKind,
}

#[derive(Debug)]
pub enum EventKind {
    FileModificationComplete,
    FileDeleted,
    FileMovedTo,
    FileMovedFrom,
}

struct Exiting;

fn forward_if_relevant(
    event: &FidEvent,
    dir: &BaseSourceDir,
    tx: &Sender<Event>,
    overflown: &AtomicBool,
) -> Result<(), Exiting> {
    // Must run fast, gets ran for each file on the mount
    let mut failed_to_forward = false;
    if let Some(ext) = event.path.extension()
        && ext == "xmp"
    {
        cold_path();
        let mut res = Ok(());
        if event.path.starts_with(dir) && event.path.is_file() {
            if event.mask & FAN_CLOSE_WRITE > 0 {
                res = res.and(tx.try_send(Event {
                    xmp_file: XmpFile(event.path.clone()),
                    kind: EventKind::FileModificationComplete,
                }));
            }
            if event.mask & FAN_DELETE > 0 {
                res = res.and(tx.try_send(Event {
                    xmp_file: XmpFile(event.path.clone()),
                    kind: EventKind::FileDeleted,
                }));
            }
            if event.mask & FAN_MOVED_FROM > 0 {
                res = res.and(tx.try_send(Event {
                    xmp_file: XmpFile(event.path.clone()),
                    kind: EventKind::FileMovedFrom,
                }))
            }
            if event.mask & FAN_MOVED_TO > 0 {
                res = res.and(tx.try_send(Event {
                    xmp_file: XmpFile(event.path.clone()),
                    kind: EventKind::FileMovedTo,
                }))
            }
        }

        failed_to_forward = match res {
            Ok(()) => false,
            Err(TrySendError::Closed(_)) => return Err(Exiting),
            Err(TrySendError::Full(_)) => true,
        };
    }
    overflown.store(failed_to_forward || event.is_overflow(), Ordering::Relaxed);
    Ok(())
}
