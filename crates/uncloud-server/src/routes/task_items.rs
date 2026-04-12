use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::{Datelike, NaiveDate, Utc};
use mongodb::bson::oid::ObjectId;
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{
    File, ProjectPermission, RecurrenceRule, Task, TaskComment, TaskProject, TaskStatus, User,
};
use crate::AppState;
use uncloud_common::{
    AttachFilesRequest, CreateTaskCommentRequest, CreateTaskRequest, ReorderTasksRequest,
    TaskCommentResponse, TaskPriority as ApiTaskPriority, TaskResponse, TaskScheduleResponse,
    TaskStatus as ApiTaskStatus, UpdateTaskCommentRequest, UpdateTaskRequest,
    UpdateTaskStatusRequest,
};

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn get_project_permission(
    state: &AppState,
    project_id: ObjectId,
    user_id: ObjectId,
) -> Result<(TaskProject, ProjectPermission)> {
    let coll = state.db.collection::<TaskProject>("task_projects");
    let project = coll
        .find_one(doc! { "_id": project_id })
        .await?
        .ok_or(AppError::NotFound("Project".into()))?;
    if project.owner_id == user_id {
        return Ok((project, ProjectPermission::Admin));
    }
    let perm = project
        .members
        .iter()
        .find(|m| m.user_id == user_id)
        .map(|m| m.permission.clone())
        .ok_or(AppError::Forbidden("Not a member of this project".into()))?;
    Ok((project, perm))
}

fn require_editor(perm: &ProjectPermission) -> Result<()> {
    match perm {
        ProjectPermission::Editor | ProjectPermission::Admin => Ok(()),
        ProjectPermission::Viewer => {
            Err(AppError::Forbidden("Editor permission required".into()))
        }
    }
}

async fn verify_task_access(
    state: &AppState,
    task_id: ObjectId,
    user_id: ObjectId,
) -> Result<(Task, TaskProject, ProjectPermission)> {
    let task_coll = state.db.collection::<Task>("tasks");
    let task = task_coll
        .find_one(doc! { "_id": task_id })
        .await?
        .ok_or(AppError::NotFound("Task".into()))?;
    let (project, perm) = get_project_permission(state, task.project_id, user_id).await?;
    Ok((task, project, perm))
}

fn status_to_model(s: &ApiTaskStatus) -> TaskStatus {
    match s {
        ApiTaskStatus::Todo => TaskStatus::Todo,
        ApiTaskStatus::InProgress => TaskStatus::InProgress,
        ApiTaskStatus::Blocked => TaskStatus::Blocked,
        ApiTaskStatus::Done => TaskStatus::Done,
        ApiTaskStatus::Cancelled => TaskStatus::Cancelled,
    }
}

fn status_to_api(s: &TaskStatus) -> ApiTaskStatus {
    match s {
        TaskStatus::Todo => ApiTaskStatus::Todo,
        TaskStatus::InProgress => ApiTaskStatus::InProgress,
        TaskStatus::Blocked => ApiTaskStatus::Blocked,
        TaskStatus::Done => ApiTaskStatus::Done,
        TaskStatus::Cancelled => ApiTaskStatus::Cancelled,
    }
}

fn priority_to_api(p: &Option<crate::models::TaskPriority>) -> ApiTaskPriority {
    match p {
        Some(crate::models::TaskPriority::High) => ApiTaskPriority::High,
        Some(crate::models::TaskPriority::Medium) => ApiTaskPriority::Medium,
        Some(crate::models::TaskPriority::Low) | None => ApiTaskPriority::Low,
    }
}

fn recurrence_to_api(
    r: &Option<RecurrenceRule>,
) -> Option<uncloud_common::RecurrenceRule> {
    r.as_ref().map(|rule| match rule {
        RecurrenceRule::Daily => uncloud_common::RecurrenceRule::Daily,
        RecurrenceRule::Weekly { days } => uncloud_common::RecurrenceRule::Weekly {
            days: days.clone(),
        },
        RecurrenceRule::Monthly { day_of_month } => uncloud_common::RecurrenceRule::Monthly {
            day_of_month: *day_of_month,
        },
        RecurrenceRule::Yearly { month, day } => uncloud_common::RecurrenceRule::Yearly {
            month: *month,
            day: *day,
        },
        RecurrenceRule::Custom { interval_days } => uncloud_common::RecurrenceRule::Custom {
            interval_days: *interval_days,
        },
    })
}

fn recurrence_from_api(
    r: &Option<uncloud_common::RecurrenceRule>,
) -> Option<RecurrenceRule> {
    r.as_ref().map(|rule| match rule {
        uncloud_common::RecurrenceRule::Daily => RecurrenceRule::Daily,
        uncloud_common::RecurrenceRule::Weekly { days } => RecurrenceRule::Weekly {
            days: days.clone(),
        },
        uncloud_common::RecurrenceRule::Monthly { day_of_month } => RecurrenceRule::Monthly {
            day_of_month: *day_of_month,
        },
        uncloud_common::RecurrenceRule::Yearly { month, day } => RecurrenceRule::Yearly {
            month: *month,
            day: *day,
        },
        uncloud_common::RecurrenceRule::Custom { interval_days } => RecurrenceRule::Custom {
            interval_days: *interval_days,
        },
    })
}

fn task_to_response(
    task: &Task,
    subtask_count: u32,
    subtask_done_count: u32,
    comment_count: u32,
) -> TaskResponse {
    TaskResponse {
        id: task.id.to_hex(),
        project_id: task.project_id.to_hex(),
        section_id: task.section_id.map(|id| id.to_hex()),
        parent_task_id: task.parent_task_id.map(|id| id.to_hex()),
        title: task.title.clone(),
        description: task.description.clone(),
        status: status_to_api(&task.status),
        status_note: task.status_note.clone(),
        priority: priority_to_api(&task.priority),
        assignee_id: task.assignee_id.map(|id| id.to_hex()),
        assignee_username: None, // filled in by caller if needed
        labels: task.labels.clone(),
        due_date: task.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
        recurrence_rule: recurrence_to_api(&task.recurrence_rule),
        position: task.position,
        attachments: task.attachments.iter().map(|id| id.to_hex()).collect(),
        subtask_count,
        subtask_done_count,
        comment_count,
        created_by: task.created_by.to_hex(),
        created_at: task.created_at.to_rfc3339(),
        updated_at: task.updated_at.to_rfc3339(),
        completed_at: task.completed_at.map(|dt| dt.to_rfc3339()),
    }
}

fn comment_to_response(comment: &TaskComment, username: &str) -> TaskCommentResponse {
    TaskCommentResponse {
        id: comment.id.to_hex(),
        task_id: comment.task_id.to_hex(),
        author_id: comment.author_id.to_hex(),
        author_username: username.to_string(),
        body: comment.body.clone(),
        created_at: comment.created_at.to_rfc3339(),
        updated_at: comment
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
    }
}

/// Compute the next due date for a recurring task.
fn compute_next_due_date(rule: &RecurrenceRule, current: NaiveDate) -> NaiveDate {
    match rule {
        RecurrenceRule::Daily => current + chrono::Duration::days(1),
        RecurrenceRule::Weekly { days } => {
            if days.is_empty() {
                return current + chrono::Duration::days(7);
            }
            let current_weekday = current.weekday().num_days_from_monday() as u8; // 0=Mon
            // Find next weekday strictly after current
            let mut sorted = days.clone();
            sorted.sort();
            if let Some(&next_day) = sorted.iter().find(|&&d| d > current_weekday) {
                current + chrono::Duration::days((next_day - current_weekday) as i64)
            } else {
                // Wrap to first day of next week
                let first = sorted[0];
                let days_until = (7 - current_weekday + first) as i64;
                current + chrono::Duration::days(days_until)
            }
        }
        RecurrenceRule::Monthly { day_of_month } => {
            let target_day = *day_of_month as u32;
            let (mut year, mut month) = (current.year(), current.month());
            // Move to next month
            if month == 12 {
                year += 1;
                month = 1;
            } else {
                month += 1;
            }
            // Clamp to month length
            let last_day = last_day_of_month(year, month);
            let day = target_day.min(last_day);
            NaiveDate::from_ymd_opt(year, month, day).unwrap_or(current + chrono::Duration::days(30))
        }
        RecurrenceRule::Yearly { month, day } => {
            let target = NaiveDate::from_ymd_opt(current.year(), *month as u32, *day as u32);
            match target {
                Some(d) if d > current => d,
                _ => {
                    // Next year
                    NaiveDate::from_ymd_opt(current.year() + 1, *month as u32, *day as u32)
                        .unwrap_or(current + chrono::Duration::days(365))
                }
            }
        }
        RecurrenceRule::Custom { interval_days } => {
            current + chrono::Duration::days(*interval_days as i64)
        }
    }
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .map(|d| d.pred_opt().unwrap().day())
    .unwrap_or(28)
}

/// Count subtasks and done-subtasks for a set of parent task IDs.
/// Returns a map of parent_id -> (total, done).
async fn count_subtasks_for(
    state: &AppState,
    parent_ids: &[ObjectId],
) -> Result<std::collections::HashMap<ObjectId, (u32, u32)>> {
    use std::collections::HashMap;
    let mut result: HashMap<ObjectId, (u32, u32)> = HashMap::new();
    if parent_ids.is_empty() {
        return Ok(result);
    }
    let bson_ids: Vec<bson::Bson> = parent_ids
        .iter()
        .map(|id| bson::Bson::ObjectId(*id))
        .collect();
    let task_coll = state.db.collection::<Task>("tasks");
    let mut cursor = task_coll
        .find(doc! { "parent_task_id": { "$in": &bson_ids } })
        .await?;
    while cursor.advance().await? {
        let sub: Task = cursor.deserialize_current()?;
        if let Some(pid) = sub.parent_task_id {
            let entry = result.entry(pid).or_insert((0, 0));
            entry.0 += 1;
            if sub.status == TaskStatus::Done {
                entry.1 += 1;
            }
        }
    }
    Ok(result)
}

/// Count comments for a set of task IDs.
async fn count_comments_for(
    state: &AppState,
    task_ids: &[ObjectId],
) -> Result<std::collections::HashMap<ObjectId, u32>> {
    use std::collections::HashMap;
    let mut result: HashMap<ObjectId, u32> = HashMap::new();
    if task_ids.is_empty() {
        return Ok(result);
    }
    let bson_ids: Vec<bson::Bson> = task_ids
        .iter()
        .map(|id| bson::Bson::ObjectId(*id))
        .collect();
    let coll = state.db.collection::<TaskComment>("task_comments");
    let mut cursor = coll
        .find(doc! { "task_id": { "$in": &bson_ids } })
        .await?;
    while cursor.advance().await? {
        let c: TaskComment = cursor.deserialize_current()?;
        *result.entry(c.task_id).or_insert(0) += 1;
    }
    Ok(result)
}

/// Look up a username by ObjectId (returns "unknown" on miss).
async fn username_for(state: &AppState, user_id: ObjectId) -> String {
    let coll = state.db.collection::<User>("users");
    coll.find_one(doc! { "_id": user_id })
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_else(|| "unknown".to_string())
}

/// Parse an optional NaiveDate from a "YYYY-MM-DD" string.
fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest("Invalid date format, expected YYYY-MM-DD".into()))
}

// ── Query params ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<String>,
    pub section_id: Option<String>,
    pub assignee_id: Option<String>,
    pub parent_task_id: Option<String>,
    /// When true, return all tasks including subtasks (no parent_task_id filter).
    pub include_subtasks: Option<bool>,
}

// ── Task handlers ───────────────────────────────────────────────────────────

/// `GET /projects/{id}/tasks`
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(project_id): Path<String>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<Vec<TaskResponse>>> {
    let project_oid =
        ObjectId::parse_str(&project_id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_project, _perm) = get_project_permission(&state, project_oid, user.id).await?;

    let mut filter = doc! { "project_id": project_oid };

    if let Some(ref status) = query.status {
        filter.insert("status", status.as_str());
    }
    if let Some(ref section_id) = query.section_id {
        let oid = ObjectId::parse_str(section_id)
            .map_err(|_| AppError::BadRequest("Invalid section_id".into()))?;
        filter.insert("section_id", oid);
    }
    if let Some(ref assignee_id) = query.assignee_id {
        let oid = ObjectId::parse_str(assignee_id)
            .map_err(|_| AppError::BadRequest("Invalid assignee_id".into()))?;
        filter.insert("assignee_id", oid);
    }
    if let Some(ref parent_task_id) = query.parent_task_id {
        let oid = ObjectId::parse_str(parent_task_id)
            .map_err(|_| AppError::BadRequest("Invalid parent_task_id".into()))?;
        filter.insert("parent_task_id", oid);
    } else if query.include_subtasks != Some(true) {
        // By default, only return top-level tasks (no parent)
        filter.insert("parent_task_id", bson::Bson::Null);
    }

    let task_coll = state.db.collection::<Task>("tasks");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "position": 1 })
        .build();
    let mut cursor = task_coll.find(filter).with_options(options).await?;

    let mut tasks: Vec<Task> = Vec::new();
    while cursor.advance().await? {
        tasks.push(cursor.deserialize_current()?);
    }

    let task_ids: Vec<ObjectId> = tasks.iter().map(|t| t.id).collect();
    let subtask_counts = count_subtasks_for(&state, &task_ids).await?;
    let comment_counts = count_comments_for(&state, &task_ids).await?;

    let responses: Vec<TaskResponse> = tasks
        .iter()
        .map(|t| {
            let (sc, sdc) = subtask_counts.get(&t.id).copied().unwrap_or((0, 0));
            let cc = comment_counts.get(&t.id).copied().unwrap_or(0);
            task_to_response(t, sc, sdc, cc)
        })
        .collect();

    Ok(Json(responses))
}

/// `POST /projects/{id}/tasks`
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(project_id): Path<String>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskResponse>)> {
    let project_oid =
        ObjectId::parse_str(&project_id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_project, perm) = get_project_permission(&state, project_oid, user.id).await?;
    require_editor(&perm)?;

    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::BadRequest("Task title cannot be empty".into()));
    }

    let section_id = body
        .section_id
        .as_deref()
        .map(|s| ObjectId::parse_str(s).map_err(|_| AppError::BadRequest("Invalid section_id".into())))
        .transpose()?;

    let parent_task_id = body
        .parent_task_id
        .as_deref()
        .map(|s| {
            ObjectId::parse_str(s).map_err(|_| AppError::BadRequest("Invalid parent_task_id".into()))
        })
        .transpose()?;

    let assignee_id = body
        .assignee_id
        .as_deref()
        .map(|s| {
            ObjectId::parse_str(s).map_err(|_| AppError::BadRequest("Invalid assignee_id".into()))
        })
        .transpose()?;

    let status = body
        .status
        .as_ref()
        .map(status_to_model)
        .unwrap_or(TaskStatus::Todo);

    let priority = body.priority.as_ref().map(|p| match p {
        ApiTaskPriority::High => crate::models::TaskPriority::High,
        ApiTaskPriority::Medium => crate::models::TaskPriority::Medium,
        ApiTaskPriority::Low => crate::models::TaskPriority::Low,
    });

    let due_date = body
        .due_date
        .as_deref()
        .map(parse_date)
        .transpose()?;

    let recurrence_rule = recurrence_from_api(&body.recurrence_rule);

    // Auto-assign position if not provided
    let position = if let Some(pos) = body.position {
        pos
    } else {
        let task_coll = state.db.collection::<Task>("tasks");
        let mut pos_filter = doc! { "project_id": project_oid, "status": bson::to_bson(&status).unwrap() };
        if let Some(sid) = section_id {
            pos_filter.insert("section_id", sid);
        }
        let options = mongodb::options::FindOptions::builder()
            .sort(doc! { "position": -1 })
            .limit(1)
            .build();
        let mut cursor = task_coll.find(pos_filter).with_options(options).await?;
        let max_pos = if cursor.advance().await? {
            let t: Task = cursor.deserialize_current()?;
            t.position
        } else {
            -1
        };
        max_pos + 1
    };

    let now = Utc::now();
    let task = Task {
        id: ObjectId::new(),
        project_id: project_oid,
        section_id,
        parent_task_id,
        title,
        description: body.description.clone(),
        status,
        status_note: None,
        priority,
        assignee_id,
        labels: body.labels.clone().unwrap_or_default(),
        due_date,
        recurrence_rule,
        position,
        attachments: Vec::new(),
        created_by: user.id,
        created_at: now,
        updated_at: now,
        completed_at: None,
    };

    let task_coll = state.db.collection::<Task>("tasks");
    task_coll.insert_one(&task).await?;

    let resp = task_to_response(&task, 0, 0, 0);
    Ok((StatusCode::CREATED, Json(resp)))
}

/// `GET /tasks/{id}`
pub async fn get_task(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (task, _project, _perm) = verify_task_access(&state, task_id, user.id).await?;

    let (sc, sdc) = count_subtasks_for(&state, &[task.id])
        .await?
        .get(&task.id)
        .copied()
        .unwrap_or((0, 0));
    let cc = count_comments_for(&state, &[task.id])
        .await?
        .get(&task.id)
        .copied()
        .unwrap_or(0);

    let mut resp = task_to_response(&task, sc, sdc, cc);

    // Fill in assignee username
    if let Some(aid) = task.assignee_id {
        resp.assignee_username = Some(username_for(&state, aid).await);
    }

    Ok(Json(resp))
}

/// `PUT /tasks/{id}`
pub async fn update_task(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskRequest>,
) -> Result<Json<TaskResponse>> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    let mut update_doc = doc! {};
    let now = Utc::now();

    if let Some(ref section_id) = body.section_id {
        let oid = ObjectId::parse_str(section_id)
            .map_err(|_| AppError::BadRequest("Invalid section_id".into()))?;
        update_doc.insert("section_id", oid);
    }
    if let Some(ref title) = body.title {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(AppError::BadRequest("Task title cannot be empty".into()));
        }
        update_doc.insert("title", trimmed);
    }
    if let Some(ref desc) = body.description {
        update_doc.insert("description", desc);
    }
    if let Some(ref status) = body.status {
        let model_status = status_to_model(status);
        update_doc.insert("status", bson::to_bson(&model_status).unwrap());
        // Handle completed_at transitions
        if model_status == TaskStatus::Done && task.status != TaskStatus::Done {
            update_doc.insert("completed_at", bson::DateTime::from_chrono(now));
        } else if model_status != TaskStatus::Done && task.status == TaskStatus::Done {
            update_doc.insert("completed_at", bson::Bson::Null);
        }
    }
    if let Some(ref status_note) = body.status_note {
        update_doc.insert("status_note", status_note);
    }
    if let Some(ref priority) = body.priority {
        let model_prio = match priority {
            ApiTaskPriority::High => crate::models::TaskPriority::High,
            ApiTaskPriority::Medium => crate::models::TaskPriority::Medium,
            ApiTaskPriority::Low => crate::models::TaskPriority::Low,
        };
        update_doc.insert("priority", bson::to_bson(&model_prio).unwrap());
    }
    if let Some(ref assignee_id) = body.assignee_id {
        if assignee_id.is_empty() {
            update_doc.insert("assignee_id", bson::Bson::Null);
        } else {
            let oid = ObjectId::parse_str(assignee_id)
                .map_err(|_| AppError::BadRequest("Invalid assignee_id".into()))?;
            update_doc.insert("assignee_id", oid);
        }
    }
    if let Some(ref labels) = body.labels {
        let bson_labels: Vec<bson::Bson> =
            labels.iter().map(|l| bson::Bson::String(l.clone())).collect();
        update_doc.insert("labels", bson_labels);
    }
    if let Some(ref due_date) = body.due_date {
        if due_date.is_empty() {
            update_doc.insert("due_date", bson::Bson::Null);
        } else {
            let date = parse_date(due_date)?;
            update_doc.insert("due_date", date.format("%Y-%m-%d").to_string());
        }
    }
    if let Some(ref recurrence) = body.recurrence_rule {
        update_doc.insert("recurrence_rule", bson::to_bson(recurrence).unwrap());
    }
    if let Some(pos) = body.position {
        update_doc.insert("position", pos);
    }

    if update_doc.is_empty() {
        // Nothing to update — return current
        let resp = task_to_response(&task, 0, 0, 0);
        return Ok(Json(resp));
    }

    update_doc.insert("updated_at", bson::DateTime::from_chrono(now));

    let task_coll = state.db.collection::<Task>("tasks");
    task_coll
        .update_one(doc! { "_id": task_id }, doc! { "$set": update_doc })
        .await?;

    // Re-fetch
    let updated = task_coll
        .find_one(doc! { "_id": task_id })
        .await?
        .ok_or(AppError::NotFound("Task".into()))?;

    let (sc, sdc) = count_subtasks_for(&state, &[task_id])
        .await?
        .get(&task_id)
        .copied()
        .unwrap_or((0, 0));
    let cc = count_comments_for(&state, &[task_id])
        .await?
        .get(&task_id)
        .copied()
        .unwrap_or(0);

    Ok(Json(task_to_response(&updated, sc, sdc, cc)))
}

/// `DELETE /tasks/{id}`
pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    let task_coll = state.db.collection::<Task>("tasks");
    let comment_coll = state.db.collection::<TaskComment>("task_comments");

    // Find subtask IDs
    let mut subtask_ids: Vec<ObjectId> = Vec::new();
    let mut cursor = task_coll
        .find(doc! { "parent_task_id": task_id })
        .await?;
    while cursor.advance().await? {
        let sub: Task = cursor.deserialize_current()?;
        subtask_ids.push(sub.id);
    }

    // Delete comments on this task and its subtasks
    let mut all_task_ids: Vec<bson::Bson> = vec![bson::Bson::ObjectId(task_id)];
    all_task_ids.extend(subtask_ids.iter().map(|id| bson::Bson::ObjectId(*id)));
    comment_coll
        .delete_many(doc! { "task_id": { "$in": &all_task_ids } })
        .await?;

    // Delete subtasks
    if !subtask_ids.is_empty() {
        let bson_sub_ids: Vec<bson::Bson> = subtask_ids
            .iter()
            .map(|id| bson::Bson::ObjectId(*id))
            .collect();
        task_coll
            .delete_many(doc! { "_id": { "$in": &bson_sub_ids } })
            .await?;
    }

    // Delete the task itself
    task_coll.delete_one(doc! { "_id": task_id }).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /tasks/{id}/status`
pub async fn update_task_status(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskStatusRequest>,
) -> Result<Json<TaskResponse>> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    let new_status = status_to_model(&body.status);
    let now = Utc::now();

    let mut update_doc = doc! {
        "status": bson::to_bson(&new_status).unwrap(),
        "updated_at": bson::DateTime::from_chrono(now),
    };

    if let Some(ref note) = body.status_note {
        update_doc.insert("status_note", note);
    }

    if new_status == TaskStatus::Done && task.status != TaskStatus::Done {
        update_doc.insert("completed_at", bson::DateTime::from_chrono(now));
    } else if new_status != TaskStatus::Done && task.status == TaskStatus::Done {
        update_doc.insert("completed_at", bson::Bson::Null);
    }

    let task_coll = state.db.collection::<Task>("tasks");
    task_coll
        .update_one(doc! { "_id": task_id }, doc! { "$set": &update_doc })
        .await?;

    // Recurring task: if completing and has recurrence_rule, spawn next instance
    if new_status == TaskStatus::Done {
        if let Some(ref rule) = task.recurrence_rule {
            let current_due = task.due_date.unwrap_or_else(|| Utc::now().date_naive());
            let next_due = compute_next_due_date(rule, current_due);
            let next_task = Task {
                id: ObjectId::new(),
                project_id: task.project_id,
                section_id: task.section_id,
                parent_task_id: None,
                title: task.title.clone(),
                description: task.description.clone(),
                status: TaskStatus::Todo,
                status_note: None,
                priority: task.priority.clone(),
                assignee_id: task.assignee_id,
                labels: task.labels.clone(),
                due_date: Some(next_due),
                recurrence_rule: task.recurrence_rule.clone(),
                position: task.position,
                attachments: Vec::new(),
                created_by: user.id,
                created_at: now,
                updated_at: now,
                completed_at: None,
            };
            task_coll.insert_one(&next_task).await?;
        }
    }

    // Re-fetch
    let updated = task_coll
        .find_one(doc! { "_id": task_id })
        .await?
        .ok_or(AppError::NotFound("Task".into()))?;

    let (sc, sdc) = count_subtasks_for(&state, &[task_id])
        .await?
        .get(&task_id)
        .copied()
        .unwrap_or((0, 0));
    let cc = count_comments_for(&state, &[task_id])
        .await?
        .get(&task_id)
        .copied()
        .unwrap_or(0);

    Ok(Json(task_to_response(&updated, sc, sdc, cc)))
}

/// `PUT /projects/{id}/tasks/reorder`
pub async fn reorder_tasks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(project_id): Path<String>,
    Json(body): Json<ReorderTasksRequest>,
) -> Result<StatusCode> {
    let project_oid =
        ObjectId::parse_str(&project_id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_project, perm) = get_project_permission(&state, project_oid, user.id).await?;
    require_editor(&perm)?;

    let task_coll = state.db.collection::<Task>("tasks");
    for (i, tid_str) in body.task_ids.iter().enumerate() {
        let tid = ObjectId::parse_str(tid_str)
            .map_err(|_| AppError::BadRequest(format!("Invalid task ID: {}", tid_str)))?;
        task_coll
            .update_one(
                doc! { "_id": tid, "project_id": project_oid },
                doc! { "$set": { "position": i as i32 } },
            )
            .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /tasks/{id}/subtasks`
pub async fn create_subtask(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskResponse>)> {
    let parent_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (parent, _project, perm) = verify_task_access(&state, parent_id, user.id).await?;
    require_editor(&perm)?;

    // Max 1 level of nesting
    if parent.parent_task_id.is_some() {
        return Err(AppError::BadRequest(
            "Cannot create subtask of a subtask (max 1 level nesting)".into(),
        ));
    }

    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::BadRequest("Task title cannot be empty".into()));
    }

    let assignee_id = body
        .assignee_id
        .as_deref()
        .map(|s| {
            ObjectId::parse_str(s).map_err(|_| AppError::BadRequest("Invalid assignee_id".into()))
        })
        .transpose()?;

    let status = body
        .status
        .as_ref()
        .map(status_to_model)
        .unwrap_or(TaskStatus::Todo);

    let priority = body.priority.as_ref().map(|p| match p {
        ApiTaskPriority::High => crate::models::TaskPriority::High,
        ApiTaskPriority::Medium => crate::models::TaskPriority::Medium,
        ApiTaskPriority::Low => crate::models::TaskPriority::Low,
    });

    let due_date = body
        .due_date
        .as_deref()
        .map(parse_date)
        .transpose()?;

    // Auto-position among siblings
    let task_coll = state.db.collection::<Task>("tasks");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "position": -1 })
        .limit(1)
        .build();
    let mut cursor = task_coll
        .find(doc! { "parent_task_id": parent_id })
        .with_options(options)
        .await?;
    let max_pos = if cursor.advance().await? {
        let t: Task = cursor.deserialize_current()?;
        t.position
    } else {
        -1
    };

    let now = Utc::now();
    let task = Task {
        id: ObjectId::new(),
        project_id: parent.project_id,
        section_id: parent.section_id,
        parent_task_id: Some(parent_id),
        title,
        description: body.description.clone(),
        status,
        status_note: None,
        priority,
        assignee_id,
        labels: body.labels.clone().unwrap_or_default(),
        due_date,
        recurrence_rule: recurrence_from_api(&body.recurrence_rule),
        position: body.position.unwrap_or(max_pos + 1),
        attachments: Vec::new(),
        created_by: user.id,
        created_at: now,
        updated_at: now,
        completed_at: None,
    };

    task_coll.insert_one(&task).await?;

    Ok((StatusCode::CREATED, Json(task_to_response(&task, 0, 0, 0))))
}

/// `POST /tasks/{id}/promote`
pub async fn promote_subtask(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    if task.parent_task_id.is_none() {
        return Err(AppError::BadRequest(
            "Task is not a subtask, cannot promote".into(),
        ));
    }

    let task_coll = state.db.collection::<Task>("tasks");
    task_coll
        .update_one(
            doc! { "_id": task_id },
            doc! { "$set": { "parent_task_id": bson::Bson::Null, "updated_at": bson::DateTime::from_chrono(Utc::now()) } },
        )
        .await?;

    let updated = task_coll
        .find_one(doc! { "_id": task_id })
        .await?
        .ok_or(AppError::NotFound("Task".into()))?;

    Ok(Json(task_to_response(&updated, 0, 0, 0)))
}

/// `POST /tasks/{id}/attachments`
pub async fn attach_files(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AttachFilesRequest>,
) -> Result<StatusCode> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    let files_coll = state.db.collection::<File>("files");
    let mut new_ids: Vec<bson::Bson> = Vec::new();

    for fid_str in &body.file_ids {
        let file_id = ObjectId::parse_str(fid_str)
            .map_err(|_| AppError::BadRequest(format!("Invalid file ID: {}", fid_str)))?;
        let exists = files_coll
            .find_one(doc! { "_id": file_id, "owner_id": user.id, "deleted_at": bson::Bson::Null })
            .await?;
        if exists.is_none() {
            return Err(AppError::NotFound(format!("File {}", fid_str)));
        }
        new_ids.push(bson::Bson::ObjectId(file_id));
    }

    if !new_ids.is_empty() {
        let task_coll = state.db.collection::<Task>("tasks");
        task_coll
            .update_one(
                doc! { "_id": task_id },
                doc! {
                    "$addToSet": { "attachments": { "$each": &new_ids } },
                    "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) }
                },
            )
            .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /tasks/{id}/attachments/{file_id}`
pub async fn detach_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, file_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid task ID".into()))?;
    let file_oid = ObjectId::parse_str(&file_id)
        .map_err(|_| AppError::BadRequest("Invalid file ID".into()))?;

    let (_task, _project, perm) = verify_task_access(&state, task_id, user.id).await?;
    require_editor(&perm)?;

    let task_coll = state.db.collection::<Task>("tasks");
    task_coll
        .update_one(
            doc! { "_id": task_id },
            doc! {
                "$pull": { "attachments": file_oid },
                "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) }
            },
        )
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Schedule handlers ───────────────────────────────────────────────────────

/// `GET /tasks/schedule`
pub async fn get_schedule(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<TaskScheduleResponse>> {
    // Find all projects the user is a member of (or owns)
    let project_coll = state.db.collection::<TaskProject>("task_projects");
    let mut project_cursor = project_coll
        .find(doc! {
            "$or": [
                { "owner_id": user.id },
                { "members.user_id": user.id }
            ]
        })
        .await?;

    let mut project_ids: Vec<bson::Bson> = Vec::new();
    while project_cursor.advance().await? {
        let p: TaskProject = project_cursor.deserialize_current()?;
        project_ids.push(bson::Bson::ObjectId(p.id));
    }

    if project_ids.is_empty() {
        return Ok(Json(TaskScheduleResponse {
            overdue: Vec::new(),
            today: Vec::new(),
            tomorrow: Vec::new(),
            next_7_days: Vec::new(),
            later: Vec::new(),
        }));
    }

    let task_coll = state.db.collection::<Task>("tasks");
    let mut cursor = task_coll
        .find(doc! {
            "project_id": { "$in": &project_ids },
            "due_date": { "$ne": bson::Bson::Null },
            "status": { "$nin": ["done", "cancelled"] },
        })
        .await?;

    let mut tasks: Vec<Task> = Vec::new();
    while cursor.advance().await? {
        tasks.push(cursor.deserialize_current()?);
    }

    let today = Utc::now().date_naive();
    let tomorrow = today + chrono::Duration::days(1);
    let week_end = today + chrono::Duration::days(7);

    let mut overdue = Vec::new();
    let mut today_tasks = Vec::new();
    let mut tomorrow_tasks = Vec::new();
    let mut next_7 = Vec::new();
    let mut later = Vec::new();

    for task in &tasks {
        if let Some(due) = task.due_date {
            let resp = task_to_response(task, 0, 0, 0);
            if due < today {
                overdue.push(resp);
            } else if due == today {
                today_tasks.push(resp);
            } else if due == tomorrow {
                tomorrow_tasks.push(resp);
            } else if due <= week_end {
                next_7.push(resp);
            } else {
                later.push(resp);
            }
        }
    }

    Ok(Json(TaskScheduleResponse {
        overdue,
        today: today_tasks,
        tomorrow: tomorrow_tasks,
        next_7_days: next_7,
        later,
    }))
}

/// `GET /tasks/assigned-to-me`
pub async fn get_assigned_to_me(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<TaskResponse>>> {
    let task_coll = state.db.collection::<Task>("tasks");
    let mut cursor = task_coll
        .find(doc! {
            "assignee_id": user.id,
            "status": { "$nin": ["done", "cancelled"] },
        })
        .await?;

    let mut tasks: Vec<Task> = Vec::new();
    while cursor.advance().await? {
        tasks.push(cursor.deserialize_current()?);
    }

    // Verify user is still a member of each task's project (filter out stale)
    let mut responses: Vec<TaskResponse> = Vec::new();
    for task in &tasks {
        if get_project_permission(&state, task.project_id, user.id)
            .await
            .is_ok()
        {
            responses.push(task_to_response(task, 0, 0, 0));
        }
    }

    Ok(Json(responses))
}

// ── Comment handlers ────────────────────────────────────────────────────────

/// `GET /tasks/{id}/comments`
pub async fn list_comments(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<TaskCommentResponse>>> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (_task, _project, _perm) = verify_task_access(&state, task_id, user.id).await?;

    let coll = state.db.collection::<TaskComment>("task_comments");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "created_at": 1 })
        .build();
    let mut cursor = coll
        .find(doc! { "task_id": task_id })
        .with_options(options)
        .await?;

    let mut comments: Vec<TaskComment> = Vec::new();
    while cursor.advance().await? {
        comments.push(cursor.deserialize_current()?);
    }

    // Batch-fetch usernames
    let author_ids: Vec<ObjectId> = comments
        .iter()
        .map(|c| c.author_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let mut usernames: std::collections::HashMap<ObjectId, String> =
        std::collections::HashMap::new();
    if !author_ids.is_empty() {
        let bson_ids: Vec<bson::Bson> = author_ids
            .iter()
            .map(|id| bson::Bson::ObjectId(*id))
            .collect();
        let user_coll = state.db.collection::<User>("users");
        let mut user_cursor = user_coll
            .find(doc! { "_id": { "$in": &bson_ids } })
            .await?;
        while user_cursor.advance().await? {
            let u: User = user_cursor.deserialize_current()?;
            usernames.insert(u.id, u.username);
        }
    }

    let responses: Vec<TaskCommentResponse> = comments
        .iter()
        .map(|c| {
            let uname = usernames
                .get(&c.author_id)
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            comment_to_response(c, uname)
        })
        .collect();

    Ok(Json(responses))
}

/// `POST /tasks/{id}/comments`
pub async fn create_comment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateTaskCommentRequest>,
) -> Result<(StatusCode, Json<TaskCommentResponse>)> {
    let task_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;
    let (task, _project, _perm) = verify_task_access(&state, task_id, user.id).await?;
    // Even Viewers can comment — no require_editor check

    let body_text = body.body.trim().to_string();
    if body_text.is_empty() {
        return Err(AppError::BadRequest("Comment body cannot be empty".into()));
    }

    let now = Utc::now();
    let comment = TaskComment {
        id: ObjectId::new(),
        task_id,
        project_id: task.project_id,
        author_id: user.id,
        body: body_text,
        created_at: now,
        updated_at: None,
    };

    let coll = state.db.collection::<TaskComment>("task_comments");
    coll.insert_one(&comment).await?;

    let resp = comment_to_response(&comment, &user.username);
    Ok((StatusCode::CREATED, Json(resp)))
}

/// `PUT /tasks/comments/{id}`
pub async fn update_comment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskCommentRequest>,
) -> Result<Json<TaskCommentResponse>> {
    let comment_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskComment>("task_comments");
    let comment = coll
        .find_one(doc! { "_id": comment_id })
        .await?
        .ok_or(AppError::NotFound("Comment".into()))?;

    if comment.author_id != user.id {
        return Err(AppError::Forbidden(
            "Only the author can edit this comment".into(),
        ));
    }

    let body_text = body.body.trim().to_string();
    if body_text.is_empty() {
        return Err(AppError::BadRequest("Comment body cannot be empty".into()));
    }

    let now = Utc::now();
    coll.update_one(
        doc! { "_id": comment_id },
        doc! { "$set": { "body": &body_text, "updated_at": bson::DateTime::from_chrono(now) } },
    )
    .await?;

    let updated = coll
        .find_one(doc! { "_id": comment_id })
        .await?
        .ok_or(AppError::NotFound("Comment".into()))?;

    Ok(Json(comment_to_response(&updated, &user.username)))
}

/// `DELETE /tasks/comments/{id}`
pub async fn delete_comment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let comment_id =
        ObjectId::parse_str(&id).map_err(|_| AppError::BadRequest("Invalid ID".into()))?;

    let coll = state.db.collection::<TaskComment>("task_comments");
    let comment = coll
        .find_one(doc! { "_id": comment_id })
        .await?
        .ok_or(AppError::NotFound("Comment".into()))?;

    // Author can always delete their own comment
    if comment.author_id != user.id {
        // Check if user is Admin of the project
        let (_project, perm) =
            get_project_permission(&state, comment.project_id, user.id).await?;
        if perm != ProjectPermission::Admin {
            return Err(AppError::Forbidden(
                "Only the author or a project admin can delete this comment".into(),
            ));
        }
    }

    coll.delete_one(doc! { "_id": comment_id }).await?;

    Ok(StatusCode::NO_CONTENT)
}
