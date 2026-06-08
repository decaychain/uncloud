use uncloud_common::{
    CreateSubsonicCredentialRequest, CreateSubsonicCredentialResponse, SubsonicCredentialResponse,
};

use super::api;

pub async fn list_credentials() -> Result<Vec<SubsonicCredentialResponse>, String> {
    let response = api::get("/subsonic/credentials")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<SubsonicCredentialResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to list Subsonic app passwords".to_string())
    }
}

pub async fn create_credential(label: &str) -> Result<CreateSubsonicCredentialResponse, String> {
    let request = CreateSubsonicCredentialRequest {
        label: label.to_string(),
    };
    let response = api::post("/subsonic/credentials")
        .json(&request)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<CreateSubsonicCredentialResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create Subsonic app password".to_string())
    }
}

pub async fn delete_credential(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/subsonic/credentials/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to revoke Subsonic app password".to_string())
    }
}
