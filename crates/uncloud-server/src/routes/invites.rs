use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{User, UserRole};
use crate::AppState;

/// Convert the common API UserRole to the server model UserRole.
fn to_model_role(role: uncloud_common::UserRole) -> UserRole {
    match role {
        uncloud_common::UserRole::Admin => UserRole::Admin,
        uncloud_common::UserRole::User => UserRole::User,
    }
}

fn to_api_role(role: UserRole) -> uncloud_common::UserRole {
    match role {
        UserRole::Admin => uncloud_common::UserRole::Admin,
        UserRole::User => uncloud_common::UserRole::User,
    }
}

pub async fn create_invite(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<uncloud_common::CreateInviteRequest>,
) -> Result<(StatusCode, Json<uncloud_common::InviteResponse>)> {
    let invite = state
        .auth
        .create_invite(user.id, req.comment, req.role.map(to_model_role), req.expires_in_hours)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(uncloud_common::InviteResponse {
            id: invite.id.to_hex(),
            token: invite.token,
            comment: invite.comment,
            role: invite.role.map(to_api_role),
            expires_at: invite.expires_at.map(|d| d.to_rfc3339()),
            used: invite.used_by.is_some(),
            used_by_username: None,
            used_by_email: None,
            created_at: invite.created_at.to_rfc3339(),
        }),
    ))
}

pub async fn list_invites(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<uncloud_common::InviteResponse>>> {
    let invites = state.auth.list_invites().await?;

    // Resolve used_by user IDs to usernames/emails
    let used_ids: Vec<ObjectId> = invites
        .iter()
        .filter_map(|inv| inv.used_by)
        .collect();

    let users_coll = state.db.collection::<User>("users");
    let mut user_map = std::collections::HashMap::new();
    if !used_ids.is_empty() {
        let ids_bson: Vec<mongodb::bson::Bson> = used_ids
            .iter()
            .map(|id| mongodb::bson::Bson::ObjectId(*id))
            .collect();
        let mut cursor = users_coll
            .find(doc! { "_id": { "$in": ids_bson } })
            .await?;
        while cursor.advance().await? {
            let user: User = cursor.deserialize_current()?;
            user_map.insert(user.id, (user.username, user.email));
        }
    }

    let responses: Vec<_> = invites
        .into_iter()
        .map(|inv| {
            let (used_by_username, used_by_email) = inv
                .used_by
                .and_then(|uid| user_map.get(&uid))
                .map(|(name, email)| (Some(name.clone()), email.clone()))
                .unwrap_or((None, None));

            uncloud_common::InviteResponse {
                id: inv.id.to_hex(),
                token: inv.token,
                comment: inv.comment.or(inv.email),
                role: inv.role.map(to_api_role),
                expires_at: inv.expires_at.map(|d| d.to_rfc3339()),
                used: inv.used_by.is_some(),
                used_by_username,
                used_by_email,
                created_at: inv.created_at.to_rfc3339(),
            }
        })
        .collect();
    Ok(Json(responses))
}

pub async fn delete_invite(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let invite_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid invite ID".to_string()))?;
    state.auth.delete_invite(invite_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
