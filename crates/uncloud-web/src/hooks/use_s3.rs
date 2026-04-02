use serde::{Deserialize, Serialize};

use super::api;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3CredentialResponse {
    pub id: String,
    pub access_key_id: String,
    pub label: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateS3CredentialResponse {
    pub id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub label: String,
    pub created_at: String,
}

pub async fn list_credentials() -> Result<Vec<S3CredentialResponse>, String> {
    let response = api::get("/v1/s3/credentials")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<S3CredentialResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to list S3 credentials".to_string())
    }
}

pub async fn create_credential(label: &str) -> Result<CreateS3CredentialResponse, String> {
    let body = serde_json::json!({ "label": label });

    let response = api::post("/v1/s3/credentials")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<CreateS3CredentialResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create S3 credential".to_string())
    }
}

pub async fn delete_credential(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/v1/s3/credentials/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete S3 credential".to_string())
    }
}
