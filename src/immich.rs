use std::collections::BTreeSet;
use std::collections::HashMap;
use std::future::pending;
use std::time::Duration;
use std::time::Instant;

use crate::fs::BaseTargetDir;
use crate::fs::TargetDir;
pub use client::ApiKey;
use client::{Immich, Uuid};

use futures::FutureExt as _;
use futures_concurrency::future::FutureExt;
use reqwest::Url;
use tokio::sync::mpsc;

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

pub async fn start_sync_daemon(
    url: Url,
    api_key: ApiKey,
    base_dir: &BaseTargetDir,
) -> color_eyre::Result<mpsc::Sender<Event>> {
    let client = client::Immich::new(url, api_key).await?;
    let (tx, rx) = mpsc::channel(4096);
    let base_dir = base_dir.clone();
    tokio::task::spawn(maintain_immich_sync(client, base_dir, rx));
    Ok(tx)
}

// TODO do not symlink, just write the preview to the target dir

pub enum Event {
    EmptyDir(TargetDir),
    NonEmptyDir(TargetDir),
    PendingScan(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingScan {
    triggers_at: tokio::time::Instant,
    library_id: Uuid,
}

async fn next_pending(pending_scans: &mut BTreeSet<PendingScan>) -> Uuid {
    if let Some(pending) = pending_scans.first() {
        tokio::time::sleep_until(pending.triggers_at).await;
    } else {
        pending::<()>().await
    }

    pending_scans
        .pop_first()
        .expect("guarded by if (the non arm diverges through the await)")
        .library_id
}

async fn maintain_immich_sync(
    mut immich: Immich,
    base_dir: BaseTargetDir,
    mut rx: tokio::sync::mpsc::Receiver<Event>,
) -> color_eyre::Result<()> {
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
                        triggers_at: tokio::time::Instant::now() + Duration::from_secs(30),
                        library_id: lib.id.clone(),
                    });
                }
            }
            Event::PendingScan(id) => {
                immich.update_library(&id).await?;
            }
        }
    }

    Ok(())
}

async fn add_managed_library(
    dir: TargetDir,
    base_dir: &BaseTargetDir,
    immich: &mut Immich,
) -> color_eyre::Result<ManagedLibrary> {
    let name = format!(
        "x-dark-sorter-{}", // x: to get these to the bottom of the list in immich
        dir.relative_to_base(base_dir).display()
    );
    // TODO FINISH THIS THEN MIGRATE TO NON SYMLINKS THEN HOOK
    // UP THE TX TO BOTH SCAN AND WATCHER
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
    id: Uuid,
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
