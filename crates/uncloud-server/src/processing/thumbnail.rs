use std::io::Cursor;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;

use crate::models::{File, TaskType};
use crate::AppState;

use super::FileProcessor;

const IMAGE_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
];

pub struct ThumbnailProcessor {
    pub size: u32,
}

#[async_trait]
impl FileProcessor for ThumbnailProcessor {
    fn task_type(&self) -> TaskType {
        TaskType::Thumbnail
    }

    fn applies_to(&self, file: &File) -> bool {
        IMAGE_MIME_TYPES.contains(&file.mime_type.as_str())
    }

    async fn process(&self, file: &File, state: Arc<AppState>) -> Result<(), String> {
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
            .map_err(|e| format!("Failed to read image: {}", e))?;

        let size = self.size;

        let jpeg_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let img = image::load_from_memory(&data)
                .map_err(|e| format!("Failed to decode image: {}", e))?;

            let thumb = img.thumbnail(size, size);

            let mut buf = Cursor::new(Vec::new());
            thumb
                .write_to(&mut buf, image::ImageFormat::Jpeg)
                .map_err(|e| format!("Failed to encode thumbnail: {}", e))?;

            Ok(buf.into_inner())
        })
        .await
        .map_err(|e| format!("Spawn blocking failed: {}", e))??;

        let thumb_path = format!(".thumbs/{}.jpg", file.id.to_hex());
        backend
            .write(&thumb_path, &jpeg_bytes)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
