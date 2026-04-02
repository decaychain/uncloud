use meilisearch_sdk::client::Client;
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::SearchConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDocument {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub mime_type: String,
    pub content_text: String,
    pub parent_id: Option<String>,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub struct SearchService {
    client: Client,
    enabled: bool,
}

impl SearchService {
    pub async fn new(config: &SearchConfig) -> Result<Self, String> {
        let client = Client::new(&config.url, config.api_key.as_deref())
            .map_err(|e| format!("Failed to create Meilisearch client: {}", e))?;

        if !config.enabled {
            return Ok(Self {
                client,
                enabled: false,
            });
        }

        // Create or ensure "files" index exists.
        let task = client
            .create_index("files", Some("id"))
            .await
            .map_err(|e| format!("Meilisearch create_index failed: {}", e))?;
        task.wait_for_completion(&client, None, None)
            .await
            .map_err(|e| format!("Meilisearch index creation wait failed: {}", e))?;

        let index = client.index("files");

        index
            .set_filterable_attributes(&["owner_id"])
            .await
            .map_err(|e| format!("set_filterable_attributes failed: {}", e))?;

        index
            .set_searchable_attributes(&["name", "content_text"])
            .await
            .map_err(|e| format!("set_searchable_attributes failed: {}", e))?;

        info!("Meilisearch index 'files' ready at {}", config.url);
        Ok(Self {
            client,
            enabled: true,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn index_file(&self, doc: SearchDocument) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        self.client
            .index("files")
            .add_or_replace(&[doc], Some("id"))
            .await
            .map_err(|e| format!("index_file failed: {}", e))?;
        Ok(())
    }

    pub async fn delete_file(&self, file_id: &str) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        self.client
            .index("files")
            .delete_document(file_id)
            .await
            .map_err(|e| format!("delete_file failed: {}", e))?;
        Ok(())
    }

    pub async fn search(
        &self,
        owner_id: ObjectId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<uncloud_common::SearchHit>, String> {
        if !self.enabled {
            return Ok(vec![]);
        }
        let filter = format!("owner_id = \"{}\"", owner_id.to_hex());
        let results = self
            .client
            .index("files")
            .search()
            .with_query(query)
            .with_filter(&filter)
            .with_limit(limit)
            .execute::<uncloud_common::SearchHit>()
            .await
            .map_err(|e| format!("search failed: {}", e))?;
        Ok(results.hits.into_iter().map(|h| h.result).collect())
    }
}
