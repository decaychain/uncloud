use serde::Deserialize;

use super::api;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub nav_label: String,
    pub icon: String,
}

pub async fn list_apps() -> Result<Vec<AppEntry>, String> {
    let response = api::get_v1("/apps")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<AppEntry>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load apps".to_string())
    }
}
