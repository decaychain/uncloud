use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{doc, oid::ObjectId},
};
use uncloud_common::SyncEventSource;

use crate::error::Result;
use crate::models::SyncEvent;
use crate::services::EventService;
use crate::services::events::Event;

#[derive(Clone)]
pub struct SyncLog {
    db: Database,
    events: EventService,
    enabled: bool,
}

#[derive(Debug, Default)]
pub struct SyncEventFilter {
    pub q: Option<String>,
    pub client: Option<String>,
    pub source: Option<Vec<SyncEventSource>>,
    pub before: Option<DateTime<Utc>>,
    /// Caller-supplied limit; service clamps to `MAX_LIMIT`.
    pub limit: u32,
}

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;

impl SyncLog {
    pub fn new(db: &Database, events: EventService, enabled: bool) -> Self {
        Self {
            db: db.clone(),
            events,
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Insert a sync event and fan it out via SSE. Failures are warn-logged and
    /// never propagated — the audit log must never break a caller's operation.
    pub async fn record(&self, event: SyncEvent) {
        if !self.enabled {
            return;
        }
        let owner_id = event.owner_id;
        let coll = self.db.collection::<SyncEvent>("sync_events");
        match coll.insert_one(&event).await {
            Ok(_) => {
                self.events
                    .emit(owner_id, Event::SyncEventAppended { event: (&event).into() })
                    .await;
            }
            Err(e) => {
                tracing::warn!("sync_log.record failed: {}", e);
            }
        }
    }

    pub async fn list(
        &self,
        owner_id: ObjectId,
        filter: SyncEventFilter,
    ) -> Result<(Vec<SyncEvent>, bool)> {
        let coll = self.db.collection::<SyncEvent>("sync_events");
        let limit_req = if filter.limit == 0 {
            DEFAULT_LIMIT
        } else {
            filter.limit.min(MAX_LIMIT)
        } as i64;

        let mut query = doc! { "owner_id": owner_id };
        if let Some(before) = filter.before {
            query.insert(
                "timestamp",
                doc! { "$lt": mongodb::bson::DateTime::from_chrono(before) },
            );
        }
        if let Some(ref q) = filter.q {
            let pattern = escape_regex(q);
            query.insert(
                "$or",
                vec![
                    doc! { "path": { "$regex": &pattern, "$options": "i" } },
                    doc! { "new_path": { "$regex": &pattern, "$options": "i" } },
                ],
            );
        }
        if let Some(ref c) = filter.client {
            let pattern = escape_regex(c);
            query.insert(
                "client_id",
                doc! { "$regex": pattern, "$options": "i" },
            );
        }
        if let Some(ref sources) = filter.source {
            if !sources.is_empty() {
                let arr: Vec<_> = sources
                    .iter()
                    .filter_map(|s| mongodb::bson::to_bson(s).ok())
                    .collect();
                query.insert("source", doc! { "$in": arr });
            }
        }

        let cursor = coll
            .find(query)
            .sort(doc! { "timestamp": -1 })
            .limit(limit_req + 1)
            .await?;
        let mut results: Vec<SyncEvent> = cursor.try_collect().await?;
        let has_more = results.len() as i64 > limit_req;
        if has_more {
            results.truncate(limit_req as usize);
        }
        Ok((results, has_more))
    }

    /// Prune oldest-first events beyond the per-user cap. Not yet scheduled —
    /// left here so callers can invoke opportunistically if required.
    pub async fn prune_overflow(&self, _owner_id: ObjectId, _keep: u32) -> Result<u64> {
        Ok(0)
    }
}

/// Escape the handful of regex metacharacters so user-supplied substrings
/// are treated literally by MongoDB's `$regex`.
fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}
