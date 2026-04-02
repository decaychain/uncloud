use std::sync::Arc;

use async_trait::async_trait;
use mongodb::bson::doc;
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::models::file::{File, TaskType};
use crate::services::search::SearchDocument;
use crate::AppState;

use super::FileProcessor;

const MAX_TEXT_BYTES: usize = 1_048_576; // 1 MB

pub struct TextExtractProcessor;

#[async_trait]
impl FileProcessor for TextExtractProcessor {
    fn task_type(&self) -> TaskType {
        TaskType::TextExtract
    }

    fn applies_to(&self, file: &File) -> bool {
        let mime = file.mime_type.as_str();
        mime.starts_with("text/")
            || mime == "application/pdf"
            || mime == "application/json"
            || mime == "application/xml"
            || mime == "application/javascript"
            || mime.starts_with("audio/")
    }

    async fn process(&self, file: &File, state: Arc<AppState>) -> Result<(), String> {
        let text = if file.mime_type.starts_with("audio/") {
            extract_audio_text(file)
        } else if file.mime_type == "application/pdf" {
            extract_pdf_text(file, &state).await?
        } else {
            extract_plain_text(file, &state).await?
        };

        if text.is_empty() {
            return Ok(());
        }

        let collection = state.db.collection::<File>("files");
        collection
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": { "metadata.content_text": &text } },
            )
            .await
            .map_err(|e| format!("Failed to store content_text: {}", e))?;

        // Re-index in Meilisearch with the extracted text.
        if state.search.is_enabled() {
            let search_doc = SearchDocument {
                id: file.id.to_hex(),
                owner_id: file.owner_id.to_hex(),
                name: file.name.clone(),
                mime_type: file.mime_type.clone(),
                content_text: text,
                parent_id: file.parent_id.map(|id| id.to_hex()),
                size_bytes: file.size_bytes,
                created_at: file.created_at.to_rfc3339(),
                updated_at: file.updated_at.to_rfc3339(),
            };
            if let Err(e) = state.search.index_file(search_doc).await {
                warn!("Search re-index after text extract failed: {}", e);
            }
        }

        Ok(())
    }
}

fn extract_audio_text(file: &File) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(mongodb::bson::Bson::Document(audio)) = file.metadata.get("audio") {
        for key in &["title", "artist", "album", "album_artist", "genre"] {
            if let Some(mongodb::bson::Bson::String(val)) = audio.get(*key) {
                if !val.is_empty() {
                    parts.push(val.clone());
                }
            }
        }
    }
    if let Some((stem, _)) = file.name.rsplit_once('.') {
        parts.push(stem.to_string());
    }
    parts.join(" ")
}

async fn extract_plain_text(file: &File, state: &AppState) -> Result<String, String> {
    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut reader = backend
        .read(&file.storage_path)
        .await
        .map_err(|e| e.to_string())?;
    let read_size = (file.size_bytes as usize).min(MAX_TEXT_BYTES);
    let mut buf = vec![0u8; read_size];
    let n = reader
        .read(&mut buf)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn extract_pdf_text(file: &File, state: &AppState) -> Result<String, String> {
    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut reader = backend
        .read(&file.storage_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    reader
        .read_to_end(&mut data)
        .await
        .map_err(|e| format!("Failed to read PDF: {}", e))?;
    tokio::task::spawn_blocking(move || {
        pdf_extract::extract_text_from_mem(&data)
            .map(|text| {
                if text.len() > MAX_TEXT_BYTES {
                    text[..MAX_TEXT_BYTES].to_string()
                } else {
                    text
                }
            })
            .unwrap_or_else(|e| {
                warn!("PDF extraction failed: {}", e);
                String::new()
            })
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {}", e))
}
