use clap::{Parser, ValueHint};
use color_eyre::eyre::OptionExt;
use dark_sorter::{
    DarktableCli, Db, SourceDir, TargetDir, ThrottledFs, running_as_root, scan_clean_and_link,
    watcher,
};
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
    source_dir: SourceDir,

    /// Folder in which sibling structure should be build and previews linked
    #[arg(short, long, value_hint=ValueHint::DirPath)]
    target_dir: TargetDir,

    /// User that will create the files.
    /// Defaults to the current user if not set
    #[arg(short, long, value_hint=ValueHint::Username)]
    user: Option<String>,

    /// Group for the files created by dark sorter.
    /// Defaults to the current users group
    #[arg(short, long)]
    photo_group: Option<String>,

    /// Refresh library on this immich instance
    #[arg(short, long, group = "immich")]
    immich_url: Option<String>,

    /// API key for th immich instance
    #[arg(short = 'a', long, group = "immich")]
    immich_api_key: Option<String>,

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

    let mut first_scan = true;
    loop {
        scan_clean_and_link::<DarktableCli>(
            cli.source_dir.clone(),
            cli.target_dir.clone(),
            fs.clone(),
            db.clone(),
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
            watcher::handle_kitty_fs_change::<DarktableCli>(
                event,
                &cli.source_dir,
                &cli.target_dir,
                &fs,
                &db,
            )
            .await?;
            warn!("Filesystem watcher overloaded, re-scanning");
        }
    }
}
