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

pub use darktable_cli::DarktableCli;
pub use database::Db;
pub use fs::{BaseSourceDir, BaseTargetDir, ThrottledFs};
pub use scan::scan_clean_and_link;

use crate::fs::{PreviewFile, RawFile, XmpFile};

/// Only here so we can test without having to run darktable
pub trait ImageExporter: 'static {
    fn export(
        xmp_file: &XmpFile,
        input_file: &RawFile,
        output_file: &PreviewFile,
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
