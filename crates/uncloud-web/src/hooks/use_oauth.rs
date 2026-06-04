use serde::{Deserialize, Serialize};

use super::api;

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthClient {
    pub client_name: String,
}

pub async fn lookup_client(client_id: &str) -> Result<OAuthClient, String> {
    let url = format!(
        "/oauth/clients/lookup?client_id={}",
        urlencoding::encode(client_id)
    );
    let response = api::get_raw(&format!("{}{}", api::api_base(), url))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("client lookup failed ({})", response.status()));
    }
    response
        .json::<OAuthClient>()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorizeSubmit {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub scope: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub decision: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorizeResponse {
    pub redirect_to: String,
}

pub async fn submit_authorize(body: AuthorizeSubmit) -> Result<AuthorizeResponse, String> {
    let response = api::post_v1("/oauth/authorize")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        let text = response.text().await.unwrap_or_default();
        return Err(format!("authorize failed: {}", text));
    }
    response
        .json::<AuthorizeResponse>()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ConnectedApp {
    pub client_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub last_issued_at: String,
    pub token_count: u64,
}

pub async fn list_connected_apps() -> Result<Vec<ConnectedApp>, String> {
    let response = api::get_v1("/oauth/connected-apps")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!(
            "failed to list connected apps ({})",
            response.status()
        ));
    }
    response
        .json::<Vec<ConnectedApp>>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn revoke_connected_app(client_id: &str) -> Result<(), String> {
    let url = format!("/oauth/connected-apps/{}", urlencoding::encode(client_id));
    let response = api::delete_v1(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status() {
        204 | 200 => Ok(()),
        s => Err(format!("revoke failed ({})", s)),
    }
}
