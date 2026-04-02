use std::sync::Arc;

use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::models::{File, ProcessingStatus, ProcessingTask, TaskType};
use crate::AppState;

use super::FileProcessor;

pub struct ProcessingService {
    processors: Vec<Arc<dyn FileProcessor>>,
    semaphore: Arc<Semaphore>,
    max_attempts: u32,
}

impl ProcessingService {
    pub fn new(max_concurrency: usize, max_attempts: u32) -> Self {
        Self {
            processors: Vec::new(),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_attempts,
        }
    }

    pub fn register(mut self, processor: impl FileProcessor + 'static) -> Self {
        self.processors.push(Arc::new(processor));
        self
    }

    /// Called from upload handlers after a file is created or its content replaced.
    /// Removes any stale tasks of applicable types, inserts fresh Pending entries,
    /// and spawns background work.
    pub async fn enqueue(&self, file: &File, state: Arc<AppState>) {
        let applicable: Vec<Arc<dyn FileProcessor>> = self
            .processors
            .iter()
            .filter(|p| p.applies_to(file))
            .cloned()
            .collect();

        if applicable.is_empty() {
            return;
        }

        let collection = state.db.collection::<File>("files");
        let file_id = file.id;

        for processor in &applicable {
            let type_str = task_type_str(&processor.task_type());

            // Remove any existing task of this type (handles re-upload/content replace).
            let _ = collection
                .update_one(
                    doc! { "_id": file_id },
                    doc! { "$pull": { "processing_tasks": { "task_type": type_str } } },
                )
                .await;

            // Push a fresh Pending task.
            let task = ProcessingTask {
                task_type: processor.task_type(),
                status: ProcessingStatus::Pending,
                attempts: 0,
                error: None,
                queued_at: Utc::now(),
                completed_at: None,
            };
            let task_doc = mongodb::bson::to_document(&task).unwrap();
            let _ = collection
                .update_one(
                    doc! { "_id": file_id },
                    doc! { "$push": { "processing_tasks": task_doc } },
                )
                .await;
        }

        for processor in applicable {
            let file = file.clone();
            let state = state.clone();
            let semaphore = self.semaphore.clone();
            let max_attempts = self.max_attempts;
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await;
                run_task(processor, file, state, max_attempts).await;
            });
        }
    }

    /// Strips all existing tasks of the given type from every file and re-enqueues
    /// them. Used by the admin reindex endpoint when search was previously disabled
    /// and files were incorrectly marked as Done.
    pub async fn reindex_task_type(&self, task_type: &TaskType, state: Arc<AppState>) {
        let type_str = task_type_str(task_type);
        let collection = state.db.collection::<File>("files");

        // Remove all existing tasks of this type so the backfill treats them as unseen.
        if let Err(e) = collection
            .update_many(
                doc! {},
                doc! { "$pull": { "processing_tasks": { "task_type": type_str } } },
            )
            .await
        {
            error!("reindex_task_type: failed to clear {} tasks: {}", type_str, e);
            return;
        }

        // Now re-enqueue for every applicable file via the normal backfill path.
        let processor = match self.processors.iter().find(|p| p.task_type() == *task_type) {
            Some(p) => p.clone(),
            None => {
                error!("reindex_task_type: no processor registered for {:?}", type_str);
                return;
            }
        };

        let mut cursor = match collection.find(doc! {}).await {
            Ok(c) => c,
            Err(e) => {
                error!("reindex_task_type: failed to list files: {}", e);
                return;
            }
        };

        let mut count = 0u32;
        while let Ok(true) = cursor.advance().await {
            let file: File = match cursor.deserialize_current() {
                Ok(f) => f,
                Err(e) => {
                    error!("reindex_task_type: deserialize error: {}", e);
                    continue;
                }
            };
            if processor.applies_to(&file) {
                self.enqueue(&file, state.clone()).await;
                count += 1;
            }
        }

        info!("reindex_task_type: queued {} file(s) for {}", count, type_str);
    }

    /// Called at server startup to resume pending/failed tasks from a previous run.
    pub async fn recover(&self, state: Arc<AppState>) {
        let max_attempts = self.max_attempts;
        let collection = state.db.collection::<File>("files");

        let filter = doc! {
            "deleted_at": mongodb::bson::Bson::Null,
            "processing_tasks": {
                "$elemMatch": {
                    "$or": [
                        { "status": "pending" },
                        { "status": "error", "attempts": { "$lt": max_attempts as i64 } }
                    ]
                }
            }
        };

        let mut cursor = match collection.find(filter).await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to query processing tasks for recovery: {}", e);
                return;
            }
        };

        let mut count = 0u32;
        while let Ok(true) = cursor.advance().await {
            let file: File = match cursor.deserialize_current() {
                Ok(f) => f,
                Err(e) => {
                    error!("Error deserializing file during recovery: {}", e);
                    continue;
                }
            };

            for task in &file.processing_tasks {
                // A search_index task that previously failed because search was
                // disabled or Meilisearch was unreachable should always be retried
                // when search is now available, regardless of attempt count.
                // A search_index task that previously failed because search was
                // disabled or Meilisearch was unreachable should always be retried
                // when search is now available, regardless of attempt count.
                let search_index_retry = task.task_type == TaskType::SearchIndex
                    && task.status == ProcessingStatus::Error
                    && state.search.is_enabled();

                let retryable = task.status == ProcessingStatus::Pending
                    || (task.status == ProcessingStatus::Error
                        && task.attempts < max_attempts)
                    || search_index_retry;
                if !retryable {
                    continue;
                }

                // Reset attempt counter for search_index retries so transient
                // Meilisearch outages don't permanently exhaust the attempts cap.
                if search_index_retry {
                    let type_str = task_type_str(&task.task_type);
                    let _ = collection
                        .update_one(
                            doc! { "_id": file.id, "processing_tasks.task_type": type_str },
                            doc! { "$set": { "processing_tasks.$.attempts": 0 } },
                        )
                        .await;
                }
                if let Some(processor) = self
                    .processors
                    .iter()
                    .find(|p| p.task_type() == task.task_type)
                    .cloned()
                {
                    let file = file.clone();
                    let state = state.clone();
                    let semaphore = self.semaphore.clone();
                    count += 1;
                    tokio::spawn(async move {
                        let _permit = semaphore.acquire().await;
                        run_task(processor, file, state, max_attempts).await;
                    });
                }
            }
        }

        if count > 0 {
            info!("Recovering {} processing task(s) from previous run", count);
        }

        // Phase 2: backfill — enqueue tasks for files that have never been processed
        // (uploaded before the pipeline was introduced, or newly registered processors).
        let mut backfill_count = 0u32;
        for processor in &self.processors {
            let type_str = task_type_str(&processor.task_type());
            let collection = state.db.collection::<File>("files");

            // Files where no task of this type exists yet (exclude trashed files).
            let filter = doc! { "processing_tasks.task_type": { "$ne": type_str }, "deleted_at": mongodb::bson::Bson::Null };

            let mut cursor = match collection.find(filter).await {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to query backfill candidates for {}: {}", type_str, e);
                    continue;
                }
            };

            while let Ok(true) = cursor.advance().await {
                let file: File = match cursor.deserialize_current() {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Error deserializing file during backfill: {}", e);
                        continue;
                    }
                };
                if processor.applies_to(&file) {
                    self.enqueue(&file, state.clone()).await;
                    backfill_count += 1;
                }
            }
        }

        if backfill_count > 0 {
            info!("Backfilling {} processing task(s) for existing files", backfill_count);
        }
    }
}

async fn run_task(
    processor: Arc<dyn FileProcessor>,
    file: File,
    state: Arc<AppState>,
    _max_attempts: u32,
) {
    let task_type = processor.task_type();
    let file_id = file.id;
    let owner_id = file.owner_id;
    let type_str = task_type_str(&task_type);
    let collection = state.db.collection::<File>("files");

    // Increment attempt counter before running.
    let _ = collection
        .update_one(
            doc! { "_id": file_id, "processing_tasks.task_type": type_str },
            doc! { "$inc": { "processing_tasks.$.attempts": 1 } },
        )
        .await;

    match processor.process(&file, state.clone()).await {
        Ok(()) => {
            let now = bson::DateTime::from_chrono(Utc::now());
            let _ = collection
                .update_one(
                    doc! { "_id": file_id, "processing_tasks.task_type": type_str },
                    doc! { "$set": {
                        "processing_tasks.$.status": "done",
                        "processing_tasks.$.completed_at": now,
                        "processing_tasks.$.error": mongodb::bson::Bson::Null,
                    }},
                )
                .await;

            state
                .events
                .emit_processing_completed(owner_id, file_id, task_type, true)
                .await;

            info!(
                "Processing task {:?} completed for file {}",
                type_str, file_id
            );
        }
        Err(e) => {
            error!(
                "Processing task {:?} failed for file {}: {}",
                type_str, file_id, e
            );
            let _ = collection
                .update_one(
                    doc! { "_id": file_id, "processing_tasks.task_type": type_str },
                    doc! { "$set": {
                        "processing_tasks.$.status": "error",
                        "processing_tasks.$.error": &e,
                    }},
                )
                .await;

            state
                .events
                .emit_processing_completed(owner_id, file_id, task_type, false)
                .await;
        }
    }
}

fn task_type_str(t: &TaskType) -> &'static str {
    match t {
        TaskType::Thumbnail => "thumbnail",
        TaskType::AudioMetadata => "audio_metadata",
        TaskType::TextExtract => "text_extract",
        TaskType::SearchIndex => "search_index",
    }
}

/// Returns the ObjectId of the owner so callers don't need to look it up.
#[allow(dead_code)]
fn owner_of(file: &File) -> ObjectId {
    file.owner_id
}
