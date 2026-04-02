pub mod audio_metadata;
pub mod search_index;
pub mod service;
pub mod text_extract;
pub mod thumbnail;

use async_trait::async_trait;
use std::sync::Arc;

use crate::models::{File, TaskType};
use crate::AppState;

#[async_trait]
pub trait FileProcessor: Send + Sync {
    fn task_type(&self) -> TaskType;
    fn applies_to(&self, file: &File) -> bool;
    async fn process(&self, file: &File, state: Arc<AppState>) -> Result<(), String>;
}

pub use audio_metadata::AudioMetadataProcessor;
pub use search_index::SearchIndexProcessor;
pub use service::ProcessingService;
pub use text_extract::TextExtractProcessor;
pub use thumbnail::ThumbnailProcessor;
