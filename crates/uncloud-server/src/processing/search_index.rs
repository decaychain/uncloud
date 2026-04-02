use std::sync::Arc;

use async_trait::async_trait;
use mongodb::bson::doc;

use crate::models::file::{File, TaskType};
use crate::services::search::SearchDocument;
use crate::AppState;

use super::FileProcessor;

pub struct SearchIndexProcessor;

#[async_trait]
impl FileProcessor for SearchIndexProcessor {
    fn task_type(&self) -> TaskType {
        TaskType::SearchIndex
    }

    fn applies_to(&self, _file: &File) -> bool {
        true // Index all files (name-only for binaries, name+text for extracted files)
    }

    async fn process(&self, file: &File, state: Arc<AppState>) -> Result<(), String> {
        if !state.search.is_enabled() {
            // Return an error so the task stays retryable. When the server is
            // restarted with search enabled, recover() will pick it up again.
            return Err("Search is not enabled".to_string());
        }

        // Re-read to get content_text that TextExtractProcessor may have written.
        let collection = state.db.collection::<File>("files");
        let fresh = collection
            .find_one(doc! { "_id": file.id })
            .await
            .map_err(|e| format!("DB read failed: {}", e))?
            .ok_or_else(|| "File no longer exists".to_string())?;

        let content_text = fresh
            .metadata
            .get("content_text")
            .and_then(|b| {
                if let mongodb::bson::Bson::String(s) = b {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let doc = SearchDocument {
            id: fresh.id.to_hex(),
            owner_id: fresh.owner_id.to_hex(),
            name: fresh.name.clone(),
            mime_type: fresh.mime_type.clone(),
            content_text,
            parent_id: fresh.parent_id.map(|id| id.to_hex()),
            size_bytes: fresh.size_bytes,
            created_at: fresh.created_at.to_rfc3339(),
            updated_at: fresh.updated_at.to_rfc3339(),
        };

        state.search.index_file(doc).await
    }
}
