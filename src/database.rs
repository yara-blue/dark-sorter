use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::fs::XmpFile;
use crate::watcher::EyreWithPath;
use crate::xmp;
use color_eyre::Section;
use color_eyre::eyre::{Context, ContextCompat, OptionExt};
use flate2::Compression;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Default)]
pub struct Db(Arc<spin::Mutex<HashMap<XmpFile, xmp::EditHash>>>);

#[derive(Debug, thiserror::Error)]
pub enum LoadDbError {
    #[error("Could not get path for user data dir")]
    GetDataPath,
    #[error("Db file not found")]
    NotFound,
    #[error("Io error opening file")]
    Opening(#[source] OpenSharedError),
    #[error("Could not deserialize compressed")]
    DeserComprr(#[source] postcard::Error),
}

impl From<OpenSharedError> for LoadDbError {
    fn from(e: OpenSharedError) -> Self {
        if let OpenSharedError::IoOpening(e) = &e
            && e.kind() == ErrorKind::NotFound
        {
            Self::NotFound
        } else {
            Self::Opening(e)
        }
    }
}

impl Db {
    #[must_use]
    pub fn get(&self, path: &XmpFile) -> Option<xmp::EditHash> {
        self.0.lock().get(path).copied()
    }

    pub fn insert(&self, path: XmpFile, hash: xmp::EditHash) {
        self.0.lock().insert(path, hash);
    }

    pub async fn load_from_default_dir_or_create() -> color_eyre::Result<Self> {
        let path = setup_db_file_path()?;
        match Self::load_from_file(path.clone()).await {
            Ok(db) => Ok(db),
            Err(LoadDbError::NotFound) => Ok(Self::default()),
            Err(other) => Err(other).wrap_err("Could not load db").note_path(path),
        }
    }

    pub async fn load_from_file(path: PathBuf) -> Result<Self, LoadDbError> {
        let task = tokio::task::spawn_blocking(move || {
            let file =
                open_db_file_read_only(&path, Duration::from_secs(1)).map_err(LoadDbError::from)?;
            let file = BufReader::new(file);
            let decompressor = flate2::read::ZlibDecoder::new(file);

            let mut buffer = [0; 1024];
            let (map, _) =
                postcard::from_io((decompressor, &mut buffer)).map_err(LoadDbError::DeserComprr)?;
            Ok::<_, LoadDbError>(map)
        });
        let map = task
            .await
            .expect("de-serialization and compression should never panic")?;
        Ok(Self(Arc::new(spin::Mutex::new(map))))
    }

    /// Can only be called when nothing else holds a lock to the map anymore if
    /// something did the critical section of the lock would become too long.
    pub async fn store_to_disk(mut self) -> color_eyre::Result<()> {
        let path: Arc<Path> = setup_db_file_path()?.into();
        let path2 = Arc::clone(&path);
        let task: JoinHandle<color_eyre::Result<()>> = tokio::task::spawn_blocking(move || {
            let map = Arc::get_mut(&mut self.0)
                .ok_or_eyre("Store to disk can only run when this is the last instance of the Db")?
                .try_lock()
                .expect("Arc::get_mut, guarantees no one else has the mutex");

            let file = open_db_file_writable(&path, Duration::from_secs(1))
                .wrap_err("Could not open db files")?;
            file.set_len(0).wrap_err("Could not truncate db file")?;
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
            .note_path(path2)?;
        Ok(())
    }
}

fn setup_db_file_path() -> color_eyre::Result<PathBuf> {
    let dir = if crate::running_as_root() {
        Path::new("/var/cache").to_path_buf()
    } else {
        dirs::data_local_dir().wrap_err("Could not get user data dir")?
    }
    .join(env!("CARGO_PKG_NAME"));

    std::fs::create_dir(&dir)
        .wrap_err("Could not setup dir for database")
        .with_note(|| format!("database dir: {}", dir.display()))?;

    Ok(dir.join("db.bitcode"))
}

#[derive(Debug, thiserror::Error)]
pub enum OpenSharedError {
    #[error("Some process is holding an exclusive lock to the file")]
    TimedOut,
    #[error("Io error while opening the file")]
    IoOpening(#[source] Arc<std::io::Error>),
    #[error("Io error prevented figuring out the lock state")]
    IoLocking(#[source] Arc<std::io::Error>),
}

fn open_db_file_read_only(path: &Path, timeout: Duration) -> Result<File, OpenSharedError> {
    use std::fs;

    let now = Instant::now();
    let file = std::fs::OpenOptions::new()
        .write(false)
        .read(true)
        .open(path)
        .map_err(Arc::new)
        .map_err(OpenSharedError::IoOpening)?;

    while now.elapsed() < timeout {
        match std::fs::File::try_lock_shared(&file) {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                sleep(Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => return Err(OpenSharedError::IoLocking(Arc::new(e))),
        }
    }

    Err(OpenSharedError::TimedOut)
}

fn open_db_file_writable(path: &Path, timeout: Duration) -> Result<File, OpenSharedError> {
    use std::fs;

    let now = Instant::now();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .read(false)
        .truncate(false)
        .create(true)
        .open(path)
        .map_err(Arc::new)
        .map_err(OpenSharedError::IoOpening)?;

    while now.elapsed() < timeout {
        match std::fs::File::try_lock(&file) {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                sleep(Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => return Err(OpenSharedError::IoLocking(Arc::new(e))),
        }
    }

    Err(OpenSharedError::TimedOut)
}
