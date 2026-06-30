use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::num::ParseIntError;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use color_eyre::eyre::Context;
use tokio::sync::Notify;

use crate::SourceDir;
use crate::fs::{PreviewFile, RawFile, ThrottledFs, XmpFile};
use crate::watcher::EyreWithPath;

/// TODO do similar thing for file metadata
#[derive(Default)]
pub(crate) struct ParsedXmps(spin::Mutex<HashMap<XmpFile, XmpState>>);

#[derive(Debug, Clone)]
pub(crate) enum XmpState {
    Loading(Arc<Notify>),
    Loaded(Xmp),
    Error(ReadParseError),
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum ReadParseError {
    #[error("File does not exist, path: {}", .0.display())]
    NotFound(Arc<Path>),
    #[error("Could not read xmp file, path: {}", .1.display())]
    Io(#[source] Arc<std::io::Error>, Arc<Path>),
    #[error("Could not parse xmp file")]
    Parse(#[source] ParseError),
}

impl ReadParseError {
    pub(crate) fn from_io(e: tokio::io::Error, path: impl AsRef<Path>) -> Self {
        if let ErrorKind::NotFound = e.kind() {
            Self::NotFound(path.as_ref().to_path_buf().into())
        } else {
            Self::Io(Arc::new(e), path.as_ref().to_path_buf().into())
        }
    }
}

impl ParsedXmps {
    pub(crate) async fn cached_or_parse(
        &self,
        path: &XmpFile,
        fs: &ThrottledFs,
    ) -> Result<Xmp, ReadParseError> {
        use std::collections::hash_map::Entry;
        let notify = Arc::new(Notify::new());
        let loading = XmpState::Loading(Arc::clone(&notify));
        let state = match self.0.lock().entry(path.clone()) {
            Entry::Occupied(entry) => Some(entry.get().clone()),
            Entry::Vacant(slot) => {
                slot.insert(loading);
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
                let xmp = fs
                    .read_to_string(path)
                    .await
                    .map_err(|e| ReadParseError::from_io(e, path))?;
                let res = Xmp::from_str(&xmp).map_err(ReadParseError::Parse);
                let new_state = match res.clone() {
                    Ok(xmp) => XmpState::Loaded(xmp),
                    Err(e) => XmpState::Error(e),
                };
                *self.0.lock().get_mut(path).expect("we inserted Loading") = new_state;
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
    pub fn preview_file(&self, source: &SourceDir) -> PreviewFile {
        PreviewFile(self.raw_file(source).0.with_extension("jpg"))
    }

    pub(crate) fn raw_file(&self, source: &SourceDir) -> RawFile {
        RawFile(source.0.0.join(&*self.raw))
    }

    pub async fn preview_missing(&self, source: &SourceDir) -> color_eyre::Result<bool> {
        let preview_path = self.preview_file(source);
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
pub enum ParseError {
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
}

impl FromStr for Xmp {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rating = Rating::from_str(s)?;
        let edits = parse_edits(s);
        let raw = parse_raw(s)?;

        Ok(dbg!(Self { rating, edits, raw }))
    }
}

pub(crate) fn parse_raw(s: &str) -> Result<Arc<str>, ParseError> {
    let start_pattern = r#"xmpMM:DerivedFrom=""#;
    let file_name_start =
        s.find(start_pattern).ok_or(ParseError::NoRawListed)? + start_pattern.len();
    let file_name_end = s[file_name_start..]
        .find('"')
        .ok_or(ParseError::NoFieldEnd)?;
    let file_name = &s[file_name_start..file_name_start + file_name_end];
    if file_name.contains('.') {
        Ok(file_name.to_string().into())
    } else {
        Err(ParseError::RawWithoutExtension)
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

impl FromStr for Rating {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let start_pattern = "xmp:Rating=\"";
        let rating_start =
            s.find(start_pattern).ok_or(ParseError::NoRatingStart)? + start_pattern.len();
        let rating_end = s[rating_start..].find('"').ok_or(ParseError::NoFieldEnd)?;
        let rating = s[rating_start..rating_start + rating_end]
            .parse()
            .map_err(ParseError::RatingNotNumber)?;
        Ok(match rating {
            -1 => Rating::Rejected,
            0 => Rating::Unrated,
            1 => Rating::One,
            2 => Rating::Two,
            3 => Rating::Three,
            4 => Rating::Four,
            5 => Rating::Five,
            _ => return Err(ParseError::RatingOutOfRange),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_parse() {
        Xmp::from_str(include_str!("../tests/assets/unrated.NEF.xmp")).unwrap();
        Xmp::from_str(include_str!("../tests/assets/rated.NEF.xmp")).unwrap();
    }
}
