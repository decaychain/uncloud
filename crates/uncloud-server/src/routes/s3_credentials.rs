use axum::{
    extract::{Path, State},
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::S3Credential;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateS3CredentialRequest {
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct CreateS3CredentialResponse {
    pub id: String,
    pub access_key_id: String,
    /// The secret is only returned once at creation time.
    pub secret_access_key: String,
    pub label: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct S3CredentialResponse {
    pub id: String,
    pub access_key_id: String,
    pub label: String,
    pub created_at: String,
}

/// Generate a random access key (20 chars, uppercase + digits like AWS).
fn generate_access_key() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generate a random secret key (40 chars, base62).
fn generate_secret_key() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..40)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

pub async fn create_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateS3CredentialRequest>,
) -> Result<Json<CreateS3CredentialResponse>> {
    if req.label.is_empty() || req.label.len() > 128 {
        return Err(AppError::BadRequest(
            "Label must be between 1 and 128 characters".to_string(),
        ));
    }

    let access_key_id = generate_access_key();
    let secret_access_key = generate_secret_key();

    let cred = S3Credential::new(
        user.id,
        access_key_id.clone(),
        secret_access_key.clone(),
        req.label.clone(),
    );

    let collection = state.db.collection::<S3Credential>("s3_credentials");
    collection.insert_one(&cred).await?;

    Ok(Json(CreateS3CredentialResponse {
        id: cred.id.to_hex(),
        access_key_id,
        secret_access_key,
        label: cred.label,
        created_at: cred.created_at.to_rfc3339(),
    }))
}

pub async fn list_credentials(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<S3CredentialResponse>>> {
    let collection = state.db.collection::<S3Credential>("s3_credentials");
    let mut cursor = collection
        .find(doc! { "user_id": user.id })
        .await?;

    let mut creds = Vec::new();
    while cursor.advance().await? {
        let c: S3Credential = cursor.deserialize_current()?;
        creds.push(S3CredentialResponse {
            id: c.id.to_hex(),
            access_key_id: c.access_key_id,
            label: c.label,
            created_at: c.created_at.to_rfc3339(),
        });
    }

    Ok(Json(creds))
}

pub async fn delete_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    let cred_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid credential ID".to_string()))?;

    let collection = state.db.collection::<S3Credential>("s3_credentials");
    let result = collection
        .delete_one(doc! { "_id": cred_id, "user_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Credential not found".to_string()));
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}
