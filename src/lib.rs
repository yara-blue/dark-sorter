#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_errors_doc)]

// #[cfg(feature = "test_support")]
pub mod test_support;

mod darktable_cli;
mod database;
mod fs;

mod scan;
pub mod watcher;
mod xmp;
pub mod immich;

pub use darktable_cli::DarktableCli;
pub use database::Db;
pub use fs::{BaseSourceDir, BaseTargetDir, ThrottledFs};
pub use scan::scan_clean_and_link;

use crate::fs::XmpFile;
use crate::xmp::Xmp;

/// Only here so we can test without having to run darktable
pub trait ImageExporter: 'static {
    fn export(
        xmp: &Xmp,
        xmp_file: &XmpFile,
        source: impl AsRef<fs::SourceDir> + Send,
        fs: &fs::ThrottledFs,
    ) -> impl Future<Output = color_eyre::Result<()>> + Send;
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
