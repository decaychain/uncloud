use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{User, UserRole};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub role: Option<UserRole>,
    pub quota_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub password: Option<String>,
    pub role: Option<UserRole>,
    pub quota_bytes: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: UserRole,
    pub quota_bytes: Option<i64>,
    pub used_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&User> for UserResponse {
    fn from(u: &User) -> Self {
        Self {
            id: u.id.to_hex(),
            username: u.username.clone(),
            email: u.email.clone(),
            role: u.role,
            quota_bytes: u.quota_bytes,
            used_bytes: u.used_bytes,
            created_at: u.created_at.to_rfc3339(),
            updated_at: u.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list_users(State(state): State<Arc<AppState>>) -> Result<Json<Vec<UserResponse>>> {
    let collection = state.db.collection::<User>("users");
    let mut cursor = collection.find(doc! {}).await?;

    let mut users = Vec::new();
    while cursor.advance().await? {
        let user: User = cursor.deserialize_current()?;
        users.push(UserResponse::from(&user));
    }

    Ok(Json(users))
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>)> {
    // Validate input
    if req.username.len() < 3 || req.username.len() > 32 {
        return Err(AppError::Validation(
            "Username must be between 3 and 32 characters".to_string(),
        ));
    }
    if req.password.len() < 8 {
        return Err(AppError::Validation(
            "Password must be at least 8 characters".to_string(),
        ));
    }
    if !req.email.contains('@') {
        return Err(AppError::Validation("Invalid email address".to_string()));
    }

    let collection = state.db.collection::<User>("users");

    // Check for existing user
    if collection
        .find_one(doc! { "username": &req.username })
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Username already taken".to_string()));
    }
    if collection
        .find_one(doc! { "email": &req.email })
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("Email already registered".to_string()));
    }

    let password_hash = state.auth.hash_password(&req.password)?;

    let mut user = User::new(req.username, req.email, password_hash);
    user.role = req.role.unwrap_or(UserRole::User);
    user.quota_bytes = req.quota_bytes;

    collection.insert_one(&user).await?;

    Ok((StatusCode::CREATED, Json(UserResponse::from(&user))))
}

pub async fn update_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>> {
    let user_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid user ID".to_string()))?;

    let collection = state.db.collection::<User>("users");

    let user = collection
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let mut update = doc! { "$set": { "updated_at": mongodb::bson::DateTime::now() } };

    if let Some(email) = &req.email {
        if !email.contains('@') {
            return Err(AppError::Validation("Invalid email address".to_string()));
        }
        // Check if email is taken by another user
        if let Some(existing) = collection.find_one(doc! { "email": email }).await? {
            if existing.id != user_id {
                return Err(AppError::Conflict("Email already in use".to_string()));
            }
        }
        update.get_document_mut("$set").unwrap().insert("email", email);
    }

    if let Some(password) = &req.password {
        if password.len() < 8 {
            return Err(AppError::Validation(
                "Password must be at least 8 characters".to_string(),
            ));
        }
        let hash = state.auth.hash_password(password)?;
        update
            .get_document_mut("$set")
            .unwrap()
            .insert("password_hash", hash);
    }

    if let Some(role) = &req.role {
        update
            .get_document_mut("$set")
            .unwrap()
            .insert("role", mongodb::bson::to_bson(role).unwrap());
    }

    if let Some(quota) = req.quota_bytes {
        if quota == 0 {
            update
                .get_document_mut("$set")
                .unwrap()
                .insert("quota_bytes", mongodb::bson::Bson::Null);
        } else {
            update
                .get_document_mut("$set")
                .unwrap()
                .insert("quota_bytes", quota);
        }
    }

    collection
        .update_one(doc! { "_id": user_id }, update)
        .await?;

    let updated_user = collection
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok(Json(UserResponse::from(&updated_user)))
}

pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    admin: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let user_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid user ID".to_string()))?;

    // Prevent self-deletion
    if user_id == admin.id {
        return Err(AppError::BadRequest("Cannot delete yourself".to_string()));
    }

    let users_collection = state.db.collection::<User>("users");

    // Verify user exists
    if users_collection
        .find_one(doc! { "_id": user_id })
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("User not found".to_string()));
    }

    // Delete user's files, folders, shares, and sessions
    // In a production system, you'd want to clean up storage too
    state.db.collection::<mongodb::bson::Document>("files")
        .delete_many(doc! { "owner_id": user_id })
        .await?;
    state.db.collection::<mongodb::bson::Document>("folders")
        .delete_many(doc! { "owner_id": user_id })
        .await?;
    state.db.collection::<mongodb::bson::Document>("shares")
        .delete_many(doc! { "owner_id": user_id })
        .await?;
    state.db.collection::<mongodb::bson::Document>("sessions")
        .delete_many(doc! { "user_id": user_id })
        .await?;

    // Delete user
    users_collection.delete_one(doc! { "_id": user_id }).await?;

    Ok(StatusCode::NO_CONTENT)
}
