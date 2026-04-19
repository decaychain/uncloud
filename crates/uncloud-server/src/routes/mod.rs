pub mod auth;
pub mod files;
pub mod folders;
pub mod invites;
pub mod music;
pub mod playlists;
pub mod search;
pub mod shares;
pub mod storages;
pub mod trash;
pub mod users;
pub mod events;
pub mod versions;
pub mod tokens;
pub mod s3_credentials;
pub mod s3;
pub mod apps;
pub mod shopping;
pub mod folder_shares;
pub mod vault_recents;
pub mod tasks;
pub mod task_items;
pub mod admin_processing;

use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{any, delete, get, post, put},
    Router,
};
use std::sync::Arc;

use crate::middleware::{admin_middleware, auth_middleware, sigv4_middleware};
use crate::AppState;

pub fn create_router(state: Arc<AppState>) -> Router {
    // -- Public routes (no auth) defined once, nested under /api and /api/v1 --
    let public_api = Router::new()
        .route("/auth/server-info", get(auth::server_info))
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/demo", post(auth::demo_login))
        .route("/auth/totp/verify", post(auth::totp_verify))
        .route("/auth/invite/{token}", get(auth::validate_invite))
        .route("/public/{token}", get(shares::get_public_share))
        .route("/public/{token}/download", get(shares::download_public))
        .route("/public/{token}/verify", post(shares::verify_share_password));

    // Public v1-only routes (app registration, webhooks — secret-protected, no user auth)
    let public_v1_only = Router::new()
        .route("/apps/register", post(apps::register_app))
        .route("/apps/webhooks", post(apps::register_webhook));

    let public_routes = Router::new()
        .route("/health", get(health_check))
        .nest("/api", public_api.clone())
        .nest("/api/v1", public_api.merge(public_v1_only));

    // -- Authenticated routes defined once, nested under /api and /api/v1 --
    let auth_api = Router::new()
        // Auth
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/{id}", delete(auth::revoke_session))
        // Password
        .route("/auth/change-password", post(auth::change_password))
        // TOTP
        .route("/auth/totp/setup", post(auth::totp_setup))
        .route("/auth/totp/enable", post(auth::totp_enable))
        .route("/auth/totp/disable", post(auth::totp_disable))
        // Files
        .route("/files", get(files::list_files))
        .route("/files/{id}", get(files::get_file))
        .route("/files/{id}", put(files::update_file))
        .route("/files/{id}", delete(files::delete_file))
        .route("/files/{id}/download", get(files::download_file))
        .route("/files/{id}/copy", post(files::copy_file))
        .route("/files/{id}/thumb", get(files::get_thumbnail))
        .route("/files/{id}/content", post(files::update_file_content)
            .layer(DefaultBodyLimit::disable()))
        .route("/files/{id}/versions", get(versions::list_versions))
        .route("/files/{file_id}/versions/{version_id}", get(versions::download_version))
        .route("/files/{file_id}/versions/{version_id}/restore", post(versions::restore_version))
        // Uploads
        .route("/uploads/init", post(files::init_upload))
        .route("/uploads/simple", post(files::simple_upload)
            .layer(DefaultBodyLimit::disable()))
        .route("/uploads/{id}/chunk", post(files::upload_chunk)
            .layer(DefaultBodyLimit::disable()))
        .route("/uploads/{id}/complete", post(files::complete_upload))
        .route("/uploads/{id}", delete(files::cancel_upload))
        // Folders
        .route("/folders", get(folders::list_folders))
        .route("/folders", post(folders::create_folder))
        .route("/folders/{id}", get(folders::get_folder))
        .route("/folders/{id}", put(folders::update_folder))
        .route("/folders/{id}", delete(folders::delete_folder))
        .route("/folders/{id}/copy", post(folders::copy_folder))
        .route("/folders/{id}/breadcrumb", get(folders::get_folder_breadcrumb))
        .route("/folders/{id}/effective-strategy", get(folders::get_effective_strategy))
        .route("/sync/tree", get(folders::sync_tree))
        // Gallery
        .route("/gallery", get(files::list_gallery))
        .route("/gallery/albums", get(files::list_gallery_albums))
        // Music
        .route("/music/tracks", get(music::list_music_tracks))
        .route("/music/folders", get(music::list_music_folders))
        .route("/music/artists", get(music::list_artists))
        .route("/music/artists/{name}/albums", get(music::list_artist_albums))
        .route("/music/albums/{artist}/{album}/tracks", get(music::list_album_tracks))
        // Playlists
        .route("/playlists", get(playlists::list_playlists))
        .route("/playlists", post(playlists::create_playlist))
        .route("/playlists/{id}", get(playlists::get_playlist))
        .route("/playlists/{id}", put(playlists::update_playlist))
        .route("/playlists/{id}", delete(playlists::delete_playlist))
        .route("/playlists/{id}/tracks", post(playlists::add_tracks))
        .route("/playlists/{id}/tracks", delete(playlists::remove_tracks))
        .route("/playlists/{id}/tracks/reorder", put(playlists::reorder_tracks))
        // Users (non-admin)
        .route("/users/names", get(users::list_usernames))
        // Shares
        .route("/shares", get(shares::list_shares))
        .route("/shares", post(shares::create_share))
        .route("/shares/{id}", delete(shares::delete_share))
        // Trash
        .route("/trash", get(trash::list_trash))
        .route("/trash", delete(trash::empty_trash))
        .route("/trash/{id}/restore", post(trash::restore_from_trash))
        .route("/trash/{id}", delete(trash::permanently_delete))
        // Search
        .route("/search/status", get(search::search_status))
        .route("/search", get(search::search_files))
        .route("/search/reindex", post(search::reindex))
        // Events (SSE)
        .route("/events", get(events::events_stream))
        // Shopping
        .route("/shopping/items", get(shopping::list_items).post(shopping::create_item))
        .route("/shopping/items/{id}", put(shopping::update_item).delete(shopping::delete_item))
        .route("/shopping/lists", get(shopping::list_lists).post(shopping::create_list))
        .route("/shopping/lists/{id}", put(shopping::update_list).delete(shopping::delete_list))
        .route("/shopping/lists/{id}/items", get(shopping::get_list_items).post(shopping::add_list_item))
        .route("/shopping/lists/{id}/items/{item_id}", axum::routing::patch(shopping::patch_list_item).delete(shopping::remove_list_item))
        .route("/shopping/lists/{id}/items/{item_id}/position", put(shopping::update_item_position))
        .route("/shopping/lists/{id}/remove-purchased", post(shopping::remove_purchased))
        .route("/shopping/lists/{id}/share", post(shopping::share_list))
        .route("/shopping/lists/{id}/share/{user_id}", delete(shopping::unshare_list))
        // Vault recents
        .route("/vault-recents", get(vault_recents::list_recent_vaults).post(vault_recents::add_recent_vault))
        .route("/vault-recents/{file_id}", delete(vault_recents::remove_recent_vault))
        // Shopping categories
        .route("/shopping/categories", get(shopping::list_categories).post(shopping::create_category))
        .route("/shopping/categories/{id}", put(shopping::update_category).delete(shopping::delete_category))
        .route("/shopping/categories/{id}/position", put(shopping::update_category_position))
        // Shopping shops
        .route("/shopping/shops", get(shopping::list_shops).post(shopping::create_shop))
        .route("/shopping/shops/{id}", put(shopping::update_shop).delete(shopping::delete_shop))
        // Folder shares
        .route("/folder-shares", post(folder_shares::create_share))
        .route("/folder-shares/by-me", get(folder_shares::list_shares_by_me))
        .route("/folder-shares/with-me", get(folder_shares::list_shares_with_me))
        .route("/folder-shares/folder/{id}", get(folder_shares::list_folder_shares))
        .route("/folder-shares/{id}", put(folder_shares::update_share).delete(folder_shares::delete_share))
        // Tasks
        .route("/tasks/projects", get(tasks::list_projects).post(tasks::create_project))
        .route("/tasks/projects/{id}", get(tasks::get_project).put(tasks::update_project).delete(tasks::delete_project))
        .route("/tasks/projects/{id}/members", post(tasks::add_project_member))
        .route("/tasks/projects/{id}/members/{user_id}", put(tasks::update_project_member).delete(tasks::remove_project_member))
        .route("/tasks/projects/{id}/sections", get(tasks::list_sections).post(tasks::create_section))
        .route("/tasks/projects/{id}/sections/reorder", put(tasks::reorder_sections))
        .route("/tasks/projects/{id}/labels", get(tasks::list_labels).post(tasks::create_label))
        .route("/tasks/projects/{id}/tasks", get(task_items::list_tasks).post(task_items::create_task))
        .route("/tasks/projects/{id}/tasks/reorder", put(task_items::reorder_tasks))
        .route("/tasks/sections/{id}", put(tasks::update_section).delete(tasks::delete_section))
        .route("/tasks/labels/{id}", put(tasks::update_label).delete(tasks::delete_label))
        .route("/tasks/{id}", get(task_items::get_task).put(task_items::update_task).delete(task_items::delete_task))
        .route("/tasks/{id}/status", put(task_items::update_task_status))
        .route("/tasks/{id}/subtasks", post(task_items::create_subtask))
        .route("/tasks/{id}/promote", post(task_items::promote_subtask))
        .route("/tasks/{id}/attachments", post(task_items::attach_files))
        .route("/tasks/{id}/attachments/{file_id}", delete(task_items::detach_file))
        .route("/tasks/{id}/comments", get(task_items::list_comments).post(task_items::create_comment))
        .route("/tasks/comments/{id}", put(task_items::update_comment).delete(task_items::delete_comment))
        .route("/tasks/schedule", get(task_items::get_schedule))
        .route("/tasks/assigned-to-me", get(task_items::get_assigned_to_me));

    // v1-only routes (API tokens, S3 credentials, apps)
    let v1_only = Router::new()
        .route("/auth/tokens", post(tokens::create_token))
        .route("/auth/tokens", get(tokens::list_tokens))
        .route("/auth/tokens/{id}", delete(tokens::delete_token))
        .route("/s3/credentials", post(s3_credentials::create_credential))
        .route("/s3/credentials", get(s3_credentials::list_credentials))
        .route("/s3/credentials/{id}", delete(s3_credentials::delete_credential))
        .route("/apps", get(apps::list_apps))
        .route("/auth/me/features", put(auth::update_my_features))
        .route("/auth/me/preferences", put(auth::update_my_preferences));

    let auth_routes = Router::new()
        .nest("/api", auth_api.clone())
        .nest("/api/v1", auth_api.merge(v1_only))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // -- Admin routes defined once, nested under /api and /api/v1 --
    let admin_api = Router::new()
        .route("/admin/storages", get(storages::list_storages))
        .route("/admin/storages", post(storages::create_storage))
        .route("/admin/storages/{id}", put(storages::update_storage))
        .route("/admin/storages/{id}", delete(storages::delete_storage))
        .route("/admin/storages/{id}/rescan", post(storages::rescan_storage))
        .route("/admin/users", get(users::list_users))
        .route("/admin/users", post(users::create_user))
        .route("/admin/users/{id}", put(users::update_user))
        .route("/admin/users/{id}", delete(users::delete_user))
        .route("/admin/users/{id}/approve", post(users::approve_user))
        .route("/admin/users/{id}/disable", post(users::disable_user))
        .route("/admin/users/{id}/enable", post(users::enable_user))
        .route("/admin/users/{id}/reset-totp", post(users::reset_user_totp))
        .route("/admin/users/{id}/reset-password", post(users::reset_user_password))
        .route("/admin/users/{id}/role", post(users::change_user_role))
        // Invites
        .route("/admin/invites", get(invites::list_invites))
        .route("/admin/invites", post(invites::create_invite))
        .route("/admin/invites/{id}", delete(invites::delete_invite))
        // Processing
        .route("/admin/processing/rerun", post(admin_processing::rerun_all));

    let admin_v1_only = Router::new()
        .route("/apps/{name}", delete(apps::delete_app))
        .route("/apps/webhooks/{id}", delete(apps::delete_webhook));

    let admin_routes = Router::new()
        .nest("/api", admin_api.clone())
        .nest("/api/v1", admin_api.merge(admin_v1_only))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_middleware,
        ));

    // S3 routes — authenticated via SigV4 middleware
    let s3_routes = Router::new()
        .route("/s3", any(s3::s3_handler))
        .route("/s3/", any(s3::s3_handler))
        .route("/s3/{*rest}", any(s3::s3_handler)
            .layer(DefaultBodyLimit::disable()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            sigv4_middleware,
        ));

    // App proxy routes — authenticated, mounted outside /api.
    // Three routes cover: /apps/shopping  /apps/shopping/  /apps/shopping/any/path
    let app_proxy_routes = Router::new()
        .route("/apps/{name}", any(apps::proxy_handler))
        .route("/apps/{name}/", any(apps::proxy_handler))
        .route("/apps/{name}/{*path}", any(apps::proxy_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .merge(public_routes)
        .merge(auth_routes)
        .merge(admin_routes)
        .merge(s3_routes)
        .merge(app_proxy_routes)
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}
