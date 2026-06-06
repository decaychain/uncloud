//! HTTP wrappers for the experimental mail client surface.

use uncloud_common::FileResponse;
use uncloud_common::{
    CreateMailAccountRequest, CreateMailIdentityRequest, MailAccountResponse,
    MailAccountSyncResponse, MailCredentialStatusResponse, MailDraftAttachmentResponse,
    MailDraftResponse, MailFolderMarkReadResponse, MailFolderResponse, MailFolderSyncResponse,
    MailIdentityResponse, MailMessageBulkMutationRequest, MailMessageBulkMutationResponse,
    MailMessageDetailResponse, MailMessageListResponse, MailMessageMutationAction,
    MailMessageMutationRequest, MailMessageMutationResponse, MailPasswordAuthRequest,
    MailProviderDiagnosticsResponse, MailSyncRequest, SaveMailAttachmentRequest,
    SaveMailAttachmentResponse, SendMailMessageRequest, SendMailMessageResponse,
    SetMailCredentialRequest, UpdateMailAccountRequest, UpdateMailFolderRequest,
    UpdateMailIdentityRequest, UpsertMailDraftRequest,
};
use wasm_bindgen::{JsCast, JsValue};

use super::api;

pub async fn list_accounts() -> Result<Vec<MailAccountResponse>, String> {
    let r = api::get("/mail/accounts")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<MailAccountResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn create_account(req: &CreateMailAccountRequest) -> Result<MailAccountResponse, String> {
    let r = api::post("/mail/accounts")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<MailAccountResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_account(
    account_id: &str,
    req: &UpdateMailAccountRequest,
) -> Result<MailAccountResponse, String> {
    let r = api::put(&format!("/mail/accounts/{account_id}"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailAccountResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_account(account_id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/mail/accounts/{account_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn set_credential(
    account_id: &str,
    password: &str,
) -> Result<MailCredentialStatusResponse, String> {
    let req = SetMailCredentialRequest {
        password: password.to_string(),
    };
    let r = api::put(&format!("/mail/accounts/{account_id}/credential"))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailCredentialStatusResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn diagnostics(account_id: &str) -> Result<MailProviderDiagnosticsResponse, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/diagnostics"))
        .json(&MailPasswordAuthRequest { password: None })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailProviderDiagnosticsResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn send_message(
    account_id: &str,
    req: &SendMailMessageRequest,
) -> Result<SendMailMessageResponse, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/send"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<SendMailMessageResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn list_drafts(account_id: &str) -> Result<Vec<MailDraftResponse>, String> {
    let r = api::get(&format!("/mail/accounts/{account_id}/drafts"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<MailDraftResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn create_draft(
    account_id: &str,
    req: &UpsertMailDraftRequest,
) -> Result<MailDraftResponse, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/drafts"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<MailDraftResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_draft(
    draft_id: &str,
    req: &UpsertMailDraftRequest,
) -> Result<MailDraftResponse, String> {
    let r = api::put(&format!("/mail/drafts/{draft_id}"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailDraftResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_draft(draft_id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/mail/drafts/{draft_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn upload_draft_attachment(
    draft_id: &str,
    file: &web_sys::File,
) -> Result<MailDraftAttachmentResponse, String> {
    let form = web_sys::FormData::new().map_err(|_| "Failed to create FormData".to_string())?;
    let blob = file.unchecked_ref::<web_sys::Blob>();
    form.append_with_blob_and_filename("file", blob, &file.name())
        .map_err(|_| "Failed to append file to form".to_string())?;

    let r = api::post(&format!("/mail/drafts/{draft_id}/attachments"))
        .body(JsValue::from(form))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<MailDraftAttachmentResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_draft_attachment(draft_id: &str, attachment_id: &str) -> Result<(), String> {
    let r = api::delete(&format!(
        "/mail/drafts/{draft_id}/attachments/{attachment_id}"
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn list_folders(account_id: &str) -> Result<Vec<MailFolderResponse>, String> {
    let r = api::get(&format!("/mail/accounts/{account_id}/folders"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<MailFolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn list_identities() -> Result<Vec<MailIdentityResponse>, String> {
    let r = api::get("/mail/identities")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<MailIdentityResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn create_identity(
    req: &CreateMailIdentityRequest,
) -> Result<MailIdentityResponse, String> {
    let r = api::post("/mail/identities")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<MailIdentityResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_identity(
    identity_id: &str,
    req: &UpdateMailIdentityRequest,
) -> Result<MailIdentityResponse, String> {
    let r = api::put(&format!("/mail/identities/{identity_id}"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailIdentityResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_identity(identity_id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/mail/identities/{identity_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn refresh_folders(account_id: &str) -> Result<Vec<MailFolderResponse>, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/folders/refresh"))
        .json(&MailPasswordAuthRequest { password: None })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<MailFolderResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_folder(
    account_id: &str,
    folder_id: &str,
    req: &UpdateMailFolderRequest,
) -> Result<MailFolderResponse, String> {
    let r = api::put(&format!("/mail/accounts/{account_id}/folders/{folder_id}"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailFolderResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn sync_account(
    account_id: &str,
    limit_per_folder: Option<u32>,
) -> Result<MailAccountSyncResponse, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/sync"))
        .json(&MailSyncRequest {
            password: None,
            limit_per_folder,
        })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailAccountSyncResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn sync_folder(
    account_id: &str,
    folder_id: &str,
    limit_per_folder: Option<u32>,
) -> Result<MailFolderSyncResponse, String> {
    let r = api::post(&format!(
        "/mail/accounts/{account_id}/folders/{folder_id}/sync"
    ))
    .json(&MailSyncRequest {
        password: None,
        limit_per_folder,
    })
    .map_err(|e| e.to_string())?
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailFolderSyncResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn mark_folder_read(
    account_id: &str,
    folder_id: &str,
) -> Result<MailFolderMarkReadResponse, String> {
    let r = api::post(&format!(
        "/mail/accounts/{account_id}/folders/{folder_id}/mark-read"
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailFolderMarkReadResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn mark_account_read(account_id: &str) -> Result<MailFolderMarkReadResponse, String> {
    let r = api::post(&format!("/mail/accounts/{account_id}/mark-read"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailFolderMarkReadResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn list_messages(
    account_id: &str,
    folder_id: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<MailMessageListResponse, String> {
    let mut path =
        format!("/mail/accounts/{account_id}/folders/{folder_id}/messages?limit={limit}");
    if let Some(cursor) = cursor.filter(|value| !value.trim().is_empty()) {
        path.push_str("&cursor=");
        path.push_str(cursor);
    }
    let r = api::get(&path).send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailMessageListResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn get_message(message_id: &str) -> Result<MailMessageDetailResponse, String> {
    let r = api::get(&format!("/mail/messages/{message_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailMessageDetailResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn mutate_message(
    message_id: &str,
    action: MailMessageMutationAction,
    target_folder_id: Option<String>,
) -> Result<MailMessageMutationResponse, String> {
    let r = api::post(&format!("/mail/messages/{message_id}/mutate"))
        .json(&MailMessageMutationRequest {
            action,
            target_folder_id,
        })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailMessageMutationResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn bulk_mutate_messages(
    message_ids: Vec<String>,
    action: MailMessageMutationAction,
    target_folder_id: Option<String>,
) -> Result<MailMessageBulkMutationResponse, String> {
    let r = api::post("/mail/messages/bulk-mutate")
        .json(&MailMessageBulkMutationRequest {
            message_ids,
            action,
            target_folder_id,
        })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<MailMessageBulkMutationResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn save_attachment(
    attachment_id: &str,
    parent_id: Option<&str>,
    filename: Option<&str>,
) -> Result<FileResponse, String> {
    let r = api::post(&format!("/mail/attachments/{attachment_id}/save"))
        .json(&SaveMailAttachmentRequest {
            parent_id: parent_id.map(str::to_string),
            filename: filename.map(str::to_string),
        })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<SaveMailAttachmentResponse>()
            .await
            .map(|response| response.file)
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

async fn extract_error(r: gloo_net::http::Response) -> String {
    let status = r.status();
    match r.text().await {
        Ok(t) if !t.is_empty() => t,
        _ => format!("HTTP {status}"),
    }
}
