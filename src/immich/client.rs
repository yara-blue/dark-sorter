use core::fmt;
use std::convert::Infallible;
use std::error::Error;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use crate::watcher::EyreWithPath;
use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt};
use reqwest::{Method, StatusCode, Url};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio_retry::strategy::ExponentialBackoff;

/// There are way more fields but we ignore those
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    // Library ID
    pub id: LibraryId,
    // Import paths
    pub import_paths: Vec<String>,
    // Library name
    pub name: String,
    // Owner user ID
    pub owner_id: UserId,
}

/// There are way more fields but we ignore those
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    // User id,
    id: UserId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLibrary {
    // Exclusion patterns
    exclusion_patterns: Vec<String>,
    // Import paths
    import_paths: Vec<String>,
    // Library name
    name: String,
    // Owner user ID
    owner_id: UserId,
}

#[derive(Clone)]
pub struct ApiKey(pub String);

impl FromStr for ApiKey {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(ApiKey(s.to_string()))
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ApiKey")
            .field(&"<value redacted for security>")
            .finish()
    }
}

pub struct Immich {
    api_url: Url,
    api_key: ApiKey,
    client: reqwest::Client,
    pub id: UserId,
}

macro_rules! uuid_wrapper {
    ($name:ident) => {
        #[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub const ZERO: Self = Self(String::new());
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

uuid_wrapper!(UserId);
uuid_wrapper!(LibraryId);

#[derive(Debug, thiserror::Error)]
pub enum RetryError<E: Error> {
    #[error("Timed out")]
    TimedOut,
    #[error("Unrecoverable error")]
    Unrecoverable(#[source] E),
}

async fn retry<T, E: Error + CanBeRecoverable>(
    mut operation: impl AsyncFnMut() -> Result<T, E>,
) -> Result<T, RetryError<E>> {
    let backoff = ExponentialBackoff::from_millis(100).max_delay(Duration::from_mins(1));
    for i in backoff {
        match (operation)().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_recoverable() => (),
            Err(e) => return Err(RetryError::Unrecoverable(e)),
        }
        tokio::time::sleep(i).await;
    }
    Err(RetryError::TimedOut)
}

// TODO retry logic (for if immich restarts or isn't up yet etc..)

impl Immich {
    pub(super) async fn new(base_url: Url, api_key: ApiKey) -> color_eyre::Result<Self> {
        let mut client = reqwest::Client::new();
        let api_url = base_url
            .join("api/")
            .wrap_err("immich url was not well formed")?;
        let get_user = async || get_api_key_user(&mut client, &base_url, &api_key).await;
        let user = retry(get_user).await?;

        Ok(Self {
            client: reqwest::Client::new(),
            api_url,
            api_key,
            id: user.id,
        })
    }

    pub(super) async fn get_all_libraries(&self) -> color_eyre::Result<Vec<Library>> {
        let get = async || self.http_request(Method::GET, ["libraries"]).await;
        retry(get).await.wrap_err("Could not get all libraries")
    }

    async fn http_request<const N: usize, T: DeserializeOwned>(
        &self,
        method: Method,
        url: [&str; N],
    ) -> Result<T, ApiError> {
        let url = url.into_iter().fold(self.api_url.clone(), |url, e| {
            url.join(e)
                .expect("In Self::new we already check the passed in Url")
        });
        let response = self
            .client
            .request(method, url)
            .header("x-api-key", &self.api_key.0)
            .send()
            .await
            .map_err(ApiError::FailedToSend)?;

        if response.status().is_success() {
            response.json::<T>().await.map_err(ApiError::InvalidJson)
        } else {
            Err(ApiError::BadStatus(response.status()))
        }
    }

    pub(super) async fn update_library(&self, id: &LibraryId) -> color_eyre::Result<()> {
        let url = self
            .api_url
            .join("libraries/")
            .unwrap()
            .join(&format!("{id}/"))
            .unwrap()
            .join("scan")
            .unwrap();
        let response = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key.0)
            .send()
            .await
            .unwrap();

        if let Err(e) = response.error_for_status_ref() {
            if let Ok(text) = response.text().await {
                Err(e).note(text)
            } else {
                Err(e).wrap_err("Got bad status code without any body")
            }
        } else {
            Ok(())
        }
    }

    pub(super) async fn create_library(
        &self,
        exclusion_patterns: Vec<String>,
        path: impl AsRef<Path>,
        name: String,
    ) -> color_eyre::Result<Library> {
        let path = path.as_ref();
        let request = CreateLibrary {
            exclusion_patterns,
            import_paths: vec![
                path.to_str()
                    .ok_or_eyre("Path must be utf8 for immich to be able to deal with it")
                    .note_path(path)?
                    .to_string(),
            ],
            name,
            owner_id: self.id.clone(),
        };

        let response = self
            .client
            .post(self.api_url.join("libraries").unwrap())
            .header("x-api-key", &self.api_key.0)
            .json(&request)
            .send()
            .await
            .unwrap();

        if let Err(e) = response.error_for_status_ref() {
            if let Ok(text) = response.text().await {
                Err(e).note(text)
            } else {
                Err(e).wrap_err("Got bad status code without any body")
            }
        } else {
            Ok(response.json().await?)
        }
    }

    pub(crate) async fn delete_library(&self, id: &LibraryId) -> color_eyre::Result<()> {
        let url = self
            .api_url
            .join("libraries/")
            .unwrap()
            .join(&format!("{id}/"))
            .unwrap();
        let response = self
            .client
            .delete(url)
            .header("x-api-key", &self.api_key.0)
            .send()
            .await
            .unwrap();

        if let Err(e) = response.error_for_status_ref() {
            if let Ok(text) = response.text().await {
                Err(e).note(text)
            } else {
                Err(e).wrap_err("Got bad status code without any body")
            }
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Could not send request")]
    FailedToSend(#[source] reqwest::Error),
    #[error("Bad status code: {0}")]
    BadStatus(StatusCode),
    #[error("Could not decode response body as json")]
    InvalidJson(#[source] reqwest::Error),
}

trait CanBeRecoverable {
    fn is_recoverable(&self) -> bool {
        false
    }
}

impl CanBeRecoverable for ApiError {
    fn is_recoverable(&self) -> bool {
        match self {
            ApiError::FailedToSend(_) => true,
            ApiError::BadStatus(
                // can always add more
                StatusCode::BAD_REQUEST
                | StatusCode::FORBIDDEN
                | StatusCode::NOT_FOUND
                | StatusCode::NOT_IMPLEMENTED
                | StatusCode::METHOD_NOT_ALLOWED
                | StatusCode::NOT_IMPLEMENTED,
            ) => false,
            ApiError::BadStatus(_) => true,
            ApiError::InvalidJson(_) => false,
        }
    }
}

async fn get_api_key_user(
    client: &mut reqwest::Client,
    base_url: &Url,
    api_key: &ApiKey,
) -> Result<User, ApiError> {
    let response = client
        .get(base_url.join("users/me").unwrap())
        .header("x-api-key", &api_key.0)
        .send()
        .await
        .map_err(ApiError::FailedToSend)?;

    if response.status().is_success() {
        response.json::<User>().await.map_err(ApiError::InvalidJson)
    } else {
        Err(ApiError::BadStatus(response.status()))
    }
}
