use clap::{Parser, ValueHint};
use color_eyre::eyre::OptionExt;
use dark_sorter::immich::ImmichSync;
use dark_sorter::{
    BaseSourceDir, BaseTargetDir, DarktableCli, Db, ThrottledFs, immich, running_as_root,
    scan_clean_and_link, watcher,
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
    #[arg(short, long, requires = "immich_api_key", value_hint=ValueHint::Url)]
    immich_url: Option<Url>,

    /// API key for th immich instance
    /// Get it here: https://my.immich.app/user-settings?isOpen=api-keys
    /// It needs: library.create, library.update, library.read, library.delete & users.read,  
    #[arg(short = 'a', long, requires = "immich_url")]
    immich_api_key: Option<immich::ApiKey>,

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

    let event_rx = cli
        .daemon
        .then_some(watcher::start(cli.source_dir.clone())?);

    let immich_sync = match (cli.immich_url, cli.immich_api_key) {
        (None, None) => None,
        (Some(url), Some(api_key)) => Some(ImmichSync::start(url, api_key, &cli.target_dir).await?),
        (None, Some(_)) | (Some(_), None) => unreachable!("from the same arg group"),
    };

    let mut first_scan = true;
    loop {
        scan_clean_and_link::<DarktableCli>(
            cli.source_dir.clone(),
            cli.target_dir.clone(),
            fs.clone(),
            db.clone(),
            immich_sync.clone(),
        )
        .await?;

        let Some(ref event_rx) = event_rx else {
            break Ok(());
        };
        if first_scan {
            info!("Initially scan complete");
            first_scan = false;
        }

        for event in event_rx {
            if event.overflow {
                warn!("watcher overflowed");
                let _ = event_rx.try_iter().count();
                break;
            }
            if let Some(ref immich_sync) = immich_sync
                && immich_sync.needs_rescan()
            {
                warn!("immich sync overflowed and has recovered, re-scanning");
                break;
            }
            watcher::handle_kitty_fs_change::<DarktableCli>(
                event,
                &cli.source_dir,
                &cli.target_dir,
                &fs,
                &db,
                immich_sync.as_ref(),
            )
            .await?;
            warn!("Filesystem watcher overloaded, re-scanning");
        }
    }
}
