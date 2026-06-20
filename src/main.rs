use clap::{Parser, ValueHint};
use dark_sorter::{
    DarktableCli, Db, SourceDir, TargetDir, ThrottledFs, scan_clean_and_link, watcher,
};

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

    /// Maintain the state post scanning?
    #[arg(short, long)]
    daemon: bool,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();

    let fs = ThrottledFs::new()?;
    let db = Db::load_from_default_dir_or_create().await?;
    scan_clean_and_link::<DarktableCli>(
        cli.source_dir.clone(),
        cli.target_dir.clone(),
        fs.clone(),
        db.clone(),
    )
    .await?;

    if !cli.daemon {
        return Ok(());
    }

    let event_rx = watcher::start(cli.source_dir.clone())?;
    loop {
        scan_clean_and_link::<DarktableCli>(
            cli.source_dir.clone(),
            cli.target_dir.clone(),
            fs.clone(),
            db.clone(),
        )
        .await?;
        for event in event_rx.iter() {
            if event.overflow {
                let _ = event_rx.try_iter().count();
                break;
            }
            watcher::handle_kitty_fs_change::<DarktableCli>(
                event,
                &cli.source_dir,
                &cli.target_dir,
                &fs,
            )
            .await?;
        }
    }
}
