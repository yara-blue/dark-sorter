use std::path::Path;

use color_eyre::Section;
use color_eyre::eyre::{Context, eyre};
use tokio::process;
use tokio::sync::Semaphore;

use crate::SourceDir;
use crate::xmp::Xmp;

// TODO limit simultaneous open files in program with semaphore

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct StringError(String);

/// Globally limit to one file at the time
pub async fn export(xmp: Xmp, xmp_file: &Path, source: &SourceDir) -> color_eyre::Result<()> {
    // darktable export is already highly parallel
    static LIMIT_EXPORTS: Semaphore = Semaphore::const_new(1);

    let _permit = LIMIT_EXPORTS
        .acquire()
        .await
        .expect("static semaphore can not be closed");

    let input_file = source.join(&*xmp.raw);
    let output_file = input_file.with_extension("jpg");
    let output = process::Command::new("nice")
        .arg("19")
        .arg("darktable-cli")
        .arg(input_file.as_os_str())
        .arg(xmp_file.as_os_str())
        .arg(output_file.as_os_str())
        .output()
        .await
        .wrap_err("Could not spawn darktable-cli export process")?;

    if output.status.success() {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(eyre!("darktable-cli failed"))
            .error(StringError(stderr))
            .with_note(|| format!("stdout was: {stdout}"))
            .with_note(|| format!("input file: {}", input_file.display()))
            .with_note(|| format!("output_file: {}", output_file.display()))
            .with_note(|| format!("xmp_file: {}", xmp_file.display()))
    }
}
