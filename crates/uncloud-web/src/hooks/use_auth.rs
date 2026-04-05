use uncloud_common::{
    CreateInviteRequest, InviteInfoResponse, InviteResponse, LoginRequest, LoginResponse,
    RegisterRequest, ServerInfoResponse, TotpDisableRequest, TotpEnableRequest, TotpSetupResponse,
    TotpVerifyRequest, UserResponse,
};

use super::api;

/// Fetch server capabilities (registration mode, version).
pub async fn server_info() -> Result<ServerInfoResponse, String> {
    let response = api::get("/auth/server-info")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ServerInfoResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to fetch server info".to_string())
    }
}

pub async fn login(username: &str, password: &str) -> Result<LoginResponse, String> {
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
        let login_resp: LoginResponse = response
            .json::<LoginResponse>()
            .await
            .map_err(|e| e.to_string())?;

        if let Some(ref user) = login_resp.user {
            if let Some(token) = &user.session_token {
                api::seed_auth_token(token.clone());
            }
        }

        Ok(login_resp)
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn totp_verify(totp_token: &str, code: &str) -> Result<LoginResponse, String> {
    let req = TotpVerifyRequest {
        totp_token: totp_token.to_string(),
        code: code.to_string(),
    };

    let response = api::post("/auth/totp/verify")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        let login_resp: LoginResponse = response
            .json::<LoginResponse>()
            .await
            .map_err(|e| e.to_string())?;

        if let Some(ref user) = login_resp.user {
            if let Some(token) = &user.session_token {
                api::seed_auth_token(token.clone());
            }
        }

        Ok(login_resp)
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn demo_login() -> Result<LoginResponse, String> {
    let response = api::post("/auth/demo")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        let login_resp: LoginResponse = response
            .json::<LoginResponse>()
            .await
            .map_err(|e| e.to_string())?;

        if let Some(ref user) = login_resp.user {
            if let Some(token) = &user.session_token {
                api::seed_auth_token(token.clone());
            }
        }

        Ok(login_resp)
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn register(
    username: &str,
    email: Option<&str>,
    password: &str,
    invite_token: Option<String>,
) -> Result<UserResponse, String> {
    let req = RegisterRequest {
        username: username.to_string(),
        email: email.map(|e| e.trim().to_string()).filter(|e| !e.is_empty()),
        password: password.to_string(),
        invite_token,
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

pub async fn validate_invite(token: &str) -> Result<InviteInfoResponse, String> {
    let response = api::get(&format!("/auth/invite/{}", token))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<InviteInfoResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to validate invite".to_string())
    }
}

// ── TOTP management ──────────────────────────────────────────────────────────

pub async fn totp_setup() -> Result<TotpSetupResponse, String> {
    let response = api::post("/auth/totp/setup")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TotpSetupResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn totp_enable(code: &str) -> Result<(), String> {
    let req = TotpEnableRequest {
        code: code.to_string(),
    };

    let response = api::post("/auth/totp/enable")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn totp_disable(code: &str) -> Result<(), String> {
    let req = TotpDisableRequest {
        code: code.to_string(),
    };

    let response = api::post("/auth/totp/disable")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

// ── Admin: invites ───────────────────────────────────────────────────────────

pub async fn create_invite(req: CreateInviteRequest) -> Result<InviteResponse, String> {
    let response = api::post("/admin/invites")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<InviteResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn list_invites() -> Result<Vec<InviteResponse>, String> {
    let response = api::get("/admin/invites")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<InviteResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn delete_invite(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/admin/invites/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

// ── Admin: user management ───────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateUserRequest {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<uncloud_common::UserRole>,
}

pub async fn create_user(req: CreateUserRequest) -> Result<(), String> {
    let response = api::post("/admin/users")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    pub role: uncloud_common::UserRole,
    #[serde(default)]
    pub status: uncloud_common::UserStatus,
    pub quota_bytes: Option<i64>,
    pub used_bytes: i64,
    #[serde(default)]
    pub totp_enabled: bool,
    #[serde(default)]
    pub demo: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_users() -> Result<Vec<AdminUserResponse>, String> {
    let response = api::get("/admin/users")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<AdminUserResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn approve_user(id: &str) -> Result<(), String> {
    let response = api::post(&format!("/admin/users/{}/approve", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn disable_user(id: &str) -> Result<(), String> {
    let response = api::post(&format!("/admin/users/{}/disable", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn enable_user(id: &str) -> Result<(), String> {
    let response = api::post(&format!("/admin/users/{}/enable", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
    }
}

pub async fn reset_user_totp(id: &str) -> Result<(), String> {
    let response = api::post(&format!("/admin/users/{}/reset-totp", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(extract_error(&text))
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
