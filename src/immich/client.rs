use core::fmt;
use std::convert::Infallible;
use std::path::Path;
use std::str::FromStr;

use crate::watcher::EyreWithPath;
use color_eyre::Section;
use color_eyre::eyre::{Context, OptionExt};
use reqwest::Url;

use serde::{Deserialize, Serialize};

/// There are way more fields but we ignore those
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    // Library ID
    pub id: Uuid,
    // Import paths
    pub import_paths: Vec<String>,
    // Library name
    pub name: String,
    // Owner user ID
    pub owner_id: Uuid,
}

/// There are way more fields but we ignore those
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    // User id,
    id: Uuid,
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
    owner_id: Uuid,
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
    base_url: Url,
    api_key: ApiKey,
    client: reqwest::Client,
    pub id: Uuid,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct Uuid(String);

impl Uuid {
    pub const ZERO: Self = Self(String::new());
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Immich {
    pub(super) async fn new(base_url: Url, api_key: ApiKey) -> color_eyre::Result<Self> {
        let mut client = reqwest::Client::new();
        let api_url = base_url.join("api/").unwrap();
        let user = get_api_key_user(&mut client, &base_url, &api_key).await?;
        Ok(Self {
            client: reqwest::Client::new(),
            base_url: api_url.join("api/").unwrap(),
            api_key,
            id: user.id,
        })
    }

    pub(super) async fn get_all_libraries(&self) -> color_eyre::Result<Vec<Library>> {
        let response = self
            .client
            .get(self.base_url.join("libraries").unwrap())
            .header("x-api-key", &self.api_key.0)
            .send()
            .await
            .unwrap();

        if let Err(e) = response.error_for_status_ref() {
            return if let Ok(text) = response.text().await {
                Err(e).note(text)
            } else {
                Err(e).wrap_err("Got bad status code without any body")
            };
        }

        response
            .json::<Vec<Library>>()
            .await
            .wrap_err("Could not get library body")
    }

    pub(super) async fn update_library(&self, id: &Uuid) -> color_eyre::Result<()> {
        let url = self
            .base_url
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
            .post(self.base_url.join("libraries").unwrap())
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

    pub(crate) async fn delete_library(&self, id: &Uuid) -> color_eyre::Result<()> {
        let url = self
            .base_url
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

async fn get_api_key_user(
    client: &mut reqwest::Client,
    base_url: &Url,
    api_key: &ApiKey,
) -> color_eyre::Result<User> {
    let response = client
        .get(base_url.join("users/me").unwrap())
        .header("x-api-key", &api_key.0)
        .send()
        .await
        .unwrap();

    if let Err(e) = response.error_for_status_ref() {
        return if let Ok(text) = response.text().await {
            Err(e).note(text)
        } else {
            Err(e).wrap_err("Got bad status code without any body")
        };
    }

    response
        .json::<User>()
        .await
        .wrap_err("Could not get library body")
}
