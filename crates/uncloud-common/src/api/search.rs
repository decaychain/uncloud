use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub parent_id: Option<String>,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}
