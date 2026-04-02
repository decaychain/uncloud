use serde::Deserialize;
use uncloud_common::SearchHit;

use super::api;

#[derive(Deserialize)]
struct SearchStatus {
    enabled: bool,
}

pub async fn fetch_search_enabled() -> bool {
    let Ok(response) = api::get("/search/status")
        .send()
        .await
    else {
        return false;
    };
    if !response.ok() {
        return false;
    }
    response
        .json::<SearchStatus>()
        .await
        .map(|s| s.enabled)
        .unwrap_or(false)
}

pub async fn trigger_reindex() -> Result<(), String> {
    let response = api::post("/search/reindex")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status() {
        202 => Ok(()),
        503 => Err("Search is not enabled on the server.".to_string()),
        _ => Err(format!("Reindex failed ({})", response.status())),
    }
}

pub async fn search_files(query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
    let encoded = js_sys::encode_uri_component(query);
    let url = format!("{}/search?q={}&limit={}", api::api_url(""), encoded, limit);
    let response = api::get_raw(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.ok() {
        response
            .json::<Vec<SearchHit>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Ok(vec![])
    }
}
