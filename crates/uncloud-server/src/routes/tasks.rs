use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{
    ProjectMember, ProjectPermission, ProjectView, TaskComment, TaskLabel, TaskProject, TaskSection,
    Task, User,
};
use crate::AppState;
use uncloud_common::{
    AddProjectMemberRequest, CreateTaskLabelRequest, CreateTaskProjectRequest,
    CreateTaskSectionRequest, ProjectMemberResponse, ReorderSectionsRequest, TaskLabelResponse,
    TaskProjectResponse, TaskSectionResponse, UpdateProjectMemberRequest, UpdateTaskLabelRequest,
    UpdateTaskProjectRequest, UpdateTaskSectionRequest,
    ProjectPermission as ApiProjectPermission, ProjectView as ApiProjectView,
};

// ── Helpers ────────────────────────────────────────────────────────────

/// Load a project and verify the user has access. Returns the project and the
/// user's effective permission level.
async fn verify_project_access(
    state: &AppState,
    project_id: ObjectId,
    user_id: ObjectId,
) -> Result<(TaskProject, ProjectPermission)> {
    let coll = state.db.collection::<TaskProject>("task_projects");
    let project = coll
        .find_one(doc! { "_id": project_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Project".to_string()))?;

    if project.owner_id == user_id {
        return Ok((project, ProjectPermission::Admin));
    }

    let permission = project
        .members
        .iter()
        .find(|m| m.user_id == user_id)
        .map(|m| m.permission.clone())
        .ok_or_else(|| AppError::Forbidden("Not a member of this project".to_string()))?;

    Ok((project, permission))
}

fn require_editor(permission: &ProjectPermission) -> Result<()> {
    if *permission == ProjectPermission::Viewer {
        return Err(AppError::Forbidden(
            "Editor or Admin permission required".to_string(),
        ));
    }
    Ok(())
}

fn require_admin(permission: &ProjectPermission) -> Result<()> {
    if *permission != ProjectPermission::Admin {
        return Err(AppError::Forbidden(
            "Admin permission required".to_string(),
        ));
    }
    Ok(())
}

// ── Response conversion ────────────────────────────────────────────────

fn permission_to_api(p: &ProjectPermission) -> ApiProjectPermission {
    match p {
        ProjectPermission::Viewer => ApiProjectPermission::Viewer,
        ProjectPermission::Editor => ApiProjectPermission::Editor,
        ProjectPermission::Admin => ApiProjectPermission::Admin,
    }
}

fn view_to_api(v: &ProjectView) -> ApiProjectView {
    match v {
        ProjectView::Board => ApiProjectView::Board,
        ProjectView::List => ApiProjectView::List,
    }
}

fn project_to_response(project: &TaskProject, owner_username: &str) -> TaskProjectResponse {
    TaskProjectResponse {
        id: project.id.to_hex(),
        name: project.name.clone(),
        description: project.description.clone(),
        color: Some(project.color.clone()),
        icon: project.icon.clone(),
        owner_id: project.owner_id.to_hex(),
        owner_username: owner_username.to_string(),
        members: project
            .members
            .iter()
            .map(|m| ProjectMemberResponse {
                user_id: m.user_id.to_hex(),
                username: m.username.clone(),
                permission: permission_to_api(&m.permission),
                added_at: m.added_at.to_rfc3339(),
            })
            .collect(),
        default_view: view_to_api(&project.default_view),
        archived: project.archived,
        created_at: project.created_at.to_rfc3339(),
        updated_at: project.updated_at.to_rfc3339(),
    }
}

fn section_to_response(section: &TaskSection) -> TaskSectionResponse {
    TaskSectionResponse {
        id: section.id.to_hex(),
        project_id: section.project_id.to_hex(),
        name: section.name.clone(),
        position: section.position,
        collapsed: section.collapsed,
    }
}

fn label_to_response(label: &TaskLabel) -> TaskLabelResponse {
    TaskLabelResponse {
        id: label.id.to_hex(),
        project_id: label.project_id.to_hex(),
        name: label.name.clone(),
        color: label.color.clone(),
    }
}

/// Helper to look up a user's username by ObjectId.
async fn get_username(state: &AppState, user_id: ObjectId) -> Result<String> {
    let users = state.db.collection::<User>("users");
    let user = users
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User".to_string()))?;
    Ok(user.username)
}

// ── Project handlers ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListProjectsQuery {
    #[serde(default)]
    pub archived: Option<bool>,
}

/// `GET /api/tasks/projects`
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(query): Query<ListProjectsQuery>,
) -> Result<Json<Vec<TaskProjectResponse>>> {
    let coll = state.db.collection::<TaskProject>("task_projects");

    let mut filter = doc! {
        "$or": [
            { "owner_id": user.id },
            { "members.user_id": user.id },
        ]
    };

    // Exclude archived by default
    if query.archived != Some(true) {
        filter.insert("archived", doc! { "$ne": true });
    }

    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "updated_at": -1 })
        .build();

    let mut cursor = coll.find(filter).with_options(options).await?;
    let mut projects: Vec<TaskProject> = Vec::new();
    while cursor.advance().await? {
        projects.push(cursor.deserialize_current()?);
    }

    // Batch-fetch owner usernames
    let owner_ids: Vec<ObjectId> = projects
        .iter()
        .map(|p| p.owner_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let users_coll = state.db.collection::<User>("users");
    let mut username_map = std::collections::HashMap::new();
    if !owner_ids.is_empty() {
        let bson_ids: Vec<bson::Bson> = owner_ids.iter().map(|id| bson::Bson::ObjectId(*id)).collect();
        let mut user_cursor = users_coll
            .find(doc! { "_id": { "$in": &bson_ids } })
            .await?;
        while user_cursor.advance().await? {
            let u: User = user_cursor.deserialize_current()?;
            username_map.insert(u.id, u.username);
        }
    }

    let responses: Vec<TaskProjectResponse> = projects
        .iter()
        .map(|p| {
            let owner_name = username_map
                .get(&p.owner_id)
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            project_to_response(p, owner_name)
        })
        .collect();

    Ok(Json(responses))
}

/// `POST /api/tasks/projects`
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateTaskProjectRequest>,
) -> Result<(StatusCode, Json<TaskProjectResponse>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Project name cannot be empty".to_string(),
        ));
    }

    let now = Utc::now();
    let color = body.color.unwrap_or_else(|| "#3B82F6".to_string());
    let default_view = match body.default_view {
        Some(ApiProjectView::List) => ProjectView::List,
        _ => ProjectView::Board,
    };

    let owner_member = ProjectMember {
        user_id: user.id,
        username: user.username.clone(),
        permission: ProjectPermission::Admin,
        added_at: now,
    };

    let project = TaskProject {
        id: ObjectId::new(),
        name,
        description: body.description,
        color,
        icon: body.icon,
        owner_id: user.id,
        members: vec![owner_member],
        default_view,
        archived: false,
        created_at: now,
        updated_at: now,
    };

    let coll = state.db.collection::<TaskProject>("task_projects");
    coll.insert_one(&project).await?;

    let response = project_to_response(&project, &user.username);
    Ok((StatusCode::CREATED, Json(response)))
}

/// `GET /api/tasks/projects/{id}`
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<TaskProjectResponse>> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (project, _perm) = verify_project_access(&state, project_id, user.id).await?;
    let owner_name = get_username(&state, project.owner_id).await?;

    Ok(Json(project_to_response(&project, &owner_name)))
}

/// `PUT /api/tasks/projects/{id}`
pub async fn update_project(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskProjectRequest>,
) -> Result<Json<TaskProjectResponse>> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_admin(&perm)?;

    let mut update_doc = doc! {};

    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Project name cannot be empty".to_string(),
            ));
        }
        update_doc.insert("name", name);
    }
    if let Some(ref desc) = body.description {
        update_doc.insert("description", desc);
    }
    if let Some(ref color) = body.color {
        update_doc.insert("color", color);
    }
    if let Some(ref icon) = body.icon {
        update_doc.insert("icon", icon);
    }
    if let Some(ref view) = body.default_view {
        let v = match view {
            ApiProjectView::Board => "board",
            ApiProjectView::List => "list",
        };
        update_doc.insert("default_view", v);
    }
    if let Some(archived) = body.archived {
        update_doc.insert("archived", archived);
    }

    if !update_doc.is_empty() {
        update_doc.insert("updated_at", bson::DateTime::from_chrono(Utc::now()));

        let coll = state.db.collection::<TaskProject>("task_projects");
        coll.update_one(
            doc! { "_id": project_id },
            doc! { "$set": update_doc },
        )
        .await?;
    }

    // Re-fetch for response
    let coll = state.db.collection::<TaskProject>("task_projects");
    let updated = coll
        .find_one(doc! { "_id": project_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Project".to_string()))?;

    let owner_name = get_username(&state, updated.owner_id).await?;
    Ok(Json(project_to_response(&updated, &owner_name)))
}

/// `DELETE /api/tasks/projects/{id}`
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (project, _perm) = verify_project_access(&state, project_id, user.id).await?;

    // Only the owner can delete a project
    if project.owner_id != user.id {
        return Err(AppError::Forbidden(
            "Only the project owner can delete it".to_string(),
        ));
    }

    // Delete all related data
    let sections_coll = state.db.collection::<TaskSection>("task_sections");
    sections_coll
        .delete_many(doc! { "project_id": project_id })
        .await?;

    let tasks_coll = state.db.collection::<Task>("tasks");
    tasks_coll
        .delete_many(doc! { "project_id": project_id })
        .await?;

    let comments_coll = state.db.collection::<TaskComment>("task_comments");
    comments_coll
        .delete_many(doc! { "project_id": project_id })
        .await?;

    let labels_coll = state.db.collection::<TaskLabel>("task_labels");
    labels_coll
        .delete_many(doc! { "project_id": project_id })
        .await?;

    let projects_coll = state.db.collection::<TaskProject>("task_projects");
    let result = projects_coll
        .delete_one(doc! { "_id": project_id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Project".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/tasks/projects/{id}/members`
pub async fn add_project_member(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddProjectMemberRequest>,
) -> Result<(StatusCode, Json<ProjectMemberResponse>)> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_admin(&perm)?;

    let member_user_id = ObjectId::parse_str(&body.user_id)
        .map_err(|_| AppError::BadRequest("Invalid user ID".into()))?;

    // Look up the user to get their username
    let users_coll = state.db.collection::<User>("users");
    let member_user = users_coll
        .find_one(doc! { "_id": member_user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User".to_string()))?;

    // Check if already a member
    let coll = state.db.collection::<TaskProject>("task_projects");
    let existing = coll
        .find_one(doc! {
            "_id": project_id,
            "members.user_id": member_user_id,
        })
        .await?;
    if existing.is_some() {
        return Err(AppError::Conflict(
            "User is already a member of this project".to_string(),
        ));
    }

    let now = Utc::now();
    let model_permission = match body.permission {
        ApiProjectPermission::Viewer => ProjectPermission::Viewer,
        ApiProjectPermission::Editor => ProjectPermission::Editor,
        ApiProjectPermission::Admin => ProjectPermission::Admin,
    };

    let member = ProjectMember {
        user_id: member_user_id,
        username: member_user.username.clone(),
        permission: model_permission.clone(),
        added_at: now,
    };

    let member_bson = bson::to_bson(&member)
        .map_err(|e| AppError::Internal(format!("Failed to serialize member: {}", e)))?;

    coll.update_one(
        doc! { "_id": project_id },
        doc! {
            "$push": { "members": member_bson },
            "$set": { "updated_at": bson::DateTime::from_chrono(now) }
        },
    )
    .await?;

    let response = ProjectMemberResponse {
        user_id: member_user_id.to_hex(),
        username: member_user.username,
        permission: permission_to_api(&model_permission),
        added_at: now.to_rfc3339(),
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// `PUT /api/tasks/projects/{id}/members/{user_id}`
pub async fn update_project_member(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, member_user_id_str)): Path<(String, String)>,
    Json(body): Json<UpdateProjectMemberRequest>,
) -> Result<StatusCode> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let member_user_id = ObjectId::parse_str(&member_user_id_str)
        .map_err(|_| AppError::BadRequest("Invalid user ID".into()))?;

    let (_project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_admin(&perm)?;

    let permission_str = match body.permission {
        ApiProjectPermission::Viewer => "viewer",
        ApiProjectPermission::Editor => "editor",
        ApiProjectPermission::Admin => "admin",
    };

    let coll = state.db.collection::<TaskProject>("task_projects");
    let result = coll
        .update_one(
            doc! { "_id": project_id, "members.user_id": member_user_id },
            doc! {
                "$set": {
                    "members.$.permission": permission_str,
                    "updated_at": bson::DateTime::from_chrono(Utc::now()),
                }
            },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Project member".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/tasks/projects/{id}/members/{user_id}`
pub async fn remove_project_member(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, member_user_id_str)): Path<(String, String)>,
) -> Result<StatusCode> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let member_user_id = ObjectId::parse_str(&member_user_id_str)
        .map_err(|_| AppError::BadRequest("Invalid user ID".into()))?;

    let (project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_admin(&perm)?;

    // Cannot remove the owner
    if member_user_id == project.owner_id {
        return Err(AppError::BadRequest(
            "Cannot remove the project owner".to_string(),
        ));
    }

    let coll = state.db.collection::<TaskProject>("task_projects");
    let result = coll
        .update_one(
            doc! { "_id": project_id },
            doc! {
                "$pull": { "members": { "user_id": member_user_id } },
                "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) }
            },
        )
        .await?;

    if result.modified_count == 0 {
        return Err(AppError::NotFound("Project member".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Section handlers ───────────────────────────────────────────────────

/// `GET /api/tasks/projects/{id}/sections`
pub async fn list_sections(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<TaskSectionResponse>>> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, _perm) = verify_project_access(&state, project_id, user.id).await?;

    let coll = state.db.collection::<TaskSection>("task_sections");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "position": 1 })
        .build();

    let mut cursor = coll
        .find(doc! { "project_id": project_id })
        .with_options(options)
        .await?;

    let mut sections: Vec<TaskSectionResponse> = Vec::new();
    while cursor.advance().await? {
        let section: TaskSection = cursor.deserialize_current()?;
        sections.push(section_to_response(&section));
    }

    Ok(Json(sections))
}

/// `POST /api/tasks/projects/{id}/sections`
pub async fn create_section(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateTaskSectionRequest>,
) -> Result<(StatusCode, Json<TaskSectionResponse>)> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_editor(&perm)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Section name cannot be empty".to_string(),
        ));
    }

    let coll = state.db.collection::<TaskSection>("task_sections");

    // Auto-assign position: max existing + 1
    let position = if let Some(pos) = body.position {
        pos
    } else {
        let options = mongodb::options::FindOneOptions::builder()
            .sort(doc! { "position": -1 })
            .build();
        let last = coll
            .find_one(doc! { "project_id": project_id })
            .with_options(options)
            .await?;
        last.map(|s| s.position + 1).unwrap_or(0)
    };

    let section = TaskSection {
        id: ObjectId::new(),
        project_id,
        name,
        position,
        collapsed: false,
        created_at: Utc::now(),
    };

    coll.insert_one(&section).await?;

    Ok((StatusCode::CREATED, Json(section_to_response(&section))))
}

/// `PUT /api/tasks/sections/{id}`
pub async fn update_section(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskSectionRequest>,
) -> Result<Json<TaskSectionResponse>> {
    let section_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskSection>("task_sections");
    let section = coll
        .find_one(doc! { "_id": section_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Section".to_string()))?;

    let (_project, perm) = verify_project_access(&state, section.project_id, user.id).await?;
    require_editor(&perm)?;

    let mut update_doc = doc! {};

    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Section name cannot be empty".to_string(),
            ));
        }
        update_doc.insert("name", name);
    }
    if let Some(position) = body.position {
        update_doc.insert("position", position);
    }
    if let Some(collapsed) = body.collapsed {
        update_doc.insert("collapsed", collapsed);
    }

    if !update_doc.is_empty() {
        coll.update_one(
            doc! { "_id": section_id },
            doc! { "$set": update_doc },
        )
        .await?;
    }

    // Re-fetch
    let updated = coll
        .find_one(doc! { "_id": section_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Section".to_string()))?;

    Ok(Json(section_to_response(&updated)))
}

/// `DELETE /api/tasks/sections/{id}`
pub async fn delete_section(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let section_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskSection>("task_sections");
    let section = coll
        .find_one(doc! { "_id": section_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Section".to_string()))?;

    let (_project, perm) = verify_project_access(&state, section.project_id, user.id).await?;
    require_editor(&perm)?;

    // Null out section_id on tasks in this section
    let tasks_coll = state.db.collection::<Task>("tasks");
    tasks_coll
        .update_many(
            doc! { "section_id": section_id },
            doc! { "$set": { "section_id": bson::Bson::Null } },
        )
        .await?;

    coll.delete_one(doc! { "_id": section_id }).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/tasks/projects/{id}/sections/reorder`
pub async fn reorder_sections(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ReorderSectionsRequest>,
) -> Result<StatusCode> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_editor(&perm)?;

    let coll = state.db.collection::<TaskSection>("task_sections");

    for (i, sid_str) in body.section_ids.iter().enumerate() {
        let section_id = ObjectId::parse_str(sid_str)
            .map_err(|_| AppError::BadRequest(format!("Invalid section ID: {}", sid_str)))?;

        coll.update_one(
            doc! { "_id": section_id, "project_id": project_id },
            doc! { "$set": { "position": i as i32 } },
        )
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Label handlers ─────────────────────────────────────────────────────

/// `GET /api/tasks/projects/{id}/labels`
pub async fn list_labels(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<TaskLabelResponse>>> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, _perm) = verify_project_access(&state, project_id, user.id).await?;

    let coll = state.db.collection::<TaskLabel>("task_labels");
    let mut cursor = coll.find(doc! { "project_id": project_id }).await?;

    let mut labels: Vec<TaskLabelResponse> = Vec::new();
    while cursor.advance().await? {
        let label: TaskLabel = cursor.deserialize_current()?;
        labels.push(label_to_response(&label));
    }

    Ok(Json(labels))
}

/// `POST /api/tasks/projects/{id}/labels`
pub async fn create_label(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateTaskLabelRequest>,
) -> Result<(StatusCode, Json<TaskLabelResponse>)> {
    let project_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let (_project, perm) = verify_project_access(&state, project_id, user.id).await?;
    require_editor(&perm)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Label name cannot be empty".to_string(),
        ));
    }

    // Check uniqueness within project
    let coll = state.db.collection::<TaskLabel>("task_labels");
    let existing = coll
        .find_one(doc! { "project_id": project_id, "name": &name })
        .await?;
    if existing.is_some() {
        return Err(AppError::Conflict(format!(
            "A label named \"{}\" already exists in this project",
            name
        )));
    }

    let label = TaskLabel {
        id: ObjectId::new(),
        project_id,
        name,
        color: body.color,
    };

    coll.insert_one(&label).await?;

    Ok((StatusCode::CREATED, Json(label_to_response(&label))))
}

/// `PUT /api/tasks/labels/{id}`
pub async fn update_label(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskLabelRequest>,
) -> Result<Json<TaskLabelResponse>> {
    let label_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskLabel>("task_labels");
    let label = coll
        .find_one(doc! { "_id": label_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Label".to_string()))?;

    let (_project, perm) = verify_project_access(&state, label.project_id, user.id).await?;
    require_editor(&perm)?;

    let mut update_doc = doc! {};
    let old_name = label.name.clone();

    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Label name cannot be empty".to_string(),
            ));
        }
        // Check uniqueness if name is changing
        if name != old_name {
            let existing = coll
                .find_one(doc! {
                    "project_id": label.project_id,
                    "name": name,
                    "_id": { "$ne": label_id },
                })
                .await?;
            if existing.is_some() {
                return Err(AppError::Conflict(format!(
                    "A label named \"{}\" already exists in this project",
                    name
                )));
            }
        }
        update_doc.insert("name", name);
    }
    if let Some(ref color) = body.color {
        update_doc.insert("color", color);
    }

    if !update_doc.is_empty() {
        coll.update_one(
            doc! { "_id": label_id },
            doc! { "$set": update_doc.clone() },
        )
        .await?;

        // If the name changed, update references in tasks
        if let Some(new_name) = update_doc.get_str("name").ok() {
            if new_name != old_name {
                let tasks_coll = state.db.collection::<Task>("tasks");
                // Replace old label name with new in tasks that have it
                tasks_coll
                    .update_many(
                        doc! { "project_id": label.project_id, "labels": &old_name },
                        doc! { "$set": { "labels.$": new_name } },
                    )
                    .await?;
            }
        }
    }

    // Re-fetch
    let updated = coll
        .find_one(doc! { "_id": label_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Label".to_string()))?;

    Ok(Json(label_to_response(&updated)))
}

/// `DELETE /api/tasks/labels/{id}`
pub async fn delete_label(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let label_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskLabel>("task_labels");
    let label = coll
        .find_one(doc! { "_id": label_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Label".to_string()))?;

    let (_project, perm) = verify_project_access(&state, label.project_id, user.id).await?;
    require_editor(&perm)?;

    // Remove label name from all tasks in the project
    let tasks_coll = state.db.collection::<Task>("tasks");
    tasks_coll
        .update_many(
            doc! { "project_id": label.project_id, "labels": &label.name },
            doc! { "$pull": { "labels": &label.name } },
        )
        .await?;

    coll.delete_one(doc! { "_id": label_id }).await?;

    Ok(StatusCode::NO_CONTENT)
}
