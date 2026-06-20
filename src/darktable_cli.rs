use std::sync::LazyLock;

use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt, eyre};
use tokio::process;
use tokio::sync::Semaphore;
use uzers::User;

use crate::ImageExporter;
use crate::fs::{SourceDir, XmpFile};
use crate::xmp::Xmp;

pub struct DarktableCli;

impl ImageExporter for DarktableCli {
    fn export(
        xmp: &Xmp,
        xmp_file: &XmpFile,
        source: &SourceDir,
    ) -> impl Future<Output = color_eyre::Result<()>> + Send {
        export(xmp, xmp_file, source)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct StringError(String);

pub fn running_as_root() -> bool {
    caps::has_cap(
        None,
        caps::CapSet::Permitted,
        caps::Capability::CAP_SYS_ADMIN,
    )
    .expect("We should always be able to see if we are sys admin")
}

fn darktable_user() -> color_eyre::Result<&'static User> {
    static DARKTABLE_USER: LazyLock<Option<uzers::User>> =
        LazyLock::new(|| uzers::get_user_by_name("darktable"));
    DARKTABLE_USER
        .as_ref()
        .ok_or_eyre("Could not find user `darktable`")
        .note("when running as root we need a user darktable to run darktable-cli under")
        .suggestion("Add a user `darktable`")
}

/// Globally limit to one file at the time
pub async fn export(xmp: &Xmp, xmp_file: &XmpFile, source: &SourceDir) -> color_eyre::Result<()> {
    // darktable export is already highly parallel
    static LIMIT_EXPORTS: Semaphore = Semaphore::const_new(1);

    let _permit = LIMIT_EXPORTS
        .acquire()
        .await
        .expect("static semaphore can not be closed");

    let input_file = source.join(&*xmp.raw);
    let output_file = input_file.with_extension("jpg");

    let mut export_cmd = process::Command::new("nice");
    let export_cmd = export_cmd
        .arg("--adjustment=19")
        .arg("darktable-cli")
        .arg(input_file.as_os_str())
        .arg(xmp_file.0.as_os_str())
        .arg(output_file.as_os_str());
    if running_as_root() {
        export_cmd.uid(darktable_user()?.uid());
        export_cmd.gid(darktable_user()?.primary_group_id());
    }
    let output = export_cmd
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
