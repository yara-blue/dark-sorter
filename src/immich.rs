use std::collections::BTreeSet;
use std::collections::HashMap;
use std::future::pending;
use std::panic;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use crate::fs::BaseTargetDir;
use crate::fs::TargetDir;
use crate::immich::client::LibraryId;
pub use client::ApiKey;
use client::Immich;

use futures::FutureExt as _;
use futures_concurrency::future::FutureExt;
use reqwest::Url;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time;
use tracing::debug;
use tracing::instrument;

mod client;

fn name_for_dir(dir: &TargetDir, base_dir: &BaseTargetDir) -> String {
    format!(
        "x-dark-sorter-{}", // x: to get these to the bottom of the list in immich
        dir.0
            .0
            .strip_prefix(&base_dir.0.0)
            .expect("dir was a full path so includes target_dir")
            .display()
    )
}

#[derive(Debug, Clone)]
pub struct ImmichSync {
    tx: mpsc::Sender<Event>,
    overflown: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<JoinHandle<color_eyre::Result<ImmichHandleDropped>>>>>,
}

impl ImmichSync {
    pub fn set_dir_empty(&self, dir: TargetDir) {
        match self.tx.try_send(Event::EmptyDir(dir)) {
            Err(TrySendError::Full(_)) => self.overflown.store(true, Ordering::Relaxed),
            Err(TrySendError::Closed(_)) => self.report_error_or_continue_panic(),
            Ok(_) => (),
        }
    }

    pub fn set_dir_not_empty(&self, dir: TargetDir) {
        match self.tx.try_send(Event::NonEmptyDir(dir)) {
            Err(TrySendError::Full(_)) => self.overflown.store(true, Ordering::Relaxed),
            Err(TrySendError::Closed(_)) => self.report_error_or_continue_panic(),
            Ok(_) => (),
        }
    }
    /// If overflown is true we could not send some messages because the buffer
    /// was full. This will happen is immich goes down for a while. Then it will
    /// need a re-scan to get in sync again.
    pub fn needs_rescan(&self) -> bool {
        if self.overflown.load(Ordering::Relaxed) && self.tx.capacity() > self.tx.max_capacity() / 2
        {
            self.overflown.store(false, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
    fn report_error_or_continue_panic(&self) {
        let Ok(Some(thread)) = self.thread.lock().as_deref_mut().map(Option::take) else {
            return; // already resumed sync panic or reported sync error
        };
        for _ in 0..10 {
            if thread.is_finished() {
                break;
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        if !thread.is_finished() {
            unreachable!("ImmichSync closed mpsc without panicking");
        }

        match thread.join() {
            Ok(Ok(ImmichHandleDropped)) => unreachable!("this only happens after the rx drops"),
            Ok(Err(report)) => {
                tracing::error!("Immich sync ran into unrecoverable error: {report}")
            }
            Err(panic) => panic::resume_unwind(panic),
        }
    }

    pub async fn start(
        url: Url,
        api_key: ApiKey,
        base_dir: &BaseTargetDir,
    ) -> color_eyre::Result<ImmichSync> {
        let client = client::Immich::new(url, api_key).await?;
        // Don't make the buffer too large, we might never empty it if immich is down.
        let (tx, rx) = mpsc::channel(512);
        let base_dir = base_dir.clone();
        let overflown = Arc::new(AtomicBool::new(false));

        let thread = std::thread::spawn(move || {
            tokio::runtime::LocalRuntime::new()
                .unwrap()
                .block_on(async move { maintain_immich_sync(client, base_dir, rx).await })
        });
        Ok(ImmichSync {
            tx,
            overflown,
            thread: Arc::new(Mutex::new(Some(thread))),
        })
    }
}

// TODO do not symlink, just write the preview to the target dir

#[derive(Debug)]
pub enum Event {
    EmptyDir(TargetDir),
    NonEmptyDir(TargetDir),
    PendingScan(LibraryId),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingScan {
    triggers_at: time::Instant,
    library_id: LibraryId,
}

async fn next_pending(pending_scans: &mut BTreeSet<PendingScan>) -> LibraryId {
    if let Some(pending) = pending_scans.first() {
        time::sleep_until(pending.triggers_at).await;
    } else {
        pending::<()>().await
    }

    pending_scans
        .pop_first()
        .expect("guarded by if (the non arm diverges through the await)")
        .library_id
}

struct ImmichHandleDropped;

async fn maintain_immich_sync(
    mut immich: Immich,
    base_dir: BaseTargetDir,
    mut rx: tokio::sync::mpsc::Receiver<Event>,
) -> color_eyre::Result<ImmichHandleDropped> {
    let mut pending_scans = BTreeSet::new();
    let mut libs: HashMap<TargetDir, ManagedLibrary> =
        get_managed_libraries(&mut immich, &base_dir)
            .await?
            .map(|lib| (lib.import_path.clone(), lib))
            .collect();
    while let Some(event) = rx
        .recv()
        .race(
            next_pending(&mut pending_scans)
                .map(Event::PendingScan)
                .map(Some),
        )
        .await
    {
        tracing::debug!("immich sync event: {event:?}");
        match event {
            Event::EmptyDir(path) => {
                if let Some(lib) = libs.remove(&path) {
                    immich.delete_library(&lib.id).await?;
                }
            }
            Event::NonEmptyDir(path) => {
                let lib = if let Some(lib) = libs.get_mut(&path) {
                    lib
                } else {
                    let new = add_managed_library(path.clone(), &base_dir, &mut immich).await?;
                    libs.insert(new.import_path.clone(), new);
                    libs.get_mut(&path).expect("just inserted")
                };
                if lib.last_scanned.is_none_or(|t| t.elapsed().as_secs() > 30) {
                    lib.last_scanned = Some(Instant::now());
                    immich.update_library(&lib.id).await?;
                } else {
                    pending_scans.insert(PendingScan {
                        triggers_at: time::Instant::now() + Duration::from_secs(30),
                        library_id: lib.id.clone(),
                    });
                }
            }
            Event::PendingScan(id) => {
                immich.update_library(&id).await?;
            }
        }
    }

    Ok(ImmichHandleDropped)
}

#[instrument]
async fn add_managed_library(
    dir: TargetDir,
    base_dir: &BaseTargetDir,
    immich: &mut Immich,
) -> color_eyre::Result<ManagedLibrary> {
    debug!("adding new library to immich");
    let name = format!(
        "x-dark-sorter-{}", // x: to get these to the bottom of the list in immich
        dir.relative_to_base(base_dir).display()
    );
    let lib = immich.create_library(Vec::new(), &dir, name).await?;
    assert_eq!(lib.import_paths.first(), Some(&dir.display().to_string()));
    Ok(ManagedLibrary {
        id: lib.id,
        import_path: dir,
        last_scanned: None,
    })
}

struct ManagedLibrary {
    // Library ID
    id: LibraryId,
    // Paths immich checks for images
    import_path: TargetDir,
    // When did we last issue a scan?
    last_scanned: Option<Instant>,
}

async fn get_managed_libraries(
    client: &mut Immich,
    base_dir: &BaseTargetDir,
) -> color_eyre::Result<impl Iterator<Item = ManagedLibrary>> {
    Ok(client
        .get_all_libraries()
        .await?
        .into_iter()
        .filter_map(|lib| {
            if lib.owner_id == client.id
                && let Some(path) = lib.import_paths.first()
                && let Ok(dir) = TargetDir::try_new(path, base_dir)
                && lib.name == name_for_dir(&dir, base_dir)
            {
                Some(ManagedLibrary {
                    id: lib.id,
                    import_path: dir,
                    last_scanned: None,
                })
            } else {
                None
            }
        }))
}
