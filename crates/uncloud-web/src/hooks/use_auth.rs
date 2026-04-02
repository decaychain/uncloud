use uncloud_common::{LoginRequest, RegisterRequest, UserResponse};

use super::api;

pub async fn login(username: &str, password: &str) -> Result<UserResponse, String> {
    let req = LoginRequest {
        username: username.to_string(),
        password: password.to_string(),
    };

    let response = api::post("/auth/login")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        let user: UserResponse = response
            .json::<UserResponse>()
            .await
            .map_err(|e| e.to_string())?;

        if let Some(token) = &user.session_token {
            api::seed_auth_token(token.clone());
        }

        Ok(user)
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn register(username: &str, email: &str, password: &str) -> Result<UserResponse, String> {
    let req = RegisterRequest {
        username: username.to_string(),
        email: email.to_string(),
        password: password.to_string(),
    };

    let response = api::post("/auth/register")
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
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn logout() -> Result<(), String> {
    let response = api::post("/auth/logout")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    api::clear_auth_token();

    if response.ok() {
        Ok(())
    } else {
        Err("Failed to logout".to_string())
    }
}

pub async fn me() -> Result<UserResponse, String> {
    let response = api::get("/auth/me")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<UserResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Not authenticated".to_string())
    }
}

fn extract_error(text: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
            return error.to_string();
        }
    }
    text.to_string()
}
