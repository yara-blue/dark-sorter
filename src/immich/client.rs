use core::fmt;
use std::convert::Infallible;
use std::error::Error;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use crate::watcher::EyreWithPath;
use color_eyre::eyre::{Context, OptionExt};
use reqwest::{Method, StatusCode, Url};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio_retry::strategy::ExponentialBackoff;
use tracing::{debug, instrument, warn};

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
struct User {
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

#[derive(Clone)]
pub struct Immich {
    api_url: Url,
    api_key: ApiKey,
    client: reqwest::Client,
    pub id: UserId,
}

impl fmt::Debug for Immich {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Immich")
            .field("api_url", &self.api_url.as_str())
            .field("id", &self.id)
            .finish()
    }
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
    let mut not_warned = true;
    let backoff = ExponentialBackoff::from_millis(100)
        .max_delay(Duration::from_millis(500))
        .take(60)
        .chain(ExponentialBackoff::from_millis(500).max_delay(Duration::from_mins(1)));
    for period in backoff {
        let recoverable_err = match (operation)().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_recoverable() => e,
            Err(e) => return Err(RetryError::Unrecoverable(e)),
        };
        tokio::time::sleep(period).await;
        if period > Duration::from_secs(5) {
            if not_warned {
                warn!("Retrying (err: {recoverable_err:?})");
                not_warned = false;
            } else {
                debug!("Retrying (err: {recoverable_err:?})");
            }
        }
    }
    Err(RetryError::TimedOut)
}

impl Immich {
    #[instrument(skip_all, fields(url = %base_url))]
    pub(super) async fn new(base_url: Url, api_key: ApiKey) -> color_eyre::Result<Self> {
        let api_url = base_url
            .join("api/")
            .wrap_err("immich url was not well formed")?;

        let mut this = Self {
            client: reqwest::Client::new(),
            api_url,
            api_key,
            id: UserId::ZERO,
        };

        let get_current_user = async || this.http_request(Method::GET, "users/me").await;
        let user: User = retry(get_current_user)
            .await
            .wrap_err("Could not get user corresponding to this API key")?;
        this.id = user.id;

        Ok(this)
    }

    #[instrument(skip(self))]
    pub(super) async fn get_all_libraries(&self) -> color_eyre::Result<Vec<Library>> {
        let get_all_libraries = async || self.http_request(Method::GET, "libraries").await;
        retry(get_all_libraries)
            .await
            .wrap_err("Could not get all libraries")
    }

    #[instrument(skip(self))]
    pub(super) async fn update_library(&self, id: &LibraryId) -> color_eyre::Result<()> {
        debug!("triggering immich sync for library");
        let update_library = async || {
            self.http_request(Method::POST, &format!("libraries/{id}/scan"))
                .await
        };
        retry(update_library)
            .await
            .wrap_err("Could not update library")
    }

    #[instrument(skip_all, fields(name = name, path = ?path.as_ref().display()))]
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
        let create_library = async || {
            self.http_request_with_body(Method::POST, "libraries", &request)
                .await
        };
        retry(create_library)
            .await
            .wrap_err("Could not create library")
    }

    #[instrument(skip(self))]
    pub(crate) async fn delete_library(&self, id: &LibraryId) -> color_eyre::Result<()> {
        debug!("deleting immich library");
        let delete_library = async || {
            self.http_request(Method::DELETE, &format!("libraries/{id}"))
                .await
        };
        retry(delete_library)
            .await
            .wrap_err("Could not delete library")
    }

    #[instrument(skip_all, fields(method=%method, url))]
    async fn http_request<T: IsUnitOrDeserialize>(
        &self,
        method: Method,
        url: &str,
    ) -> Result<T, ApiError> {
        self.http_request_inner(method, url, None::<&()>).await
    }

    #[instrument(skip_all, fields(method=%method, url))]
    async fn http_request_with_body<B: Serialize, T: IsUnitOrDeserialize>(
        &self,
        method: Method,
        url: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.http_request_inner(method, url, Some(body)).await
    }

    #[instrument(skip_all, fields(method=%method, url))]
    async fn http_request_inner<B: Serialize, RB: IsUnitOrDeserialize>(
        &self,
        method: Method,
        url: &str,
        body: Option<&B>,
    ) -> Result<RB, ApiError> {
        let url = self
            .api_url
            .join(url)
            .expect("In Self::new we already check the passed in Url");
        tracing::Span::current().record("url", url.as_str());
        let request = self
            .client
            .request(method, url)
            .header("x-api-key", &self.api_key.0.clone());
        let request = if let Some(body) = body {
            request.json(body)
        } else {
            request
        };
        let response = request.send().await.map_err(ApiError::FailedToSend)?;

        if response.status().is_success() {
            if let Some(body) = RB::is_unit() {
                Ok(body)
            } else {
                let response = response.text().await.map_err(ApiError::FailedToReceive)?;
                serde_json::from_str(&response)
                    .map_err(|error| ApiError::InvalidJson { error, response })
            }
        } else {
            Err(ApiError::BadStatus(response.status()))
        }
    }
}

trait IsUnitOrDeserialize: DeserializeOwned {
    fn is_unit() -> Option<Self> {
        None
    }
}

impl IsUnitOrDeserialize for Vec<Library> {}
impl IsUnitOrDeserialize for Library {}
impl IsUnitOrDeserialize for User {}
impl IsUnitOrDeserialize for () {
    fn is_unit() -> Option<Self> {
        Some(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Could not send request")]
    FailedToSend(#[source] reqwest::Error),
    #[error("Bad status code: {0}")]
    BadStatus(StatusCode),
    #[error("Could not decode response body as json, response text is: {response}")]
    InvalidJson {
        #[source]
        error: serde_json::Error,
        response: String,
    },
    #[error("Could not receive response")]
    FailedToReceive(#[source] reqwest::Error),
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
            ApiError::FailedToReceive(_) => true,
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
            ApiError::InvalidJson { .. } => false,
        }
    }
}
