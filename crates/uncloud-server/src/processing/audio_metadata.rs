use std::io::Cursor;
use std::sync::Arc;

use async_trait::async_trait;
use bson::doc;
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::models::{File, TaskType};
use crate::AppState;
use super::FileProcessor;
use uncloud_common::AudioMeta;

const AUDIO_MIME_TYPES: &[&str] = &[
    "audio/mpeg",
    "audio/flac",
    "audio/ogg",
    "audio/opus",
    "audio/mp4",
    "audio/x-m4a",
    "audio/aac",
    "audio/wav",
    "audio/x-wav",
    "audio/webm",
    "audio/vorbis",
];

pub struct AudioMetadataProcessor {
    pub thumbnail_size: u32,
}

#[async_trait]
impl FileProcessor for AudioMetadataProcessor {
    fn task_type(&self) -> TaskType {
        TaskType::AudioMetadata
    }

    fn applies_to(&self, file: &File) -> bool {
        AUDIO_MIME_TYPES.contains(&file.mime_type.as_str())
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
            .map_err(|e| format!("Failed to read audio file: {}", e))?;

        let thumbnail_size = self.thumbnail_size;
        let file_name = file.name.clone();

        let (meta, cover_jpeg): (AudioMeta, Option<Vec<u8>>) =
            tokio::task::spawn_blocking(move || -> Result<(AudioMeta, Option<Vec<u8>>), String> {
                use lofty::prelude::*;
                use lofty::probe::Probe;

                let cursor = Cursor::new(&data);
                let tagged_file = Probe::new(cursor)
                    .guess_file_type()
                    .map_err(|e| format!("Failed to probe audio: {}", e))?
                    .read()
                    .map_err(|e| format!("Failed to read audio tags: {}", e))?;

                let properties = tagged_file.properties();
                let duration_secs = properties.duration().as_secs_f64();
                let duration_secs = if duration_secs > 0.0 {
                    Some(duration_secs)
                } else {
                    None
                };

                let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag());

                let mut meta = AudioMeta {
                    duration_secs,
                    ..AudioMeta::default()
                };

                let mut cover_jpeg: Option<Vec<u8>> = None;

                if let Some(tag) = tag {
                    use lofty::tag::ItemKey;

                    meta.title = tag
                        .get_string(&ItemKey::TrackTitle)
                        .map(str::to_string)
                        .or_else(|| {
                            file_name
                                .rsplit_once('.')
                                .map(|(stem, _)| stem.to_string())
                        });
                    meta.artist = tag.get_string(&ItemKey::TrackArtist).map(str::to_string);
                    meta.album = tag.get_string(&ItemKey::AlbumTitle).map(str::to_string);
                    meta.album_artist =
                        tag.get_string(&ItemKey::AlbumArtist).map(str::to_string);
                    meta.genre = tag.get_string(&ItemKey::Genre).map(str::to_string);
                    meta.track_number = tag.track();
                    meta.disc_number = tag.disk();
                    meta.year = tag.year().map(|y| y as i32);

                    // Extract front cover art
                    use lofty::picture::PictureType;
                    let cover_pic = tag
                        .pictures()
                        .iter()
                        .find(|p| p.pic_type() == PictureType::CoverFront)
                        .or_else(|| tag.pictures().first());

                    if let Some(pic) = cover_pic {
                        let pic_data = pic.data();
                        if let Ok(img) = image::load_from_memory(pic_data) {
                            let thumb = img.thumbnail(thumbnail_size, thumbnail_size);
                            let mut buf = Cursor::new(Vec::new());
                            if thumb
                                .write_to(&mut buf, image::ImageFormat::Jpeg)
                                .is_ok()
                            {
                                cover_jpeg = Some(buf.into_inner());
                                meta.has_cover_art = true;
                            }
                        }
                    }
                } else {
                    // No tags — fall back to filename as title
                    meta.title = file_name
                        .rsplit_once('.')
                        .map(|(stem, _)| stem.to_string());
                }

                Ok((meta, cover_jpeg))
            })
            .await
            .map_err(|e| format!("spawn_blocking failed: {}", e))??;

        // Write cover art to the .thumbs/ path (same as ThumbnailProcessor)
        if let Some(jpeg) = cover_jpeg {
            let thumb_path = format!(".thumbs/{}.jpg", file.id.to_hex());
            if let Err(e) = backend.write(&thumb_path, &jpeg).await {
                warn!("Failed to write cover art for {}: {}", file.id, e);
                // Non-fatal — continue to write metadata
            }
        }

        // Write metadata to MongoDB: $set metadata.audio
        let meta_bson = mongodb::bson::to_bson(&meta)
            .map_err(|e| format!("Failed to serialise AudioMeta: {}", e))?;

        let collection = state.db.collection::<File>("files");
        collection
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": { "metadata.audio": meta_bson } },
            )
            .await
            .map_err(|e| format!("Failed to update metadata: {}", e))?;

        Ok(())
    }
}
