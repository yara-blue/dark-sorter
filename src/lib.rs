// #[cfg(feature = "test_support")]
pub mod test_support;

mod darktable_cli;
mod database;
mod fs;

mod scan;
pub mod watcher;
mod xmp;

pub use darktable_cli::DarktableCli;
pub use database::Db;
pub use fs::{SourceDir, TargetDir, ThrottledFs};
pub use scan::scan_clean_and_link;

use crate::fs::XmpFile;
use crate::xmp::Xmp;

/// Only here so we can test without having to run darktable
pub trait ImageExporter: 'static {
    fn export(
        xmp: &Xmp,
        xmp_file: &XmpFile,
        source: &SourceDir,
    ) -> impl Future<Output = color_eyre::Result<()>> + Send;
}
