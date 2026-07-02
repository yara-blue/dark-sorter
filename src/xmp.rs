use std::collections::HashMap;
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::num::ParseIntError;
use std::path::Path;
use std::sync::Arc;

use color_eyre::eyre::Context;
use tokio::sync::Notify;
use tracing::debug;

use crate::fs::{PreviewFile, RawFile, SourceDir, TargetDir, ThrottledFs, XmpFile};
use crate::watcher::EyreWithPath;

#[derive(Default, Clone)]
pub(crate) struct ParsedXmps(Arc<spin::Mutex<HashMap<XmpFile, XmpState>>>);

impl fmt::Debug for ParsedXmps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items = self.0.lock();
        let items: Vec<_> = items
            .iter()
            .map(|(path, state)| {
                let state = match state {
                    XmpState::Loading(_) => "Loading".to_string(),
                    XmpState::Loaded(xmp) => format!("{xmp:?}"),
                    XmpState::Error(_) => "Err".to_string(),
                };
                format!("{}: {state}", path.display())
            })
            .collect();
        f.debug_tuple("ParsedXmps").field(&items).finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum XmpState {
    Loading(Arc<Notify>),
    Loaded(Xmp),
    Error(XmpError),
}

impl ParsedXmps {
    #[tracing::instrument(skip(self, fs))]
    pub(crate) async fn get_cached_or_read_from_file(
        &self,
        path: &XmpFile,
        fs: &ThrottledFs,
    ) -> Result<Xmp, XmpError> {
        use std::collections::hash_map::Entry;
        let notify = Arc::new(Notify::new());
        let state = match self.0.lock().entry(path.clone()) {
            Entry::Occupied(entry) => Some(entry.get().clone()),
            Entry::Vacant(slot) => {
                slot.insert(XmpState::Loading(Arc::clone(&notify)));
                None
            }
        };
        match state {
            Some(XmpState::Loaded(xmp)) => Ok(xmp),
            Some(XmpState::Error(e)) => Err(e),
            Some(XmpState::Loading(notify)) => {
                notify.notified().await;
                match self.0.lock().get(path).cloned() {
                    Some(XmpState::Error(e)) => Err(e),
                    Some(XmpState::Loaded(xmp)) => Ok(xmp),
                    Some(XmpState::Loading(_)) | None => unreachable!("we where notified"),
                }
            }
            None => {
                debug!("reading in xmp");
                let res = Xmp::read_from_file(path, fs).await;
                let new_state = match res.clone() {
                    Ok(xmp) => XmpState::Loaded(xmp),
                    Err(e) => XmpState::Error(e),
                };
                *self.0.lock().get_mut(path).expect("we inserted Loading") = new_state;
                debug!("done reading in, update xmp: {self:?}");
                notify.notify_waiters();
                res
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditHash(u64);

impl EditHash {
    pub const NO_EDITS: Self = Self(0);
}

#[derive(Debug, Clone)]
pub struct Xmp {
    pub(crate) rating: Rating,
    /// if the edits changed we need to re-export
    pub(crate) edits: Option<EditHash>,
    pub(crate) raw: Arc<str>,
}

impl Xmp {
    pub async fn read_from_file(path: &XmpFile, fs: &ThrottledFs) -> Result<Self, XmpError> {
        let s = fs
            .read_to_string(path)
            .await
            .map_err(|e| XmpError::from_io(e, path))?;

        let rating = Rating::from_str(&s)?;
        let edits = parse_edits(&s);
        let raw = parse_raw(&s)?;

        let xmp_file_name = path.file_stem().to_str().ok_or(XmpError::FileNameNotUtf8)?;

        if *raw != *xmp_file_name {
            return Err(XmpError::RawNameMismatches {
                raw: Arc::clone(&raw),
                xmp_file_name: xmp_file_name.to_string(),
            });
        }

        Ok(Self { rating, edits, raw })
    }

    pub fn preview_file(&self, target: impl AsRef<TargetDir>) -> PreviewFile {
        PreviewFile(target.as_ref().0.0.join(&*self.raw).with_extension("jpg"))
    }

    pub(crate) fn raw_file(&self, source: impl AsRef<SourceDir>) -> RawFile {
        RawFile(source.as_ref().0.0.join(&*self.raw))
    }

    pub async fn preview_missing(&self, target: impl AsRef<TargetDir>) -> color_eyre::Result<bool> {
        let preview_path = self.preview_file(target.as_ref());
        let preview_exists = tokio::fs::try_exists(&preview_path)
            .await
            .wrap_err("Could not check if jpeg exists")
            .note_path(preview_path)?;
        Ok(!preview_exists)
    }

    pub fn rated(&self) -> bool {
        match self.rating {
            Rating::Rejected | Rating::Unrated => false,
            Rating::One | Rating::Two | Rating::Three | Rating::Four | Rating::Five => true,
        }
    }
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum XmpError {
    #[error("File does not exist, path: {}", .0.display())]
    NotFound(Arc<Path>),
    #[error("Could not read xmp file, path: {}", .1.display())]
    Io(#[source] Arc<std::io::Error>, Arc<Path>),
    #[error("There was no rating field")]
    NoRatingStart,
    #[error("The rating field did not end")]
    NoFieldEnd,
    #[error("The rating was not a number")]
    RatingNotNumber(#[source] ParseIntError),
    #[error("The XMP spec requires a rating to be between -1 and (including) 5")]
    RatingOutOfRange,
    #[error("Xmp misses the field that lists the raw it describes")]
    NoRawListed,
    #[error("The file name listed in the Xmp has no extension")]
    RawWithoutExtension,
    /// Else the name of the identically named raw file cannot be stored in the xmp
    #[error("Xmp file name must be valid utf8")]
    FileNameNotUtf8,
    #[error(
        "The raw file listed in the xmp must have the same name \
        as the xmp file without it's extension. \
        raw file listed: {raw}, \
        xmp file name without extension: {xmp_file_name}"
    )]
    RawNameMismatches {
        raw: Arc<str>,
        xmp_file_name: String,
    },
}

impl XmpError {
    pub(crate) fn from_io(e: tokio::io::Error, path: impl AsRef<Path>) -> Self {
        if let ErrorKind::NotFound = e.kind() {
            Self::NotFound(path.as_ref().to_path_buf().into())
        } else {
            Self::Io(Arc::new(e), path.as_ref().to_path_buf().into())
        }
    }
}

pub(crate) fn parse_raw(s: &str) -> Result<Arc<str>, XmpError> {
    let start_pattern = r#"xmpMM:DerivedFrom=""#;
    let file_name_start = s.find(start_pattern).ok_or(XmpError::NoRawListed)? + start_pattern.len();
    let file_name_end = s[file_name_start..].find('"').ok_or(XmpError::NoFieldEnd)?;
    let file_name = &s[file_name_start..file_name_start + file_name_end];
    if file_name.contains('.') {
        Ok(file_name.to_string().into())
    } else {
        Err(XmpError::RawWithoutExtension)
    }
}

pub(crate) fn parse_edits(s: &str) -> Option<EditHash> {
    let start_pattern = r"<darktable:history>";
    let end_pattern = r"</darktable:history>";

    let start = s.find(start_pattern)? + start_pattern.len();
    let end = s[start..].find(end_pattern)?;
    let edits = &s[start..end];

    let mut hasher = DefaultHasher::new();
    edits.hash(&mut hasher);
    Some(EditHash(hasher.finish()))
}

#[derive(Debug, Clone)]
pub enum Rating {
    Rejected,
    Unrated,
    One,
    Two,
    Three,
    Four,
    Five,
}

impl Rating {
    fn from_str(s: &str) -> Result<Self, XmpError> {
        let start_pattern = "xmp:Rating=\"";
        let rating_start =
            s.find(start_pattern).ok_or(XmpError::NoRatingStart)? + start_pattern.len();
        let rating_end = s[rating_start..].find('"').ok_or(XmpError::NoFieldEnd)?;
        let rating = s[rating_start..rating_start + rating_end]
            .parse()
            .map_err(XmpError::RatingNotNumber)?;
        Ok(match rating {
            -1 => Rating::Rejected,
            0 => Rating::Unrated,
            1 => Rating::One,
            2 => Rating::Two,
            3 => Rating::Three,
            4 => Rating::Four,
            5 => Rating::Five,
            _ => return Err(XmpError::RatingOutOfRange),
        })
    }
}
