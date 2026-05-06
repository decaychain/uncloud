use uncloud_common::{
    TaskProjectResponse, TaskSectionResponse, TaskResponse, TaskCommentResponse,
    TaskLabelResponse, TaskScheduleResponse,
    CreateTaskProjectRequest, UpdateTaskProjectRequest,
    AddProjectMemberRequest, UpdateProjectMemberRequest,
    CreateTaskSectionRequest, UpdateTaskSectionRequest, ReorderSectionsRequest,
    CreateTaskRequest, UpdateTaskRequest, UpdateTaskStatusRequest, ReorderTasksRequest,
    CreateTaskCommentRequest, UpdateTaskCommentRequest,
    CreateTaskLabelRequest, UpdateTaskLabelRequest,
    AttachFilesRequest,
};

use super::api;

// ── Projects ──

pub async fn list_projects() -> Result<Vec<TaskProjectResponse>, String> {
    let response = api::get("/tasks/projects")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskProjectResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load projects".to_string())
    }
}

pub async fn create_project(req: &CreateTaskProjectRequest) -> Result<TaskProjectResponse, String> {
    let response = api::post("/tasks/projects")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskProjectResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create project".to_string())
    }
}

pub async fn get_project(id: &str) -> Result<TaskProjectResponse, String> {
    let response = api::get(&format!("/tasks/projects/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskProjectResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 404 {
        Err("Project not found".to_string())
    } else {
        Err("Failed to load project".to_string())
    }
}

pub async fn update_project(
    id: &str,
    req: &UpdateTaskProjectRequest,
) -> Result<TaskProjectResponse, String> {
    let response = api::put(&format!("/tasks/projects/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskProjectResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to update project".to_string())
    }
}

pub async fn delete_project(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/projects/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete project".to_string())
    }
}

pub async fn add_member(
    project_id: &str,
    req: &AddProjectMemberRequest,
) -> Result<(), String> {
    let response = api::post(&format!("/tasks/projects/{}/members", project_id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to add member".to_string())
    }
}

pub async fn update_member(
    project_id: &str,
    user_id: &str,
    req: &UpdateProjectMemberRequest,
) -> Result<(), String> {
    let response = api::put(&format!(
        "/tasks/projects/{}/members/{}",
        project_id, user_id
    ))
    .json(req)
    .map_err(|e| e.to_string())?
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to update member".to_string())
    }
}

pub async fn remove_member(project_id: &str, user_id: &str) -> Result<(), String> {
    let response = api::delete(&format!(
        "/tasks/projects/{}/members/{}",
        project_id, user_id
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to remove member".to_string())
    }
}

// ── Sections ──

pub async fn list_sections(project_id: &str) -> Result<Vec<TaskSectionResponse>, String> {
    let response = api::get(&format!("/tasks/projects/{}/sections", project_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskSectionResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load sections".to_string())
    }
}

pub async fn create_section(
    project_id: &str,
    req: &CreateTaskSectionRequest,
) -> Result<TaskSectionResponse, String> {
    let response = api::post(&format!("/tasks/projects/{}/sections", project_id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskSectionResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create section".to_string())
    }
}

pub async fn update_section(
    id: &str,
    req: &UpdateTaskSectionRequest,
) -> Result<TaskSectionResponse, String> {
    let response = api::put(&format!("/tasks/sections/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskSectionResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to update section".to_string())
    }
}

pub async fn delete_section(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/sections/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete section".to_string())
    }
}

pub async fn reorder_sections(project_id: &str, section_ids: &[&str]) -> Result<(), String> {
    let body = ReorderSectionsRequest {
        section_ids: section_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::put(&format!(
        "/tasks/projects/{}/sections/reorder",
        project_id
    ))
    .json(&body)
    .map_err(|e| e.to_string())?
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to reorder sections".to_string())
    }
}

// ── Tasks ──

pub async fn list_tasks(
    project_id: &str,
    status: Option<&str>,
    section_id: Option<&str>,
) -> Result<Vec<TaskResponse>, String> {
    list_tasks_inner(project_id, status, section_id, false).await
}

/// Fetch all tasks including subtasks (no parent_task_id filter).
pub async fn list_all_tasks(project_id: &str) -> Result<Vec<TaskResponse>, String> {
    list_tasks_inner(project_id, None, None, true).await
}

async fn list_tasks_inner(
    project_id: &str,
    status: Option<&str>,
    section_id: Option<&str>,
    include_subtasks: bool,
) -> Result<Vec<TaskResponse>, String> {
    let mut path = format!("/tasks/projects/{}/tasks", project_id);
    let mut params = Vec::new();
    if let Some(s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(s) = section_id {
        params.push(format!("section_id={}", s));
    }
    if include_subtasks {
        params.push("include_subtasks=true".to_string());
    }
    if !params.is_empty() {
        path.push('?');
        path.push_str(&params.join("&"));
    }

    let response = api::get(&path)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load tasks".to_string())
    }
}

pub async fn list_subtasks(
    project_id: &str,
    parent_task_id: &str,
) -> Result<Vec<TaskResponse>, String> {
    let path = format!(
        "/tasks/projects/{}/tasks?parent_task_id={}",
        project_id, parent_task_id
    );
    let response = api::get(&path)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load subtasks".to_string())
    }
}

pub async fn create_task(
    project_id: &str,
    req: &CreateTaskRequest,
) -> Result<TaskResponse, String> {
    let response = api::post(&format!("/tasks/projects/{}/tasks", project_id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create task".to_string())
    }
}

pub async fn get_task(id: &str) -> Result<TaskResponse, String> {
    let response = api::get(&format!("/tasks/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 404 {
        Err("Task not found".to_string())
    } else {
        Err("Failed to load task".to_string())
    }
}

pub async fn update_task(id: &str, req: &UpdateTaskRequest) -> Result<TaskResponse, String> {
    let response = api::put(&format!("/tasks/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update task".to_string())
    }
}

pub async fn delete_task(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete task".to_string())
    }
}


pub async fn clear_completion_history(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/{}/completion-history", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to clear completion history".to_string())
    }
}

pub async fn update_task_status(
    id: &str,
    req: &UpdateTaskStatusRequest,
) -> Result<TaskResponse, String> {
    let response = api::put(&format!("/tasks/{}/status", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update task status".to_string())
    }
}

pub async fn reorder_tasks(project_id: &str, task_ids: &[&str]) -> Result<(), String> {
    let body = ReorderTasksRequest {
        task_ids: task_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::put(&format!("/tasks/projects/{}/tasks/reorder", project_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to reorder tasks".to_string())
    }
}

pub async fn create_subtask(
    parent_id: &str,
    req: &CreateTaskRequest,
) -> Result<TaskResponse, String> {
    let response = api::post(&format!("/tasks/{}/subtasks", parent_id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create subtask".to_string())
    }
}

pub async fn promote_subtask(id: &str) -> Result<TaskResponse, String> {
    let response = api::post(&format!("/tasks/{}/promote", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to promote subtask".to_string())
    }
}

pub async fn attach_files(task_id: &str, file_ids: &[&str]) -> Result<(), String> {
    let body = AttachFilesRequest {
        file_ids: file_ids.iter().map(|s| s.to_string()).collect(),
    };
    let response = api::post(&format!("/tasks/{}/attachments", task_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to attach files".to_string())
    }
}

pub async fn detach_file(task_id: &str, file_id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/{}/attachments/{}", task_id, file_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to detach file".to_string())
    }
}

// ── Schedule ──

pub async fn get_schedule() -> Result<TaskScheduleResponse, String> {
    let response = api::get("/tasks/schedule")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskScheduleResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load schedule".to_string())
    }
}

pub async fn get_assigned_to_me() -> Result<Vec<TaskResponse>, String> {
    let response = api::get("/tasks/assigned-to-me")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load assigned tasks".to_string())
    }
}

// ── Comments ──

pub async fn list_comments(task_id: &str) -> Result<Vec<TaskCommentResponse>, String> {
    let response = api::get(&format!("/tasks/{}/comments", task_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskCommentResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load comments".to_string())
    }
}

pub async fn create_comment(task_id: &str, body: &str) -> Result<TaskCommentResponse, String> {
    let req = CreateTaskCommentRequest {
        body: body.to_string(),
    };
    let response = api::post(&format!("/tasks/{}/comments", task_id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskCommentResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create comment".to_string())
    }
}

pub async fn update_comment(id: &str, body: &str) -> Result<TaskCommentResponse, String> {
    let req = UpdateTaskCommentRequest {
        body: body.to_string(),
    };
    let response = api::put(&format!("/tasks/comments/{}", id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskCommentResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update comment".to_string())
    }
}

pub async fn delete_comment(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/comments/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete comment".to_string())
    }
}

// ── Labels ──

pub async fn list_labels(project_id: &str) -> Result<Vec<TaskLabelResponse>, String> {
    let response = api::get(&format!("/tasks/projects/{}/labels", project_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<TaskLabelResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load labels".to_string())
    }
}

pub async fn create_label(
    project_id: &str,
    req: &CreateTaskLabelRequest,
) -> Result<TaskLabelResponse, String> {
    let response = api::post(&format!("/tasks/projects/{}/labels", project_id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<TaskLabelResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create label".to_string())
    }
}

pub async fn update_label(
    id: &str,
    req: &UpdateTaskLabelRequest,
) -> Result<TaskLabelResponse, String> {
    let response = api::put(&format!("/tasks/labels/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<TaskLabelResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to update label".to_string())
    }
}

pub async fn delete_label(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/tasks/labels/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete label".to_string())
    }
}
