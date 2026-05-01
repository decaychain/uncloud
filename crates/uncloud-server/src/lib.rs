pub mod config;
pub mod db;
pub mod error;
pub mod frontend;
pub mod middleware;
pub mod models;
pub mod processing;
pub mod routes;
pub mod services;
pub mod storage;
pub mod supervisor;
pub mod migrate;
pub mod backup;

use mongodb::Database;

use config::Config;
use services::{AuthService, EventService, RescanService, SearchService, StorageService, SyncLog};

pub struct AppState {
    pub config: Config,
    pub db: Database,
    pub auth: AuthService,
    pub storage: StorageService,
    pub events: EventService,
    pub processing: processing::ProcessingService,
    pub search: SearchService,
    pub rescan: RescanService,
    pub sync_log: SyncLog,
    pub http_client: reqwest::Client,
}
