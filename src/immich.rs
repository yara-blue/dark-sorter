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
use crate::fs::PreviewFile;
use crate::fs::TargetDir;
use crate::immich::client::AssetId;
use crate::immich::client::ExternalLibraryId;
use crate::immich::client::SearchFilters;
pub use client::ApiKey;
use client::Immich;

use futures::FutureExt as _;
use futures_concurrency::future::FutureExt;
use itertools::Itertools;
use reqwest::Url;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time;
use tracing::debug;
use tracing::instrument;
use tracing::warn;

mod client;

fn lib_name_for_dir(dir: &TargetDir, base_dir: &BaseTargetDir) -> String {
    format!(
        "z-dark-sorter:/{}", // z: to get these to the bottom of any alphabetical list
        dir.relative_to_base(base_dir).display()
    )
}

#[derive(Debug, Clone)]
pub struct ImmichSync {
    tx: mpsc::Sender<Event>,
    overflown: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<JoinHandle<color_eyre::Result<ImmichHandleDropped>>>>>,
}

impl ImmichSync {
    fn mark_overflow(&self) {
        self.overflown.store(true, Ordering::Relaxed);
        warn!("Immich sync overflown, dropped some events. Will rescan when it catches up")
    }

    #[instrument(skip(self))]
    pub fn signal_dir_empty(&self, dir: TargetDir) {
        debug!("ImmichSync: marking dir as empty");
        match self.tx.try_send(Event::EmptyDir(dir)) {
            Err(TrySendError::Full(_)) => self.mark_overflow(),
            Err(TrySendError::Closed(_)) => self.report_error_or_continue_panic(),
            Ok(_) => (),
        }
    }

    #[instrument(skip(self))]
    pub fn signal_file_modified_or_added(&self, file: PreviewFile) {
        debug!("ImmichSync: marking dir as not empty");
        match self.tx.try_send(Event::ModifiedOrAdded(file)) {
            Err(TrySendError::Full(_)) => self.mark_overflow(),
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
                tracing::error!("Immich sync ran into unrecoverable error: {report:?}")
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
    ModifiedOrAdded(PreviewFile),
    PendingScan(PendingScan),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PendingScan {
    triggers_at: time::Instant,
    library_id: ExternalLibraryId,
    paths: Vec<PreviewFile>,
}

async fn next_pending(pending_scans: &mut Vec<PendingScan>) -> PendingScan {
    let Some(idx) = pending_scans
        .iter()
        .position_min_by_key(|ps| ps.triggers_at)
    else {
        pending::<()>().await;
        unreachable!();
    };

    let pending = pending_scans.swap_remove(idx);
    time::sleep_until(pending.triggers_at).await;
    pending
}

struct ImmichHandleDropped;

async fn maintain_immich_sync(
    mut immich: Immich,
    base_dir: BaseTargetDir,
    mut rx: tokio::sync::mpsc::Receiver<Event>,
) -> color_eyre::Result<ImmichHandleDropped> {
    let mut pending_scans = Vec::new();
    let mut libs: HashMap<TargetDir, ManagedLibrary> =
        get_managed_libraries(&mut immich, &base_dir)
            .await?
            .map(|lib| (lib.import_path.clone(), lib))
            .collect();
    tracing::debug!("Existing immich libs: {libs:?}");

    loop {
        tracing::debug!("waiting for immich sync event");
        let Some(event) = rx
            .recv()
            .race(
                next_pending(&mut pending_scans)
                    .map(Event::PendingScan)
                    .map(Some),
            )
            .await
        else {
            break;
        };
        tracing::debug!("immich sync event: {event:?}");
        match event {
            Event::EmptyDir(path) => {
                if let Some(lib) = libs.remove(&path) {
                    immich.delete_library(&lib.id).await?;
                }
            }
            Event::ModifiedOrAdded(preview_file) => {
                let dir = preview_file.parent_dir();
                let lib = if let Some(lib) = libs.get_mut(&dir) {
                    lib
                } else {
                    let new = add_managed_library(dir.clone(), &base_dir, &mut immich).await?;
                    libs.insert(new.import_path.clone(), new);
                    libs.get_mut(&dir).expect("just inserted")
                };

                if let Some(pending) = pending_scans.iter_mut().find(|p| p.library_id == lib.id) {
                    pending.paths.push(preview_file)
                } else {
                    let scan_in = if let Some(t) = lib.last_scanned
                        && t.elapsed().as_secs() > 30
                    {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(30)
                    };

                    pending_scans.push(PendingScan {
                        triggers_at: time::Instant::now() + scan_in,
                        library_id: lib.id.clone(),
                        paths: vec![preview_file],
                    });
                }
            }
            Event::PendingScan(pending) => {
                immich.update_library(&pending.library_id).await?;
                // TODO rework this to use tasks, store handles in pending scans?
                // OR schedule yet another follow up that does the polling whether
                // the scan has completed...
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
    let name = lib_name_for_dir(&dir, base_dir);
    let lib = immich.create_library(Vec::new(), &dir, name).await?;
    assert_eq!(lib.import_paths.first(), Some(&dir.display().to_string()));
    Ok(ManagedLibrary {
        id: lib.id,
        import_path: dir,
        last_scanned: None,
    })
}

#[derive(Debug)]
struct ManagedLibrary {
    // Library ID
    id: ExternalLibraryId,
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
                && lib.name.starts_with("z-dark-sorter:/")
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

async fn get_asset_from_path(
    immich: &mut Immich,
    path: &PreviewFile,
    library_id: ExternalLibraryId,
) -> color_eyre::Result<AssetId> {
    let mut filters = SearchFilters::default();
    filters.library_id = Some(library_id);
    filters.original_path = Some(path.0.clone());
    let mut response = immich.search_assets(filters).await?;

    assert_eq!(
        response.assets.items.len(),
        1,
        "External libraries can only have a single asset for each path"
    );
    Ok(response.assets.items.pop().expect("see assert above").id)
}
