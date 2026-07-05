use std::path::PathBuf;

use clap::{Parser, ValueHint};
use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt};
use dark_sorter::immich::ImmichSync;
use dark_sorter::watcher::EyreWithPath;
use dark_sorter::{
    BaseSourceDir, BaseTargetDir, DarktableCli, Db, ThrottledFs, immich, running_as_root, watcher,
};
use reqwest::Url;
use tracing::{info, warn};
use tracing_error::ErrorLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Scans a folder tree and creates a sibling folder structure of
/// symlinks to jpg previews for all the photos that where rated
/// in the scanned tree.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Folder tree where the RAWs and darktable xmp files are.
    #[arg(short, long, value_hint=ValueHint::DirPath)]
    source_dir: BaseSourceDir,

    /// Folder in which sibling structure should be build and previews linked
    #[arg(short, long, value_hint=ValueHint::DirPath)]
    target_dir: BaseTargetDir,

    /// User that will create the files.
    /// Defaults to the current user if not set
    #[arg(short, long, value_hint=ValueHint::Username)]
    user: Option<String>,

    /// Group for the files created by dark sorter.
    /// Defaults to the current users group
    #[arg(short, long)]
    photo_group: Option<String>,

    /// Refresh library on this immich instance
    #[arg(long, group = "url", requires = "api_key", value_hint=ValueHint::Url)]
    immich_url: Option<Url>,

    /// Refresh library on this immich instance
    #[arg(long, group = "url", requires = "api_key", value_hint=ValueHint::FilePath)]
    immich_url_path: Option<PathBuf>,

    /// API key for the immich instance. Get it here:
    /// `https://my.immich.app/user-settings?isOpen=api-keys`. The API key needs
    /// permissions: library.create, library.update, library.read,
    /// library.delete & users.read,  
    #[arg(short = 'a', long, group = "api_key", requires = "url")]
    immich_api_key: Option<immich::ApiKey>,

    /// File containing the API key for the immich instance. Get it here:
    /// `https://my.immich.app/user-settings?isOpen=api-keys`. The API key needs
    /// permissions: library.create, library.update, library.read,
    /// library.delete & users.read,  
    #[arg(long, group = "api_key", requires = "url", value_hint=ValueHint::FilePath)]
    immich_api_key_path: Option<PathBuf>,

    /// Watch files after scan, requires dark-sorter to run as root.
    #[arg(short, long)]
    daemon: bool,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::registry()
        .with(fmt::layer().pretty())
        .with(EnvFilter::from_default_env())
        .with(ErrorLayer::default())
        .init();

    let cli = Cli::parse();
    info!("Started dark-sorter");

    let user = if let Some(name) = cli.user {
        uzers::get_user_by_name(&name)
            .ok_or_eyre("User not found on system")?
            .uid()
    } else {
        uzers::get_current_uid()
    };
    let group = if let Some(name) = cli.photo_group {
        uzers::get_group_by_name(&name)
            .ok_or_eyre("Group not found on system")?
            .gid()
    } else {
        if running_as_root() {
            warn!("Links will only be readable by the root user");
        }
        uzers::get_current_gid()
    };

    let fs = ThrottledFs::new(user, group)?;
    let db = Db::load_from_default_dir_or_create().await?;

    let watcher = cli
        .daemon
        .then_some(watcher::FanotifyWatcher::start(cli.source_dir.clone())?);

    let url = cli
        .immich_url_path
        .map(|path| {
            let url = std::fs::read_to_string(&path)
                .wrap_err("Failed to read file")
                .note_path(path)?;
            Url::parse(&url)
                .wrap_err("Content of file is not a url")
                .with_note(|| format!("file content: `{url}`"))
        })
        .transpose()
        .wrap_err("Could not read Immich url from file")?
        .or(cli.immich_url);

    let api_key = cli
        .immich_api_key_path
        .map(|path| {
            std::fs::read_to_string(&path)
                .map(|s| s.trim().to_string())
                .wrap_err("Could not read Immich API key from file")
                .note_path(path)
        })
        .transpose()?
        .map(immich::ApiKey)
        .or(cli.immich_api_key);

    let immich_sync = if let Some((url, api_key)) = url.zip(api_key) {
        Some(
            ImmichSync::start(url, api_key, &cli.target_dir)
                .await
                .wrap_err("Could not start immich sync")?,
        )
    } else {
        None
    };

    dark_sorter::main_loop::<DarktableCli>(
        cli.source_dir,
        cli.target_dir,
        fs,
        db,
        immich_sync,
        watcher,
    )
    .await
}
