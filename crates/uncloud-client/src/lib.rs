use std::path::Path;
use std::sync::Arc;

use reqwest::cookie::Jar;
use serde_json::json;
use tracing::instrument;
use uncloud_common::{
    CreateFolderRequest, EffectiveStrategyResponse, FileResponse, FolderResponse,
    SyncTreeResponse, UpdateFolderRequest, UserResponse,
};

mod error;
pub use error::{ClientError, Result};

/// Native async HTTP client for the Uncloud server API.
///
/// Uses a persistent cookie jar so that the session cookie returned by `login`
/// is automatically sent on subsequent requests.
pub struct Client {
    base_url: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        let jar = Arc::new(Jar::default());
        let http = reqwest::Client::builder()
            .cookie_provider(jar)
            .build()
            .expect("failed to build reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            http,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // ── Auth ──────────────────────────────────────────────────────────────────

    #[instrument(skip(self, password))]
    pub async fn login(&self, username: &str, password: &str) -> Result<UserResponse> {
        let body = json!({ "username": username, "password": password });
        let resp = self
            .http
            .post(self.url("/api/auth/login"))
            .json(&body)
            .send()
            .await?;
        self.parse(resp).await
    }

    pub async fn logout(&self) -> Result<()> {
        let resp = self
            .http
            .post(self.url("/api/auth/logout"))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::api(resp).await)
        }
    }

    pub async fn me(&self) -> Result<UserResponse> {
        let resp = self.http.get(self.url("/api/auth/me")).send().await?;
        self.parse(resp).await
    }

    // ── Files ─────────────────────────────────────────────────────────────────

    pub async fn list_files(&self, parent_id: Option<&str>) -> Result<Vec<FileResponse>> {
        let url = match parent_id {
            Some(id) => format!("{}/api/files?parent_id={}", self.base_url, id),
            None => self.url("/api/files"),
        };
        let resp = self.http.get(&url).send().await?;
        self.parse(resp).await
    }

    /// Download a file by ID to `dest` on the local filesystem.
    pub async fn download_file(&self, id: &str, dest: &Path) -> Result<()> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        let resp = self
            .http
            .get(self.url(&format!("/api/files/{}/download", id)))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(ClientError::api(resp).await);
        }

        let mut file = tokio::fs::File::create(dest).await.map_err(|e| {
            ClientError::Io(format!("cannot create {}: {}", dest.display(), e))
        })?;

        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            file.write_all(&bytes).await.map_err(|e| {
                ClientError::Io(format!("write error: {}", e))
            })?;
        }
        Ok(())
    }

    /// Upload a local file to the server.
    pub async fn upload_file(
        &self,
        path: &Path,
        parent_id: Option<&str>,
    ) -> Result<FileResponse> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();

        let bytes = tokio::fs::read(path).await.map_err(|e| {
            ClientError::Io(format!("cannot read {}: {}", path.display(), e))
        })?;

        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(pid) = parent_id {
            form = form.text("parent_id", pid.to_owned());
        }

        let resp = self
            .http
            .post(self.url("/api/uploads/simple"))
            .multipart(form)
            .send()
            .await?;
        self.parse(resp).await
    }

    /// Replace the content of an existing file, archiving the previous version server-side.
    pub async fn update_file_content(&self, file_id: &str, path: &Path) -> Result<FileResponse> {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            ClientError::Io(format!("cannot read {}: {}", path.display(), e))
        })?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();
        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self
            .http
            .post(self.url(&format!("/api/files/{}/content", file_id)))
            .multipart(form)
            .send()
            .await?;
        self.parse(resp).await
    }

    /// Download a file by ID and return its bytes. Used by `uncloud-sync` so
    /// the engine can write through a `LocalFs` backend instead of `std::fs`.
    pub async fn download_file_bytes(&self, id: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(self.url(&format!("/api/files/{}/download", id)))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ClientError::api(resp).await);
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Upload raw bytes as a new file. Parallel to [`upload_file`] but sourced
    /// from memory rather than disk.
    pub async fn upload_bytes(
        &self,
        file_name: &str,
        bytes: Vec<u8>,
        parent_id: Option<&str>,
    ) -> Result<FileResponse> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_owned());
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(pid) = parent_id {
            form = form.text("parent_id", pid.to_owned());
        }
        let resp = self
            .http
            .post(self.url("/api/uploads/simple"))
            .multipart(form)
            .send()
            .await?;
        self.parse(resp).await
    }

    /// Replace existing file content with raw bytes.
    pub async fn update_file_content_bytes(
        &self,
        file_id: &str,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<FileResponse> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_owned());
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self
            .http
            .post(self.url(&format!("/api/files/{}/content", file_id)))
            .multipart(form)
            .send()
            .await?;
        self.parse(resp).await
    }

    pub async fn delete_file(&self, id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/api/files/{}", id)))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::api(resp).await)
        }
    }

    // ── Folders ───────────────────────────────────────────────────────────────

    pub async fn list_folders(&self, parent_id: Option<&str>) -> Result<Vec<FolderResponse>> {
        let url = match parent_id {
            Some(id) => format!("{}/api/folders?parent_id={}", self.base_url, id),
            None => self.url("/api/folders"),
        };
        let resp = self.http.get(&url).send().await?;
        self.parse(resp).await
    }

    pub async fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
    ) -> Result<FolderResponse> {
        let body = CreateFolderRequest {
            name: name.to_owned(),
            parent_id: parent_id.map(|s| s.to_owned()),
        };
        let resp = self
            .http
            .post(self.url("/api/folders"))
            .json(&body)
            .send()
            .await?;
        self.parse(resp).await
    }

    pub async fn delete_folder(&self, id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/api/folders/{}", id)))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::api(resp).await)
        }
    }

    pub async fn get_effective_strategy(
        &self,
        folder_id: &str,
    ) -> Result<EffectiveStrategyResponse> {
        let resp = self
            .http
            .get(self.url(&format!(
                "/api/folders/{}/effective-strategy",
                folder_id
            )))
            .send()
            .await?;
        self.parse(resp).await
    }

    /// Fetch the breadcrumb (root → leaf) for a folder.
    pub async fn get_folder_breadcrumb(
        &self,
        folder_id: &str,
    ) -> Result<Vec<FolderResponse>> {
        let resp = self
            .http
            .get(self.url(&format!("/api/folders/{}/breadcrumb", folder_id)))
            .send()
            .await?;
        self.parse(resp).await
    }

    pub async fn update_folder(
        &self,
        id: &str,
        req: &UpdateFolderRequest,
    ) -> Result<FolderResponse> {
        let resp = self
            .http
            .put(self.url(&format!("/api/folders/{}", id)))
            .json(req)
            .send()
            .await?;
        self.parse(resp).await
    }

    // ── Sync ──────────────────────────────────────────────────────────────────

    pub async fn sync_tree(&self, parent_id: Option<&str>) -> Result<SyncTreeResponse> {
        let url = match parent_id {
            Some(id) => format!("{}/api/sync/tree?parent_id={}", self.base_url, id),
            None => self.url("/api/sync/tree"),
        };
        let resp = self.http.get(&url).send().await?;
        self.parse(resp).await
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    async fn parse<T: serde::de::DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T> {
        if resp.status().is_success() {
            resp.json::<T>().await.map_err(ClientError::from)
        } else {
            Err(ClientError::api(resp).await)
        }
    }
}
