use axum::{
    extract::{Path, State},
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::ApiToken;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTokenResponse {
    pub id: String,
    pub name: String,
    pub token: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

fn generate_bearer_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Json<CreateTokenResponse>> {
    if req.name.is_empty() || req.name.len() > 128 {
        return Err(AppError::BadRequest(
            "Token name must be between 1 and 128 characters".to_string(),
        ));
    }

    let raw_token = generate_bearer_token();
    let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));

    let api_token = ApiToken::new(user.id, req.name.clone(), token_hash);

    let collection = state.db.collection::<ApiToken>("api_tokens");
    collection.insert_one(&api_token).await?;

    Ok(Json(CreateTokenResponse {
        id: api_token.id.to_hex(),
        name: api_token.name,
        token: raw_token,
        created_at: api_token.created_at.to_rfc3339(),
    }))
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<TokenResponse>>> {
    let collection = state.db.collection::<ApiToken>("api_tokens");
    let mut cursor = collection
        .find(doc! { "user_id": user.id })
        .await?;

    let mut tokens = Vec::new();
    while cursor.advance().await? {
        let t: ApiToken = cursor.deserialize_current()?;
        tokens.push(TokenResponse {
            id: t.id.to_hex(),
            name: t.name,
            created_at: t.created_at.to_rfc3339(),
        });
    }

    Ok(Json(tokens))
}

pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    let token_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid token ID".to_string()))?;

    let collection = state.db.collection::<ApiToken>("api_tokens");
    let result = collection
        .delete_one(doc! { "_id": token_id, "user_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Token not found".to_string()));
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}
