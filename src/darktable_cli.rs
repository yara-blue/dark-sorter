use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt, eyre};
use tokio::process;
use tokio::sync::Semaphore;
use tracing::debug;

use crate::fs::{MetadataExtExt, PreviewFile, RawFile, XmpFile};
use crate::watcher::{EyreWithPath, ResultExt};
use crate::{ImageExporter, ThrottledFs};

pub struct DarktableCli;

impl ImageExporter for DarktableCli {
    fn export(
        xmp_file: &XmpFile,
        input_file: &RawFile,
        output_file: &PreviewFile,
        fs: &ThrottledFs,
    ) -> impl Future<Output = color_eyre::Result<()>> + Send {
        export(xmp_file, input_file, output_file, fs)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct StringError(String);

/// Globally limit to one file at the time
pub async fn export(
    xmp_file: &XmpFile,
    input_file: &RawFile,
    output_file: &PreviewFile,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    // darktable export is already highly parallel
    static LIMIT_EXPORTS: Semaphore = Semaphore::const_new(1);

    let _permit = LIMIT_EXPORTS
        .acquire()
        .await
        .expect("static semaphore can not be closed");

    debug!("Exporting image: {input_file}");
    asses_file_state(&input_file, &output_file, fs).await?;

    let output = process::Command::new("nice")
        .arg("--adjustment=19")
        .arg("darktable-cli")
        .arg(&input_file)
        .arg(xmp_file)
        .arg(&output_file)
        .arg("--core")
        .arg("--library")
        .arg(":memory:") // don't create a darktable library file
        // can't stop darktable from getting configs give it a place to put them
        // it derives it's paths from home so we gotta give it one.
        .env("HOME", &darktable_home(fs)?)
        .uid(fs.user)
        .gid(fs.group)
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
            .with_note(|| format!("stdout was: \"{stdout}\""))
            .with_note(|| format!("input file: {}", input_file.display()))
            .with_note(|| format!("output_file: {}", output_file.display()))
            .with_note(|| format!("xmp_file: {}", xmp_file.display()))
    }
}

/// Darktable-cli's errors are not super helpful when im eepy. Let's give future
/// me some nice errors instead.
async fn asses_file_state(
    input_file: &RawFile,
    output_file: &PreviewFile,
    fs: &ThrottledFs,
) -> color_eyre::Result<()> {
    let input = fs
        .metadata(input_file)
        .await
        .wrap_err("Could not check input file")
        .note_path(input_file)?;
    if !input.is_file() {
        return Err(eyre!("Not a file")).note_path(input_file)?;
    }

    let readable = input.user_can_read(fs.user) || input.group_can_read(fs.group);
    if !readable {
        return Err(eyre!(
            "The darktable-cli user has no permission to read the file"
        ))
        .note_path(input_file)
        .suggestion("Change the files permissions or ownership")
        .suggestion("Ensure dark-sorter is set up to use the correct user")?;
    }

    let Some(output) = fs
        .metadata(output_file)
        .await
        .map(Some)
        .ignore_err_if(|e| e.kind() == ErrorKind::NotFound, None)
        .wrap_err("Could not check existing output file")
        .note_path(output_file)?
    else {
        return Ok(());
    };

    let writable = output.user_can_write(fs.user) || output.group_can_write(fs.group);
    if !writable {
        return Err(eyre!(
            "The darktable-cli user has no permission to write to the file"
        ))
        .note_path(input_file)
        .suggestion("Change the files permissions or ownership")
        .suggestion("Ensure dark-sorter is set up to use the correct user")?;
    }

    Ok(())
}

fn darktable_home(fs: &ThrottledFs) -> color_eyre::Result<PathBuf> {
    let dir = if crate::running_as_root() {
        Path::new("/var/cache").to_path_buf()
    } else {
        // isolate this from the "real" darktable home
        dirs::cache_dir().ok_or_eyre("Could not get user cache dir")?
    }
    .join(env!("CARGO_PKG_NAME"))
    .join("darktable_cache");

    std::fs::create_dir_all(&dir)
        .wrap_err("Could not setup dir for darktable 'home'")
        .with_note(|| format!("database dir: {}", dir.display()))?;

    std::os::unix::fs::chown(&dir, Some(fs.user), Some(fs.group))
        .wrap_err("Failed to set user and group for darktable 'home' dir")
        .note_path(&dir)?;
    Ok(dir)
}
