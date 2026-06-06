use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::xmp;
use color_eyre::Section;
use color_eyre::eyre::{Context, ContextCompat, OptionExt};
use flate2::Compression;
use tokio::task::JoinHandle;


#[derive(Debug, Clone)]
pub struct Db(Arc<spin::Mutex<HashMap<PathBuf, xmp::EditHash>>>);

#[derive(Debug, thiserror::Error)]
pub enum LoadDbError {
    #[error("Could not get path for user data dir")]
    GetDataPath,
    #[error("Db file not found")]
    NotFound,
    #[error("Io error opening file")]
    Opening(Arc<std::io::Error>),
    #[error("Could not deserialize compressed")]
    DeserComprr(postcard::Error),
}

impl LoadDbError {
    fn from_io(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound,
            _ => Self::Opening(Arc::new(err)),
        }
    }
}

impl Db {
    pub fn new() -> Self {
        Self(Arc::new(spin::Mutex::new(HashMap::new())))
    }

    pub fn get(&self, path: &Path) -> Option<xmp::EditHash> {
        self.0.lock().get(path).copied()
    }

    pub fn insert(&self, path: PathBuf, hash: xmp::EditHash) {
        self.0.lock().insert(path, hash);
    }

    pub async fn load_or_create() -> color_eyre::Result<Self> {
        let path = db_file_path()?;
        match Self::load_from_file(path.clone()).await {
            Ok(db) => Ok(db),
            Err(LoadDbError::NotFound) => Ok(Self::new()),
            Err(other) => Err(other)
                .wrap_err("Could not load db")
                .with_note(|| format!("path: {}", path.display())),
        }
    }

    pub async fn load_from_file(path: PathBuf) -> Result<Self, LoadDbError> {
        let task = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(path).map_err(LoadDbError::from_io)?;
            let file = BufReader::new(file);
            let decompressor = flate2::read::ZlibDecoder::new(file);

            let mut buffer = [0; 1024];
            let (map, _): (HashMap<PathBuf, xmp::EditHash>, _) =
                postcard::from_io((decompressor, &mut buffer)).map_err(LoadDbError::DeserComprr)?;
            Ok(map)
        });
        let map = task
            .await
            .expect("de-serialization and compression should never panic")?;
        Ok(Self(Arc::new(spin::Mutex::new(map))))
    }

    /// Can only be called when nothing else holds a lock to the map anymore if
    /// something did the critical section of the lock would become too long.
    pub async fn store_to_disk(mut self) -> color_eyre::Result<()> {
        let path: Arc<Path> = db_file_path()?.into();
        let path2 = Arc::clone(&path);
        let task: JoinHandle<color_eyre::Result<()>> = tokio::task::spawn_blocking(move || {
            let map = Arc::get_mut(&mut self.0)
                .ok_or_eyre("Store to disk can only run when this is the last instance of the Db")?
                .try_lock()
                .expect("Arc::get_mut, guarantees no one else has the mutex");
            let file = std::fs::File::create(&path).wrap_err("Could not open db files")?;
            let file = std::io::BufWriter::new(file);
            // use ZLIB as it has no check-summing (we don't need any, any
            // corruption will just lead to an image being re-exported not data
            // loss)
            let compressor = flate2::write::ZlibEncoder::new(file, Compression::best());
            let compressor = postcard::to_io(&*map, compressor)
                .wrap_err("Failed to encode to compressed file")?;
            compressor
                .finish()
                .wrap_err("Failed to flush compressed file")?;
            Ok(())
        });
        task.await
            .wrap_err("compress task panicked")?
            .with_note(|| format!("path: {}", path2.display()))?;
        Ok(())
    }
}

fn db_file_path() -> color_eyre::Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .wrap_err("Could not get user data dir")?
        .join(env!("CARGO_PKG_NAME"))
        .join("db.bitcode"))
}

