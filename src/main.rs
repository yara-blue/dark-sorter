use std::path::PathBuf;

use clap::{Parser, ValueHint};
use dark_sorter::{Db, ThrottledFs, scan_clean_and_link};

/// Scans a folder tree and creates a sibling folder structure of
/// symlinks to jpg previews for all the photos that where rated
/// in the scanned tree.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Folder tree where the RAWs and darktable xmp files are.
    #[arg(short, long, value_hint=ValueHint::DirPath)]
    source_dir: PathBuf,

    /// Folder in which sibling structure should be build and previews linked
    #[arg(short, long, value_hint=ValueHint::DirPath)]
    target_dir: PathBuf,

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
    scan_clean_and_link(cli.source_dir, cli.target_dir, fs, db).await?;

    if cli.daemon {
        // figure out last changed dirs;
        
        // sleep
        // scan
    }

    Ok(())
}
