use std::io::Cursor;
use std::sync::Arc;

use async_trait::async_trait;
use bson::doc;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use image::metadata::Orientation;
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

struct ExifInfo {
    orientation: Orientation,
    captured_at: Option<DateTime<Utc>>,
}

/// Parse EXIF from raw image bytes. Returns orientation (default
/// `NoTransforms`) and `DateTimeOriginal` / `DateTime` when present.
fn parse_exif(data: &[u8]) -> ExifInfo {
    let mut info = ExifInfo {
        orientation: Orientation::NoTransforms,
        captured_at: None,
    };

    let mut cursor = Cursor::new(data);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut cursor) else {
        return info;
    };

    if let Some(field) = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
        if let Some(v) = field.value.get_uint(0) {
            if let Some(o) = Orientation::from_exif(v.min(255) as u8) {
                info.orientation = o;
            }
        }
    }

    // DateTimeOriginal is the shutter-press time; fall back to DateTime
    // (last modified) if the original is missing.
    let datetime_field = exif
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY));

    if let Some(field) = datetime_field {
        if let exif::Value::Ascii(ref vec) = field.value {
            if let Some(bytes) = vec.first() {
                if let Ok(s) = std::str::from_utf8(bytes) {
                    // EXIF format: "YYYY:MM:DD HH:MM:SS" (local time, no zone).
                    // Store as UTC — accurate ordering is more important than
                    // absolute wall-clock correctness for gallery grouping.
                    if let Ok(naive) = NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S") {
                        info.captured_at = Some(Utc.from_utc_datetime(&naive));
                    }
                }
            }
        }
    }

    info
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

        let (jpeg_bytes, captured_at) = tokio::task::spawn_blocking(
            move || -> Result<(Vec<u8>, Option<DateTime<Utc>>), String> {
                let exif = parse_exif(&data);

                let mut img = image::load_from_memory(&data)
                    .map_err(|e| format!("Failed to decode image: {}", e))?;
                img.apply_orientation(exif.orientation);

                let thumb = img.thumbnail(size, size);

                let mut buf = Cursor::new(Vec::new());
                thumb
                    .write_to(&mut buf, image::ImageFormat::Jpeg)
                    .map_err(|e| format!("Failed to encode thumbnail: {}", e))?;

                Ok((buf.into_inner(), exif.captured_at))
            },
        )
        .await
        .map_err(|e| format!("Spawn blocking failed: {}", e))??;

        let thumb_path = format!(".thumbs/{}.jpg", file.id.to_hex());
        backend
            .write(&thumb_path, &jpeg_bytes)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(dt) = captured_at {
            let collection = state.db.collection::<File>("files");
            collection
                .update_one(
                    doc! { "_id": file.id },
                    doc! { "$set": { "captured_at": bson::DateTime::from_chrono(dt) } },
                )
                .await
                .map_err(|e| format!("Failed to store captured_at: {}", e))?;
        }

        Ok(())
    }
}
