use crate::api::*;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Server error: {0}")]
    Server(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Not found")]
    NotFound,

    #[error("Validation error: {0}")]
    Validation(String),
}

pub struct ApiClient {
    base_url: String,
    #[cfg(not(target_arch = "wasm32"))]
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            #[cfg(not(target_arch = "wasm32"))]
            client: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, ApiError> {
        let status = response.status();

        if status.is_success() {
            response
                .json()
                .await
                .map_err(|e| ApiError::Network(e.to_string()))
        } else if status == reqwest::StatusCode::UNAUTHORIZED {
            Err(ApiError::Unauthorized)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Err(ApiError::NotFound)
        } else {
            let text = response.text().await.unwrap_or_default();
            Err(ApiError::Server(text))
        }
    }

    // Auth endpoints
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn register(&self, req: RegisterRequest) -> Result<UserResponse, ApiError> {
        let response = self
            .client
            .post(self.url("/api/auth/register"))
            .json(&req)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn login(&self, req: LoginRequest) -> Result<UserResponse, ApiError> {
        let response = self
            .client
            .post(self.url("/api/auth/login"))
            .json(&req)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn logout(&self) -> Result<(), ApiError> {
        self.client
            .post(self.url("/api/auth/logout"))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn me(&self) -> Result<UserResponse, ApiError> {
        let response = self
            .client
            .get(self.url("/api/auth/me"))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    // Files endpoints
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn list_files(&self, parent_id: Option<&str>) -> Result<Vec<FileResponse>, ApiError> {
        let mut url = self.url("/api/files");
        if let Some(id) = parent_id {
            url = format!("{}?parent_id={}", url, id);
        }
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn delete_file(&self, id: &str) -> Result<(), ApiError> {
        let response = self
            .client
            .delete(self.url(&format!("/api/files/{}", id)))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Server(response.text().await.unwrap_or_default()))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_url(&self, id: &str) -> String {
        self.url(&format!("/api/files/{}/download", id))
    }

    // Folders endpoints
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn list_folders(
        &self,
        parent_id: Option<&str>,
    ) -> Result<Vec<FolderResponse>, ApiError> {
        let mut url = self.url("/api/folders");
        if let Some(id) = parent_id {
            url = format!("{}?parent_id={}", url, id);
        }
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn create_folder(&self, req: CreateFolderRequest) -> Result<FolderResponse, ApiError> {
        let response = self
            .client
            .post(self.url("/api/folders"))
            .json(&req)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn delete_folder(&self, id: &str) -> Result<(), ApiError> {
        let response = self
            .client
            .delete(self.url(&format!("/api/folders/{}", id)))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Server(response.text().await.unwrap_or_default()))
        }
    }

    // Shares endpoints
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn list_shares(&self) -> Result<Vec<ShareResponse>, ApiError> {
        let response = self
            .client
            .get(self.url("/api/shares"))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn create_share(&self, req: CreateShareRequest) -> Result<ShareResponse, ApiError> {
        let response = self
            .client
            .post(self.url("/api/shares"))
            .json(&req)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn delete_share(&self, id: &str) -> Result<(), ApiError> {
        let response = self
            .client
            .delete(self.url(&format!("/api/shares/{}", id)))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Server(response.text().await.unwrap_or_default()))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn get_public_share(&self, token: &str) -> Result<PublicShareResponse, ApiError> {
        let response = self
            .client
            .get(self.url(&format!("/api/public/{}", token)))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        self.handle_response(response).await
    }

    pub fn public_download_url(&self, token: &str) -> String {
        self.url(&format!("/api/public/{}/download", token))
    }
}
