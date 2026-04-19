use uncloud_common::{UpdatePreferencesRequest, UserResponse};

use super::api;

pub async fn update_preferences(req: UpdatePreferencesRequest) -> Result<UserResponse, String> {
    let response = api::put_v1("/auth/me/preferences")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<UserResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update preferences".to_string())
    }
}
