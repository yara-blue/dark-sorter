#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_errors_doc)]

// #[cfg(feature = "test_support")]
pub mod test_support;

mod darktable_cli;
mod database;
mod fs;

pub mod immich;
mod scan;
pub mod watcher;
mod xmp;
pub use xmp::Rating;

pub use darktable_cli::DarktableCli;
pub use database::Db;
pub use fs::{BaseSourceDir, BaseTargetDir, ThrottledFs};
pub use scan::scan_clean_and_link;
use tracing::{info, warn};

use crate::fs::{PreviewFile, RawFile, XmpFile};
use crate::immich::ImmichSync;

/// Only here so we can test without having to run darktable
pub trait ImageExporter: 'static {
    fn export(
        xmp_file: &XmpFile,
        input_file: &RawFile,
        output_file: &PreviewFile,
        fs: &fs::ThrottledFs,
    ) -> impl Future<Output = color_eyre::Result<()>> + Send;
}

/// Only here so we can test watch queue behavior without having to run as root
pub trait Watcher: 'static + Sized {
    fn clear(&mut self);
    fn next(&mut self) -> impl Future<Output = watcher::Event>;
    fn overflown(&self) -> bool;
}

#[must_use]
pub fn running_as_root() -> bool {
    caps::has_cap(
        None,
        caps::CapSet::Permitted,
        caps::Capability::CAP_SYS_ADMIN,
    )
    .expect("We should always be able to see if we are sys admin")
}

pub async fn main_loop<Exporter: ImageExporter>(
    source: BaseSourceDir,
    target: BaseTargetDir,
    fs: ThrottledFs,
    db: Db,
    immich_sync: Option<ImmichSync>,
    mut watcher: Option<impl Watcher>,
) -> color_eyre::Result<()> {
    let mut first_scan = true;
    loop {
        scan_clean_and_link::<Exporter>(
            source.clone(),
            target.clone(),
            fs.clone(),
            db.clone(),
            immich_sync.clone(),
        )
        .await?;

        let Some(ref mut watcher) = watcher else {
            info!("Scan complete");
            return Ok(());
        };
        if first_scan {
            info!("Initially scan complete, now watching for changes");
            first_scan = false;
        }

        loop {
            let event = watcher.next().await;
            if watcher.overflown() {
                warn!("Filesystem watcher overloaded, re-scanning to catch up");
                watcher.clear();
                break;
            }
            if let Some(ref immich_sync) = immich_sync
                && immich_sync.needs_rescan()
            {
                warn!("Immich sync overflowed and recovered, re-scanning to catch up");
                break;
            }

            watcher::handle_event::<Exporter>(
                event,
                &source,
                &target,
                &fs,
                &db,
                immich_sync.as_ref(),
            )
            .await?;
        }
    }
}
