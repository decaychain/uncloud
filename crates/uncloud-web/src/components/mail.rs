use std::rc::Rc;

use dioxus::prelude::*;
use uncloud_common::{
    CreateMailAccountRequest, CreateMailIdentityRequest, FolderResponse, MailAccountResponse,
    MailAccountSyncResponse, MailAddressDto, MailAttachmentResponse, MailComposeMode,
    MailDraftAttachmentResponse, MailDraftResponse, MailFolderResponse, MailFolderRole,
    MailFolderRoleSource, MailIdentityResponse, MailMessageDetailResponse,
    MailMessageMutationAction, MailMessageSummaryResponse, MailProviderDiagnosticsResponse,
    MailProviderRoleStatus, MailSecurity, MailSentCopyDiagnosticStatus, MailSentCopyStatus,
    MailServerSettings, SendMailMessageRequest, UpdateMailAccountRequest, UpdateMailFolderRequest,
    UpdateMailIdentityRequest, UpsertMailDraftRequest,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::components::icons::{
    IconArchive, IconCheck, IconChevronRight, IconDownload, IconEye, IconFileText, IconFolder,
    IconFolderOpen, IconForward, IconMail, IconMoreVertical, IconMoveRight, IconPaperclip,
    IconPencil, IconPlus, IconRefreshCw, IconReply, IconReplyAll, IconSend, IconSettings, IconStar,
    IconTrash, IconX,
};
use crate::components::scroll_sentinel::ScrollSentinel;
use crate::hooks::{use_files, use_mail};
use crate::router::Route;
use crate::state::MailAccountDirtyTick;

const MAIL_MESSAGE_PAGE_SIZE: u32 = 50;
const MAIL_BACKFILL_PAGE_SIZE: u32 = 50;
const MAIL_STATUS_POLL_MS: u32 = 15_000;
const MAIL_NOTICE_TOAST_TIMEOUT_MS: u32 = 6_000;
const MAIL_MIN_SYNC_INTERVAL_MINUTES: u64 = 1;
const MAIL_MAX_SYNC_INTERVAL_MINUTES: u64 = 7 * 24 * 60;
const MAIL_COMPOSE_EDITOR_ID: &str = "mail-compose-editor";
const MAIL_COMPOSE_ATTACHMENT_INPUT_ID: &str = "mail-compose-attachment-input";

struct MailEditorListener {
    cb: Closure<dyn FnMut(web_sys::Event)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MailMobilePane {
    Folders,
    Messages,
    Detail,
}

impl Drop for MailEditorListener {
    fn drop(&mut self) {
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = self.cb.as_ref().unchecked_ref();
            let _ = win.remove_event_listener_with_callback("uncloud:mail-editor-change", f);
        }
    }
}

#[component]
pub fn MailPage(#[props(default)] route_account_id: Option<String>) -> Element {
    let nav = use_navigator();
    let mut account_dirty = use_context::<Signal<MailAccountDirtyTick>>();
    let mut accounts = use_signal(Vec::<MailAccountResponse>::new);
    let mut identities = use_signal(Vec::<MailIdentityResponse>::new);
    let mut folders = use_signal(Vec::<MailFolderResponse>::new);
    let mut drafts = use_signal(Vec::<MailDraftResponse>::new);
    let mut messages = use_signal(Vec::<MailMessageSummaryResponse>::new);
    let mut detail = use_signal(|| None::<MailMessageDetailResponse>);
    let remote_images_loaded_messages = use_signal(Vec::<String>::new);
    let mut selected_account = use_signal(String::new);
    let mut selected_folder = use_signal(String::new);
    let mut selected_message = use_signal(String::new);
    let mut selecting_messages = use_signal(|| false);
    let mut selected_message_ids = use_signal(Vec::<String>::new);
    let mut mobile_mail_pane = use_signal(|| MailMobilePane::Folders);
    let mut loading = use_signal(|| true);
    let mut syncing = use_signal(|| false);
    let mut refreshing_folders = use_signal(|| false);
    let mut loading_more_messages = use_signal(|| false);
    let mut backfilling_messages = use_signal(|| false);
    let mut loading_detail = use_signal(|| false);
    let mut mutating_message = use_signal(|| false);
    let mut bulk_mutating_messages = use_signal(|| false);
    let mut move_target_folder = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<String>);
    let mut notice_auto_dismiss_token = use_signal(|| 0u64);
    let mut sync_status = use_signal(|| None::<String>);
    let mut show_setup = use_signal(|| false);
    let mut show_compose = use_signal(|| false);
    let mut sending_message = use_signal(|| false);
    let mut saving_draft = use_signal(|| false);
    let mut uploading_draft_attachment = use_signal(|| false);
    let mut compose_autosave_token = use_signal(|| 0u64);
    let mut compose_draft_id = use_signal(String::new);
    let mut compose_mode = use_signal(|| MailComposeMode::New);
    let mut compose_source_message_id = use_signal(String::new);
    let mut compose_identity_id = use_signal(String::new);
    let mut compose_to = use_signal(String::new);
    let mut compose_cc = use_signal(String::new);
    let mut compose_bcc = use_signal(String::new);
    let mut compose_subject = use_signal(String::new);
    let mut compose_body = use_signal(String::new);
    let mut compose_body_html = use_signal(String::new);
    let mut compose_in_reply_to = use_signal(String::new);
    let mut compose_references = use_signal(Vec::<String>::new);
    let mut compose_attachments = use_signal(Vec::<MailDraftAttachmentResponse>::new);
    let mut compose_editor_key = use_signal(|| 0u64);
    let mut settings_folder = use_signal(|| None::<MailFolderResponse>);
    let mut folder_role_value = use_signal(|| "auto".to_string());
    let mut folder_sync_enabled = use_signal(|| true);
    let mut saving_folder_settings = use_signal(|| false);
    let mut account_settings = use_signal(|| None::<MailAccountResponse>);
    let mut account_display_name = use_signal(String::new);
    let mut account_email_address = use_signal(String::new);
    let mut account_imap_host = use_signal(String::new);
    let mut account_imap_port = use_signal(String::new);
    let mut account_imap_security = use_signal(|| "tls".to_string());
    let mut account_imap_username = use_signal(String::new);
    let mut account_smtp_host = use_signal(String::new);
    let mut account_smtp_port = use_signal(String::new);
    let mut account_smtp_security = use_signal(|| "tls".to_string());
    let mut account_smtp_username = use_signal(String::new);
    let mut account_password = use_signal(String::new);
    let mut account_sync_enabled = use_signal(|| false);
    let mut account_sync_interval_minutes = use_signal(String::new);
    let mut account_sort_order = use_signal(String::new);
    let mut saving_account_settings = use_signal(|| false);
    let mut confirming_account_delete = use_signal(|| false);
    let mut deleting_account = use_signal(|| false);
    let mut provider_diagnostics = use_signal(|| None::<MailProviderDiagnosticsResponse>);
    let mut loading_provider_diagnostics = use_signal(|| false);
    let mut identity_editor_open = use_signal(|| false);
    let mut identity_editing_id = use_signal(String::new);
    let mut identity_display_name = use_signal(String::new);
    let mut identity_email_address = use_signal(String::new);
    let mut identity_reply_to = use_signal(String::new);
    let mut identity_signature = use_signal(String::new);
    let mut identity_is_default = use_signal(|| false);
    let mut saving_identity = use_signal(|| false);
    let mut deleting_identity_id = use_signal(String::new);
    let mut message_next_cursor = use_signal(|| None::<String>);
    let mut message_has_more = use_signal(|| false);
    let mut saving_attachment = use_signal(|| None::<MailAttachmentResponse>);
    let mut attachment_save_parent = use_signal(|| None::<String>);
    let mut attachment_save_folders = use_signal(Vec::<FolderResponse>::new);
    let mut attachment_save_breadcrumb = use_signal(Vec::<FolderResponse>::new);
    let mut attachment_save_loading = use_signal(|| false);
    let mut attachment_save_busy = use_signal(|| false);

    let mut display_name = use_signal(String::new);
    let mut email_address = use_signal(String::new);
    let mut imap_host = use_signal(String::new);
    let mut imap_port = use_signal(|| "993".to_string());
    let mut imap_security = use_signal(|| "tls".to_string());
    let mut imap_username = use_signal(String::new);
    let mut smtp_host = use_signal(String::new);
    let mut smtp_port = use_signal(|| "465".to_string());
    let mut smtp_security = use_signal(|| "tls".to_string());
    let mut smtp_username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut creating = use_signal(|| false);

    use_hook(move || {
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
            let Some(custom) = e.dyn_ref::<web_sys::CustomEvent>() else {
                return;
            };
            let detail = custom.detail();
            if mail_editor_detail_string(&detail, "id") != MAIL_COMPOSE_EDITOR_ID {
                return;
            }
            compose_body_html.set(mail_editor_detail_string(&detail, "html"));
            compose_body.set(mail_editor_detail_string(&detail, "text"));
        });
        if let Some(win) = web_sys::window() {
            let f: &js_sys::Function = cb.as_ref().unchecked_ref();
            let _ = win.add_event_listener_with_callback("uncloud:mail-editor-change", f);
        }
        Rc::new(MailEditorListener { cb })
    });

    use_effect(move || {
        let show = show_compose();
        let _key = compose_editor_key();
        let editor_id = serde_json::to_string(MAIL_COMPOSE_EDITOR_ID)
            .unwrap_or_else(|_| "\"mail-compose-editor\"".to_string());

        if !show {
            let _ = js_sys::eval(&format!(
                "window.UncloudMailEditor && window.UncloudMailEditor.destroy({editor_id});"
            ));
            return;
        }

        let content = serde_json::to_string(&compose_body_html.peek().clone())
            .unwrap_or_else(|_| "\"\"".to_string());
        let _ = js_sys::eval(&format!(
            "window.UncloudMailEditor && window.UncloudMailEditor.mount({editor_id}, {content});"
        ));
    });

    use_effect(move || {
        let Some(message) = notice() else {
            return;
        };
        let token = *notice_auto_dismiss_token.peek() + 1;
        notice_auto_dismiss_token.set(token);
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(MAIL_NOTICE_TOAST_TIMEOUT_MS).await;
            let should_clear = {
                let current_notice = notice.peek();
                *notice_auto_dismiss_token.peek() == token
                    && current_notice.as_ref() == Some(&message)
            };
            if should_clear {
                notice.set(None);
            }
        });
    });

    use_effect(use_reactive!(|(route_account_id)| {
        let requested_account_id = route_account_id.clone().unwrap_or_default();
        spawn(async move {
            // The route param is the source of truth for the active account. Skip
            // reloading when this run is merely the echo of our own nav.replace
            // (the account is already selected and its data is loaded).
            if !requested_account_id.is_empty()
                && selected_account.peek().as_str() == requested_account_id
                && !accounts.peek().is_empty()
            {
                return;
            }
            loading.set(true);
            match use_mail::list_accounts().await {
                Ok(list) => {
                    let first = list.first().map(|a| a.id.clone()).unwrap_or_default();
                    let selected = if !requested_account_id.is_empty()
                        && list
                            .iter()
                            .any(|account| account.id == requested_account_id)
                    {
                        requested_account_id.clone()
                    } else {
                        first.clone()
                    };
                    let account_changed = selected_account.peek().as_str() != selected;
                    accounts.set(list);
                    selected_account.set(selected.clone());
                    selecting_messages.set(false);
                    selected_message_ids.set(Vec::new());
                    mobile_mail_pane.set(MailMobilePane::Folders);
                    if account_changed {
                        selected_folder.set(String::new());
                        selected_message.set(String::new());
                        messages.set(Vec::new());
                        message_next_cursor.set(None);
                        message_has_more.set(false);
                        detail.set(None);
                    }
                    if requested_account_id != selected {
                        if selected.is_empty() {
                            nav.replace(Route::Mail {});
                        } else {
                            nav.replace(Route::MailAccount {
                                account_id: selected.clone(),
                            });
                        }
                    }
                    match use_mail::list_identities().await {
                        Ok(rows) => identities.set(rows),
                        Err(e) => error.set(Some(e)),
                    }
                    if !selected.is_empty() {
                        match use_mail::list_folders(&selected).await {
                            Ok(rows) => folders.set(rows),
                            Err(e) => error.set(Some(e)),
                        }
                        match use_mail::list_drafts(&selected).await {
                            Ok(rows) => drafts.set(rows),
                            Err(e) => error.set(Some(e)),
                        }
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    }));

    use_effect(use_reactive!(|(account_dirty)| {
        let _ = account_dirty().0;
        let account_id = selected_account.peek().clone();
        if account_id.is_empty() {
            return;
        }
        spawn(async move {
            if let Ok(rows) = use_mail::list_accounts().await {
                accounts.set(rows);
            }
            if let Ok(rows) = use_mail::list_folders(&account_id).await {
                folders.set(rows);
            }
            if *loading_more_messages.peek() || *backfilling_messages.peek() {
                return;
            }
            let folder_id = selected_folder.peek().clone();
            if folder_id.is_empty() {
                return;
            }
            if let Ok(page) =
                use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None).await
            {
                let selected = selected_message.peek().clone();
                let still_selected = page.messages.iter().any(|message| message.id == selected);
                messages.set(page.messages);
                message_next_cursor.set(page.next_cursor);
                message_has_more.set(page.has_more);
                if !selected.is_empty() && !still_selected {
                    selected_message.set(String::new());
                    detail.set(None);
                    mobile_mail_pane.set(MailMobilePane::Messages);
                }
            }
        });
    }));

    use_effect(move || {
        if saving_attachment().is_none() {
            return;
        }
        let parent = attachment_save_parent();
        spawn(async move {
            attachment_save_loading.set(true);
            if let Ok(rows) = use_files::list_folders(parent.as_deref()).await {
                attachment_save_folders.set(rows);
            }
            match &parent {
                Some(parent_id) => {
                    if let Ok(rows) = use_files::get_breadcrumb(parent_id).await {
                        attachment_save_breadcrumb.set(rows);
                    }
                }
                None => attachment_save_breadcrumb.set(Vec::new()),
            }
            attachment_save_loading.set(false);
        });
    });

    use_effect(move || {
        if !show_compose() || *sending_message.peek() || *saving_draft.peek() {
            return;
        }
        let account_id = selected_account();
        if account_id.is_empty() {
            return;
        }
        let identity_id = compose_identity_id();
        let draft_id = compose_draft_id.peek().clone();
        let mode = compose_mode();
        let source_message_id = compose_source_message_id();
        let to = compose_to();
        let cc = compose_cc();
        let bcc = compose_bcc();
        let subject = compose_subject();
        let body = compose_body();
        let body_html = compose_body_html();
        let in_reply_to = compose_in_reply_to();
        let references = compose_references();
        let has_content = !draft_id.trim().is_empty()
            || !to.trim().is_empty()
            || !cc.trim().is_empty()
            || !bcc.trim().is_empty()
            || !subject.trim().is_empty()
            || !body.trim().is_empty()
            || !body_html.trim().is_empty()
            || !compose_attachments.peek().is_empty();
        if !has_content {
            return;
        }

        let token = *compose_autosave_token.peek() + 1;
        compose_autosave_token.set(token);
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(1_200).await;
            if *compose_autosave_token.peek() != token
                || !*show_compose.peek()
                || *sending_message.peek()
                || *saving_draft.peek()
            {
                return;
            }

            let req = UpsertMailDraftRequest {
                identity_id: nonempty_string(identity_id),
                mode,
                source_message_id: nonempty_string(source_message_id),
                to: parse_compose_addresses(&to),
                cc: parse_compose_addresses(&cc),
                bcc: parse_compose_addresses(&bcc),
                subject,
                body_text: body,
                body_html: nonempty_string(body_html),
                in_reply_to: nonempty_string(in_reply_to),
                references,
            };
            saving_draft.set(true);
            let result = if draft_id.trim().is_empty() {
                use_mail::create_draft(&account_id, &req).await
            } else {
                use_mail::update_draft(&draft_id, &req).await
            };
            match result {
                Ok(saved) => {
                    compose_draft_id.set(saved.id.clone());
                    compose_attachments.set(saved.attachments.clone());
                    drafts.set(upsert_draft(drafts(), saved));
                }
                Err(e) => error.set(Some(format!("Draft autosave failed: {e}"))),
            }
            saving_draft.set(false);
        });
    });

    use_effect(move || {
        let account_id = selected_account();
        if account_id.is_empty() {
            return;
        }

        spawn(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(MAIL_STATUS_POLL_MS).await;
                if selected_account.peek().as_str() != account_id {
                    break;
                }

                let was_syncing = accounts
                    .peek()
                    .iter()
                    .any(|account| account.id == account_id && account.sync_in_progress)
                    || folders
                        .peek()
                        .iter()
                        .any(|folder| folder.account_id == account_id && folder.sync_in_progress);
                let mut now_syncing = was_syncing;

                if let Ok(rows) = use_mail::list_accounts().await {
                    now_syncing = rows
                        .iter()
                        .any(|account| account.id == account_id && account.sync_in_progress);
                    accounts.set(rows);
                }
                if let Ok(rows) = use_mail::list_folders(&account_id).await {
                    now_syncing = now_syncing
                        || rows.iter().any(|folder| {
                            folder.account_id == account_id && folder.sync_in_progress
                        });
                    folders.set(rows);
                }

                if was_syncing
                    && !now_syncing
                    && !*syncing.peek()
                    && !*backfilling_messages.peek()
                    && !*loading_more_messages.peek()
                {
                    let folder_id = selected_folder.peek().clone();
                    if !folder_id.is_empty() {
                        if let Ok(page) = use_mail::list_messages(
                            &account_id,
                            &folder_id,
                            MAIL_MESSAGE_PAGE_SIZE,
                            None,
                        )
                        .await
                        {
                            let selected = selected_message.peek().clone();
                            let still_selected =
                                page.messages.iter().any(|message| message.id == selected);
                            messages.set(page.messages);
                            message_next_cursor.set(page.next_cursor);
                            message_has_more.set(page.has_more);
                            if !selected.is_empty() && !still_selected {
                                selected_message.set(String::new());
                                detail.set(None);
                                mobile_mail_pane.set(MailMobilePane::Messages);
                            }
                        }
                    }
                }
            }
        });
    });

    let accounts_snapshot = accounts();
    let identities_snapshot = identities();
    let folders_snapshot = sorted_mail_folders(&folders());
    let drafts_snapshot = drafts();
    let messages_snapshot = messages();
    let detail_snapshot = detail();
    let mobile_mail_pane_snapshot = mobile_mail_pane();
    let selected_account_id = selected_account();
    let selected_folder_id = selected_folder();
    let selected_message_id = selected_message();
    let selecting_messages_snapshot = selecting_messages();
    let selected_message_ids_snapshot = selected_message_ids();
    let selected_message_count = selected_message_ids_snapshot.len();
    let active_account = accounts_snapshot
        .iter()
        .find(|a| a.id == selected_account_id)
        .cloned();
    let active_folder = folders_snapshot
        .iter()
        .find(|f| f.id == selected_folder_id)
        .cloned();
    let active_identities = identities_snapshot
        .iter()
        .filter(|identity| identity.account_id == selected_account_id)
        .cloned()
        .collect::<Vec<_>>();
    let active_default_identity_id = default_mail_identity_id(&active_identities);
    let can_backfill_active_folder = active_folder
        .as_ref()
        .map(|folder| folder.selectable && !folder.sync_completed)
        .unwrap_or(false);
    let has_archive_target_for_active_folder = active_folder
        .as_ref()
        .map(|active| {
            folders_snapshot.iter().any(|folder| {
                folder.selectable
                    && folder.id != active.id
                    && matches!(
                        folder.role,
                        Some(MailFolderRole::Archive | MailFolderRole::AllMail)
                    )
            })
        })
        .unwrap_or(false);
    let has_trash_target_for_active_folder = active_folder
        .as_ref()
        .map(|active| {
            folders_snapshot.iter().any(|folder| {
                folder.selectable
                    && folder.id != active.id
                    && folder.role == Some(MailFolderRole::Trash)
            })
        })
        .unwrap_or(false);
    let background_sync_in_progress = active_account
        .as_ref()
        .map(|account| account.sync_in_progress)
        .unwrap_or(false)
        || folders_snapshot
            .iter()
            .any(|folder| folder.sync_in_progress);
    let sync_status_message = sync_status()
        .or_else(|| {
            if backfilling_messages() {
                Some("Syncing older messages".to_string())
            } else {
                None
            }
        })
        .or_else(|| {
            if syncing() {
                Some("Syncing mail".to_string())
            } else {
                None
            }
        })
        .or_else(|| {
            if background_sync_in_progress {
                Some("Syncing mail".to_string())
            } else {
                None
            }
        });
    let toast_stack_class = if sync_status_message.is_some() {
        concat!(
            "pointer-events-none fixed bottom-14 right-4 z-[60] flex ",
            "w-[min(28rem,calc(100vw-2rem))] flex-col gap-2",
        )
    } else {
        concat!(
            "pointer-events-none fixed bottom-4 right-4 z-[60] flex ",
            "w-[min(28rem,calc(100vw-2rem))] flex-col gap-2",
        )
    };

    let trigger_load_more_messages = move || {
        if *syncing.peek() || *loading_more_messages.peek() || *backfilling_messages.peek() {
            return;
        }
        let account_id = selected_account.peek().clone();
        let folder_id = selected_folder.peek().clone();
        if account_id.is_empty() || folder_id.is_empty() {
            return;
        }

        if let Some(cursor) = message_next_cursor.peek().clone() {
            spawn(async move {
                loading_more_messages.set(true);
                error.set(None);
                match use_mail::list_messages(
                    &account_id,
                    &folder_id,
                    MAIL_MESSAGE_PAGE_SIZE,
                    Some(&cursor),
                )
                .await
                {
                    Ok(page) => {
                        messages.set(append_unique_messages(messages(), page.messages));
                        message_next_cursor.set(page.next_cursor);
                        message_has_more.set(page.has_more);
                    }
                    Err(e) => error.set(Some(e)),
                }
                loading_more_messages.set(false);
            });
            return;
        }

        let should_backfill = folders
            .peek()
            .iter()
            .find(|folder| folder.id == folder_id)
            .map(|folder| folder.selectable && !folder.sync_completed)
            .unwrap_or(false);
        if !should_backfill {
            return;
        }
        let last_cached_cursor = messages
            .peek()
            .last()
            .map(|message| message.uid.to_string());
        spawn(async move {
            backfilling_messages.set(true);
            sync_status.set(Some("Syncing older messages".to_string()));
            error.set(None);
            match use_mail::sync_folder(&account_id, &folder_id, Some(MAIL_BACKFILL_PAGE_SIZE))
                .await
            {
                Ok(_) => {
                    if let Ok(rows) = use_mail::list_folders(&account_id).await {
                        folders.set(rows);
                    }
                    match use_mail::list_messages(
                        &account_id,
                        &folder_id,
                        MAIL_MESSAGE_PAGE_SIZE,
                        last_cached_cursor.as_deref(),
                    )
                    .await
                    {
                        Ok(page) => {
                            messages.set(append_unique_messages(messages(), page.messages));
                            message_next_cursor.set(page.next_cursor);
                            message_has_more.set(page.has_more);
                        }
                        Err(e) => error.set(Some(e)),
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            backfilling_messages.set(false);
            sync_status.set(None);
        });
    };

    rsx! {
        div { class: "space-y-3",
            div { class: "flex flex-col gap-3 md:flex-row md:items-center md:justify-between",
                div {
                    h1 { class: "text-2xl font-semibold tracking-normal", "Mail" }
                    div { class: "flex items-center gap-2 text-sm text-base-content/60",
                        if let Some(account) = active_account.as_ref() {
                            span { "{account.email_address}" }
                            button {
                                class: "btn btn-ghost btn-xs h-7 min-h-7 w-7 p-0",
                                title: "Account settings",
                                onclick: {
                                    let account = account.clone();
                                    move |_| {
                                        account_display_name.set(account.display_name.clone());
                                        account_email_address.set(account.email_address.clone());
                                        account_imap_host.set(account.imap.host.clone());
                                        account_imap_port.set(account.imap.port.to_string());
                                        account_imap_security.set(security_to_value(account.imap.security).to_string());
                                        account_imap_username.set(account.imap.username.clone());
                                        account_smtp_host.set(account.smtp.host.clone());
                                        account_smtp_port.set(account.smtp.port.to_string());
                                        account_smtp_security.set(security_to_value(account.smtp.security).to_string());
                                        account_smtp_username.set(account.smtp.username.clone());
                                        account_password.set(String::new());
                                        account_sync_enabled.set(account.sync_enabled);
                                        account_sync_interval_minutes.set(
                                            account
                                                .sync_interval_secs
                                                .map(sync_interval_minutes_value)
                                                .unwrap_or_default(),
                                        );
                                        account_sort_order.set(account.sort_order.to_string());
                                        confirming_account_delete.set(false);
                                        provider_diagnostics.set(None);
                                        loading_provider_diagnostics.set(false);
                                        identity_editor_open.set(false);
                                        identity_editing_id.set(String::new());
                                        identity_display_name.set(String::new());
                                        identity_email_address.set(String::new());
                                        identity_reply_to.set(String::new());
                                        identity_signature.set(String::new());
                                        identity_is_default.set(false);
                                        deleting_identity_id.set(String::new());
                                        account_settings.set(Some(account.clone()));
                                    }
                                },
                                IconSettings { class: "h-3.5 w-3.5".to_string() }
                            }
                        } else {
                            span { "No account selected" }
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        class: "btn btn-sm btn-primary gap-2",
                        disabled: selected_account_id.is_empty(),
                        onclick: move |_| {
                            compose_draft_id.set(String::new());
                            compose_mode.set(MailComposeMode::New);
                            compose_source_message_id.set(String::new());
                            let account_id = selected_account.peek().clone();
                            compose_identity_id.set(default_mail_identity_id_for_account(
                                &identities.peek(),
                                &account_id,
                            ));
                            compose_to.set(String::new());
                            compose_cc.set(String::new());
                            compose_bcc.set(String::new());
                            compose_subject.set(String::new());
                            compose_body.set(String::new());
                            compose_body_html.set(String::new());
                            compose_in_reply_to.set(String::new());
                            compose_references.set(Vec::new());
                            compose_attachments.set(Vec::new());
                            bump_compose_editor_key(&mut compose_editor_key);
                            show_compose.set(true);
                            mobile_mail_pane.set(MailMobilePane::Detail);
                        },
                        IconSend { class: "w-4 h-4".to_string() }
                        span { "Compose" }
                    }
                    button {
                        class: "btn btn-sm btn-outline gap-2",
                        disabled: syncing() || refreshing_folders() || selected_account_id.is_empty(),
                        onclick: move |_| {
                            let account_id = selected_account();
                            if account_id.is_empty() {
                                return;
                            }
                            spawn(async move {
                                syncing.set(true);
                                sync_status.set(Some("Syncing account".to_string()));
                                error.set(None);
                                match use_mail::sync_account(&account_id, Some(25)).await {
                                    Ok(result) => {
                                        if let Some(message) = account_sync_error_notice(&result) {
                                            error.set(Some(message));
                                        }
                                        if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                            folders.set(rows);
                                        }
                                        let folder_id = selected_folder();
                                        if !folder_id.is_empty() {
                                            if let Ok(page) = use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None).await {
                                                let selected = selected_message.peek().clone();
                                                let still_selected = page.messages.iter().any(|message| message.id == selected);
                                                messages.set(page.messages);
                                                message_next_cursor.set(page.next_cursor);
                                                message_has_more.set(page.has_more);
                                                if !selected.is_empty() && !still_selected {
                                                    selected_message.set(String::new());
                                                    detail.set(None);
                                                    mobile_mail_pane.set(MailMobilePane::Messages);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => error.set(Some(e)),
                                }
                                syncing.set(false);
                                sync_status.set(None);
                            });
                        },
                        if syncing() {
                            span { class: "loading loading-spinner loading-xs" }
                        } else {
                            IconRefreshCw { class: "w-4 h-4".to_string() }
                        }
                        span { "Sync account" }
                    }
                    button {
                        class: "btn btn-sm btn-primary gap-2",
                        onclick: move |_| show_setup.set(true),
                        IconPlus { class: "w-4 h-4".to_string() }
                        span { "Add account" }
                    }
                }
            }

            if loading() {
                div { class: "flex h-80 items-center justify-center text-base-content/60",
                    span { class: "loading loading-spinner loading-md" }
                }
            } else if accounts_snapshot.is_empty() {
                div { class: "border border-base-300 bg-base-100 p-6 text-center",
                    IconMail { class: "mx-auto mb-3 h-10 w-10 text-base-content/50".to_string() }
                    h2 { class: "text-lg font-semibold", "Mail" }
                    div { class: "mt-3",
                        button {
                            class: "btn btn-primary btn-sm gap-2",
                            onclick: move |_| show_setup.set(true),
                            IconPlus { class: "w-4 h-4".to_string() }
                            span { "Add account" }
                        }
                    }
                }
            } else {
                div { class: "grid min-h-0 grid-cols-1 gap-3 lg:h-[calc(100vh-9rem)] lg:grid-cols-[18rem_minmax(22rem,30rem)_minmax(0,1fr)]",
                    section { class: "{mail_folders_pane_class(mobile_mail_pane_snapshot)}",
                        if !drafts_snapshot.is_empty() {
                            div { class: "border-b border-base-300",
                                div { class: "flex items-center justify-between px-3 py-2",
                                    div { class: "text-xs font-semibold uppercase text-base-content/60", "Local drafts" }
                                    span { class: "badge badge-ghost badge-sm", "{drafts_snapshot.len()}" }
                                }
                                div { class: "max-h-40 overflow-y-auto p-1",
                                    for draft in drafts_snapshot.clone() {
                                        {
                                            let draft_id = draft.id.clone();
                                            let draft_for_open = draft.clone();
                                            let draft_subject = draft_subject_label(&draft);
                                            let draft_meta = draft.updated_at.clone();
                                            rsx! {
                                                div {
                                                    key: "{draft_id}",
                                                    class: "group flex items-center gap-1 text-sm hover:bg-base-200",
                                                    button {
                                                        class: "min-w-0 flex-1 px-2 py-1.5 text-left",
                                                        onclick: move |_| {
                                                            compose_draft_id.set(draft_for_open.id.clone());
                                                            compose_mode.set(draft_for_open.mode);
                                                            compose_source_message_id.set(
                                                                draft_for_open.source_message_id.clone().unwrap_or_default(),
                                                            );
                                                            compose_identity_id.set(
                                                                draft_for_open.identity_id.clone().unwrap_or_default(),
                                                            );
                                                            compose_to.set(compose_address_line(&draft_for_open.to));
                                                            compose_cc.set(compose_address_line(&draft_for_open.cc));
                                                            compose_bcc.set(compose_address_line(&draft_for_open.bcc));
                                                            compose_subject.set(draft_for_open.subject.clone());
                                                            compose_body.set(draft_for_open.body_text.clone());
                                                            compose_body_html.set(
                                                                draft_for_open
                                                                    .body_html
                                                                    .clone()
                                                                    .unwrap_or_else(|| {
                                                                        plain_text_to_html(&draft_for_open.body_text)
                                                                    }),
                                                            );
                                                            compose_in_reply_to.set(
                                                                draft_for_open.in_reply_to.clone().unwrap_or_default(),
                                                            );
                                                            compose_references.set(draft_for_open.references.clone());
                                                            compose_attachments.set(draft_for_open.attachments.clone());
                                                            bump_compose_editor_key(&mut compose_editor_key);
                                                            show_compose.set(true);
                                                            mobile_mail_pane.set(MailMobilePane::Detail);
                                                        },
                                                        div { class: "truncate font-medium", "{draft_subject}" }
                                                        div { class: "truncate text-xs text-base-content/50",
                                                            "{short_date(Some(draft_meta.as_str()))}"
                                                        }
                                                    }
                                                    button {
                                                        class: "btn btn-ghost btn-xs h-7 min-h-7 w-7 p-0 opacity-0 group-hover:opacity-100",
                                                        title: "Delete draft",
                                                        onclick: {
                                                            let draft_id = draft_id.clone();
                                                            move |_| {
                                                                let draft_id = draft_id.clone();
                                                                spawn(async move {
                                                                    error.set(None);
                                                                    match use_mail::delete_draft(&draft_id).await {
                                                                        Ok(()) => {
                                                                            drafts.set(
                                                                                drafts()
                                                                                    .into_iter()
                                                                                    .filter(|draft| draft.id != draft_id)
                                                                                    .collect(),
                                                                            );
                                                                        }
                                                                        Err(e) => error.set(Some(e)),
                                                                    }
                                                                });
                                                            }
                                                        },
                                                        IconX { class: "h-3.5 w-3.5".to_string() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center justify-between border-b border-base-300 px-3 py-2",
                            div { class: "text-xs font-semibold uppercase text-base-content/60", "Folders" }
                            button {
                                class: "btn btn-ghost btn-xs",
                                disabled: selected_account_id.is_empty() || syncing() || refreshing_folders(),
                                onclick: move |_| {
                                    let account_id = selected_account();
                                    if account_id.is_empty() {
                                        return;
                                    }
                                    spawn(async move {
                                        refreshing_folders.set(true);
                                        sync_status.set(Some("Refreshing folders".to_string()));
                                        error.set(None);
                                        match use_mail::refresh_folders(&account_id).await {
                                            Ok(rows) => folders.set(rows),
                                            Err(e) => error.set(Some(e)),
                                        }
                                        refreshing_folders.set(false);
                                        sync_status.set(None);
                                    });
                                },
                                if refreshing_folders() {
                                    span { class: "loading loading-spinner loading-xs" }
                                } else {
                                    IconRefreshCw { class: "w-4 h-4".to_string() }
                                }
                            }
                        }
                        div { class: "min-h-0 flex-1 overflow-y-auto p-1",
                            if folders_snapshot.is_empty() {
                                div { class: "px-3 py-4 text-sm text-base-content/60", "No folders" }
                            }
                            for folder in folders_snapshot.clone() {
                                {
                                    let id = folder.id.clone();
                                    let account_id = folder.account_id.clone();
                                    let active = id == selected_folder_id;
                                    let depth = folder_depth(&folder);
                                    let auto_sync_empty_folder = folder.selectable && !folder.sync_completed;
                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: if active {
                                                "group flex w-full items-center gap-1 bg-primary/10 text-sm text-primary"
                                            } else if folder.selectable {
                                                "group flex w-full items-center gap-1 text-sm hover:bg-base-200"
                                            } else {
                                                "group flex w-full items-center gap-1 text-sm text-base-content/45"
                                            },
                                            style: "padding-left: {0.5 + depth as f32 * 1.0}rem",
                                            button {
                                                class: "flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-1 text-left disabled:cursor-not-allowed",
                                                disabled: !folder.selectable,
                                                onclick: move |_| {
                                                    let account_id = account_id.clone();
                                                    let folder_id = id.clone();
                                                    let auto_sync_empty_folder = auto_sync_empty_folder;
                                                    mobile_mail_pane.set(MailMobilePane::Messages);
                                                    spawn(async move {
                                                        selected_folder.set(folder_id.clone());
                                                        selected_message.set(String::new());
                                                        selecting_messages.set(false);
                                                        selected_message_ids.set(Vec::new());
                                                        move_target_folder.set(String::new());
                                                        detail.set(None);
                                                        error.set(None);
                                                        message_next_cursor.set(None);
                                                        message_has_more.set(false);
                                                        match use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None).await {
                                                            Ok(page) => {
                                                                let should_auto_sync =
                                                                    page.messages.is_empty()
                                                                        && !page.has_more
                                                                        && auto_sync_empty_folder
                                                                        && !*syncing.peek()
                                                                        && !*backfilling_messages.peek();
                                                                messages.set(page.messages);
                                                                message_next_cursor.set(page.next_cursor);
                                                                message_has_more.set(page.has_more);
                                                                if should_auto_sync {
                                                                    backfilling_messages.set(true);
                                                                    sync_status.set(Some("Syncing messages".to_string()));
                                                                    match use_mail::sync_folder(&account_id, &folder_id, Some(MAIL_BACKFILL_PAGE_SIZE)).await {
                                                                        Ok(_) => {
                                                                            if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                                                folders.set(rows);
                                                                            }
                                                                            match use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None).await {
                                                                                Ok(page) => {
                                                                                    messages.set(page.messages);
                                                                                    message_next_cursor.set(page.next_cursor);
                                                                                    message_has_more.set(page.has_more);
                                                                                }
                                                                                Err(e) => error.set(Some(e)),
                                                                            }
                                                                        }
                                                                        Err(e) => error.set(Some(e)),
                                                                    }
                                                                    backfilling_messages.set(false);
                                                                    sync_status.set(None);
                                                                }
                                                            }
                                                            Err(e) => error.set(Some(e)),
                                                        }
                                                    });
                                                },
                                                IconFolder { class: "h-4 w-4".to_string() }
                                                span { class: "min-w-0 flex-1 truncate", "{folder.name}" }
                                                if let Some(label) = folder_role_label(folder.role) {
                                                    span {
                                                        class: if folder.role_source == MailFolderRoleSource::User {
                                                            "badge badge-primary badge-outline badge-xs"
                                                        } else {
                                                            "badge badge-ghost badge-xs"
                                                        },
                                                        "{label}"
                                                    }
                                                }
                                                if folder.unread_count > 0 {
                                                    span { class: "inline-flex min-w-5 shrink-0 items-center justify-center rounded-full bg-primary px-2 py-0.5 text-[11px] font-semibold leading-none text-primary-content",
                                                        "{folder.unread_count}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    section { class: "{mail_messages_pane_class(mobile_mail_pane_snapshot)}",
                        div {
                            class: if selecting_messages_snapshot {
                                "flex flex-col gap-2 border-b border-base-300 px-3 py-2"
                            } else {
                                "flex items-center justify-between border-b border-base-300 px-3 py-2"
                            },
                            div { class: "flex min-w-0 items-center gap-2",
                                button {
                                    class: "btn btn-ghost btn-sm h-8 min-h-8 px-2 lg:hidden",
                                    onclick: move |_| mobile_mail_pane.set(MailMobilePane::Folders),
                                    IconChevronRight { class: "h-4 w-4 rotate-180".to_string() }
                                    span { "Folders" }
                                }
                                div { class: "min-w-0",
                                    div { class: "text-xs font-semibold uppercase text-base-content/60", "Messages" }
                                    div { class: "truncate text-sm",
                                        if let Some(folder) = active_folder.as_ref() {
                                            "{folder.display_path}"
                                        } else {
                                            "Select a folder"
                                        }
                                    }
                                }
                            }
                            if selecting_messages_snapshot {
                                div { class: "flex w-full flex-wrap items-center gap-1 border-t border-base-300 pt-2",
                                    span { class: "px-2 text-xs text-base-content/60", "{selected_message_count} selected" }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 px-2",
                                        disabled: bulk_mutating_messages(),
                                        onclick: move |_| selected_message_ids.set(Vec::new()),
                                        "Clear"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 px-2",
                                        disabled: bulk_mutating_messages() || messages_snapshot.is_empty(),
                                        onclick: move |_| selected_message_ids.set(loaded_message_ids(&messages())),
                                        "All"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                        disabled: bulk_mutating_messages() || selected_message_count == 0,
                                        onclick: move |_| {
                                            let ids = selected_message_ids();
                                            spawn(async move {
                                                bulk_mutating_messages.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let action = MailMessageMutationAction::MarkRead;
                                                match use_mail::bulk_mutate_messages(ids, action, None).await {
                                                    Ok(result) => {
                                                        if !result.messages.is_empty() {
                                                            messages.set(replace_messages(messages(), result.messages.clone()));
                                                            if let Some(mut current_detail) = detail() {
                                                                if let Some(updated) = result.messages.iter().find(|message| message.id == current_detail.message.id) {
                                                                    current_detail.message = updated.clone();
                                                                    detail.set(Some(current_detail));
                                                                }
                                                            }
                                                        }
                                                        selected_message_ids.set(retain_loaded_selection(selected_message_ids(), &messages()));
                                                        let account_id_for_refresh = selected_account.peek().clone();
                                                        if let Ok(rows) = use_mail::list_folders(&account_id_for_refresh).await {
                                                            folders.set(rows);
                                                        }
                                                        if result.failed > 0 {
                                                            error.set(Some(format!("{} message action(s) failed", result.failed)));
                                                        } else {
                                                            notice.set(Some(bulk_action_success_label(action, result.succeeded)));
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                bulk_mutating_messages.set(false);
                                            });
                                        },
                                        IconEye { class: "h-4 w-4".to_string() }
                                        span { "Read" }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                        disabled: bulk_mutating_messages()
                                            || selected_message_count == 0
                                            || !has_archive_target_for_active_folder,
                                        onclick: move |_| {
                                            let ids = selected_message_ids();
                                            spawn(async move {
                                                bulk_mutating_messages.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let action = MailMessageMutationAction::MarkUnread;
                                                match use_mail::bulk_mutate_messages(ids, action, None).await {
                                                    Ok(result) => {
                                                        if !result.messages.is_empty() {
                                                            messages.set(replace_messages(messages(), result.messages.clone()));
                                                            if let Some(mut current_detail) = detail() {
                                                                if let Some(updated) = result.messages.iter().find(|message| message.id == current_detail.message.id) {
                                                                    current_detail.message = updated.clone();
                                                                    detail.set(Some(current_detail));
                                                                }
                                                            }
                                                        }
                                                        selected_message_ids.set(retain_loaded_selection(selected_message_ids(), &messages()));
                                                        let account_id_for_refresh = selected_account.peek().clone();
                                                        if let Ok(rows) = use_mail::list_folders(&account_id_for_refresh).await {
                                                            folders.set(rows);
                                                        }
                                                        if result.failed > 0 {
                                                            error.set(Some(format!("{} message action(s) failed", result.failed)));
                                                        } else {
                                                            notice.set(Some(bulk_action_success_label(action, result.succeeded)));
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                bulk_mutating_messages.set(false);
                                            });
                                        },
                                        IconEye { class: "h-4 w-4".to_string() }
                                        span { "Unread" }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                        disabled: bulk_mutating_messages() || selected_message_count == 0,
                                        onclick: move |_| {
                                            let ids = selected_message_ids();
                                            spawn(async move {
                                                bulk_mutating_messages.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let action = MailMessageMutationAction::Archive;
                                                match use_mail::bulk_mutate_messages(ids, action, None).await {
                                                    Ok(result) => {
                                                        if !result.removed_message_ids.is_empty() {
                                                            messages.set(remove_messages(messages(), &result.removed_message_ids));
                                                            if result.removed_message_ids.iter().any(|id| id == &selected_message.peek().clone()) {
                                                                selected_message.set(String::new());
                                                                detail.set(None);
                                                                mobile_mail_pane.set(MailMobilePane::Messages);
                                                            }
                                                        }
                                                        selected_message_ids.set(retain_loaded_selection(selected_message_ids(), &messages()));
                                                        let account_id_for_refresh = selected_account.peek().clone();
                                                        if let Ok(rows) = use_mail::list_folders(&account_id_for_refresh).await {
                                                            folders.set(rows);
                                                        }
                                                        if result.failed > 0 {
                                                            error.set(Some(format!("{} message action(s) failed", result.failed)));
                                                        } else {
                                                            notice.set(Some(bulk_action_success_label(action, result.succeeded)));
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                bulk_mutating_messages.set(false);
                                            });
                                        },
                                        IconArchive { class: "h-4 w-4".to_string() }
                                        span { "Archive" }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1 text-error",
                                        disabled: bulk_mutating_messages()
                                            || selected_message_count == 0
                                            || !has_trash_target_for_active_folder,
                                        onclick: move |_| {
                                            let ids = selected_message_ids();
                                            spawn(async move {
                                                bulk_mutating_messages.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let action = MailMessageMutationAction::Trash;
                                                match use_mail::bulk_mutate_messages(ids, action, None).await {
                                                    Ok(result) => {
                                                        if !result.removed_message_ids.is_empty() {
                                                            messages.set(remove_messages(messages(), &result.removed_message_ids));
                                                            if result.removed_message_ids.iter().any(|id| id == &selected_message.peek().clone()) {
                                                                selected_message.set(String::new());
                                                                detail.set(None);
                                                                mobile_mail_pane.set(MailMobilePane::Messages);
                                                            }
                                                        }
                                                        selected_message_ids.set(retain_loaded_selection(selected_message_ids(), &messages()));
                                                        let account_id_for_refresh = selected_account.peek().clone();
                                                        if let Ok(rows) = use_mail::list_folders(&account_id_for_refresh).await {
                                                            folders.set(rows);
                                                        }
                                                        if result.failed > 0 {
                                                            error.set(Some(format!("{} message action(s) failed", result.failed)));
                                                        } else {
                                                            notice.set(Some(bulk_action_success_label(action, result.succeeded)));
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                bulk_mutating_messages.set(false);
                                            });
                                        },
                                        IconTrash { class: "h-4 w-4".to_string() }
                                        span { "Trash" }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 px-2",
                                        disabled: bulk_mutating_messages(),
                                        onclick: move |_| {
                                            selecting_messages.set(false);
                                            selected_message_ids.set(Vec::new());
                                        },
                                        "Done"
                                    }
                                }
                            } else {
                                div { class: "flex items-center gap-1",
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 px-2",
                                        disabled: messages_snapshot.is_empty() || bulk_mutating_messages(),
                                        onclick: move |_| {
                                            selecting_messages.set(true);
                                            selected_message_ids.set(Vec::new());
                                        },
                                        "Select"
                                    }
                                    div { class: "dropdown dropdown-end",
                                        button {
                                            class: "btn btn-ghost btn-sm h-8 min-h-8 w-8 p-0",
                                            title: "More actions",
                                            disabled: selected_account_id.is_empty() || bulk_mutating_messages(),
                                            tabindex: "0",
                                            IconMoreVertical { class: "h-4 w-4".to_string() }
                                        }
                                        ul {
                                            class: "menu dropdown-content bg-base-100 rounded-box z-50 mt-2 w-52 border border-base-300 p-1 shadow",
                                            tabindex: "0",
                                            li {
                                                button {
                                                    disabled: selected_folder_id.is_empty() || bulk_mutating_messages(),
                                                    onclick: move |_| {
                                                        let account_id = selected_account();
                                                        let folder_id = selected_folder();
                                                        if account_id.is_empty() || folder_id.is_empty() {
                                                            return;
                                                        }
                                                        spawn(async move {
                                                            bulk_mutating_messages.set(true);
                                                            sync_status.set(Some("Marking folder read".to_string()));
                                                            error.set(None);
                                                            notice.set(None);
                                                            match use_mail::mark_folder_read(&account_id, &folder_id).await {
                                                                Ok(result) => {
                                                                    folders.set(result.folders);
                                                                    messages.set(mark_messages_read(messages()));
                                                                    if let Some(mut current_detail) = detail() {
                                                                        current_detail.message = mark_message_read(current_detail.message);
                                                                        detail.set(Some(current_detail));
                                                                    }
                                                                    if result.failed > 0 {
                                                                        error.set(Some(format!("{} folder action(s) failed", result.failed)));
                                                                    } else {
                                                                        notice.set(Some("Folder marked as read".to_string()));
                                                                    }
                                                                }
                                                                Err(e) => error.set(Some(e)),
                                                            }
                                                            sync_status.set(None);
                                                            bulk_mutating_messages.set(false);
                                                        });
                                                    },
                                                    "Mark folder as read"
                                                }
                                            }
                                            li {
                                                button {
                                                    disabled: selected_account_id.is_empty() || bulk_mutating_messages(),
                                                    onclick: move |_| {
                                                        let account_id = selected_account();
                                                        if account_id.is_empty() {
                                                            return;
                                                        }
                                                        spawn(async move {
                                                            bulk_mutating_messages.set(true);
                                                            sync_status.set(Some("Marking account read".to_string()));
                                                            error.set(None);
                                                            notice.set(None);
                                                            match use_mail::mark_account_read(&account_id).await {
                                                                Ok(result) => {
                                                                    folders.set(result.folders);
                                                                    messages.set(mark_messages_read(messages()));
                                                                    if let Some(mut current_detail) = detail() {
                                                                        current_detail.message = mark_message_read(current_detail.message);
                                                                        detail.set(Some(current_detail));
                                                                    }
                                                                    if result.failed > 0 {
                                                                        error.set(Some(format!("{} folder action(s) failed", result.failed)));
                                                                    } else {
                                                                        notice.set(Some("Account marked as read".to_string()));
                                                                    }
                                                                }
                                                                Err(e) => error.set(Some(e)),
                                                            }
                                                            sync_status.set(None);
                                                            bulk_mutating_messages.set(false);
                                                        });
                                                    },
                                                    "Mark account as read"
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn btn-ghost btn-sm h-8 min-h-8 w-8 p-0",
                                        title: "Folder settings",
                                        disabled: active_folder.is_none(),
                                        onclick: {
                                            let active_folder = active_folder.clone();
                                            move |_| {
                                                if let Some(folder) = active_folder.clone() {
                                                    folder_role_value.set(folder_role_value_for(&folder));
                                                    folder_sync_enabled.set(folder.sync_enabled);
                                                    settings_folder.set(Some(folder));
                                                }
                                            }
                                        },
                                        IconSettings { class: "h-4 w-4".to_string() }
                                    }
                                    button {
                                        class: "btn btn-sm btn-outline gap-2",
                                        disabled: selected_account_id.is_empty()
                                            || selected_folder_id.is_empty()
                                            || syncing()
                                            || refreshing_folders(),
                                        onclick: move |_| {
                                            let account_id = selected_account();
                                            let folder_id = selected_folder();
                                            if account_id.is_empty() || folder_id.is_empty() {
                                                return;
                                            }
                                            spawn(async move {
                                                syncing.set(true);
                                                sync_status.set(Some("Syncing folder".to_string()));
                                                error.set(None);
                                                match use_mail::sync_folder(&account_id, &folder_id, Some(50)).await {
                                                    Ok(result) => {
                                                        if let Some(message) = result.error {
                                                            error.set(Some(message));
                                                        }
                                                        if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                            folders.set(rows);
                                                        }
                                                        if let Ok(page) = use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None).await {
                                                            let selected = selected_message.peek().clone();
                                                            let still_selected = page.messages.iter().any(|message| message.id == selected);
                                                            messages.set(page.messages);
                                                            message_next_cursor.set(page.next_cursor);
                                                            message_has_more.set(page.has_more);
                                                            if !selected.is_empty() && !still_selected {
                                                                selected_message.set(String::new());
                                                                detail.set(None);
                                                            }
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                syncing.set(false);
                                                sync_status.set(None);
                                            });
                                        },
                                        if syncing() {
                                            span { class: "loading loading-spinner loading-xs" }
                                        } else {
                                            IconRefreshCw { class: "w-4 h-4".to_string() }
                                        }
                                        span { "Sync" }
                                    }
                                }
                            }
                        }
                        div { class: "min-h-0 flex-1 overflow-y-auto",
                            if messages_snapshot.is_empty() {
                                div { class: "flex h-52 flex-col items-center justify-center gap-3 px-4 text-sm text-base-content/60",
                                    div { "No cached messages" }
                                    if can_backfill_active_folder {
                                        button {
                                            class: "btn btn-sm btn-outline",
                                            disabled: syncing() || backfilling_messages(),
                                            onclick: move |_| trigger_load_more_messages(),
                                            if backfilling_messages() {
                                                span { class: "loading loading-spinner loading-xs" }
                                                span { "Syncing messages" }
                                            } else {
                                                span { "Sync messages" }
                                            }
                                        }
                                    }
                                }
                            }
                            for message in messages_snapshot.clone() {
                                {
                                    let id = message.id.clone();
                                    let active = id == selected_message_id;
                                    let selected_for_bulk =
                                        message_id_selected(&selected_message_ids_snapshot, &id);
                                    let is_seen = message_has_flag(&message, "\\Seen");
                                    let is_flagged = message_has_flag(&message, "\\Flagged");
                                    let subject_class = if is_seen {
                                        "min-w-0 flex-1 truncate text-sm font-medium"
                                    } else {
                                        "min-w-0 flex-1 truncate text-sm font-semibold"
                                    };
                                    rsx! {
                                        button {
                                            key: "{id}",
                                            class: if selected_for_bulk {
                                                "block w-full border-l-4 border-primary bg-primary/10 px-3 py-3 text-left"
                                            } else if active {
                                                "block w-full border-l-4 border-primary bg-primary/10 px-3 py-3 text-left"
                                            } else if !is_seen {
                                                "block w-full border-l-4 border-transparent bg-base-200/50 px-3 py-3 text-left hover:bg-base-200"
                                            } else {
                                                "block w-full border-l-4 border-transparent px-3 py-3 text-left hover:bg-base-200"
                                            },
                                            onclick: move |_| {
                                                let message_id = id.clone();
                                                if *selecting_messages.peek() {
                                                    selected_message_ids.set(toggle_message_id(
                                                        selected_message_ids(),
                                                        &message_id,
                                                    ));
                                                    return;
                                                }
                                                mobile_mail_pane.set(MailMobilePane::Detail);
                                                spawn(async move {
                                                    selected_message.set(message_id.clone());
                                                    move_target_folder.set(String::new());
                                                    loading_detail.set(true);
                                                    error.set(None);
                                                    match use_mail::get_message(&message_id).await {
                                                        Ok(row) => {
                                                            let updated_message = row.message.clone();
                                                            let mut current = messages();
                                                            for item in &mut current {
                                                                if item.id == updated_message.id {
                                                                    *item = updated_message.clone();
                                                                    break;
                                                                }
                                                            }
                                                            messages.set(current);
                                                            detail.set(Some(row));
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                    loading_detail.set(false);
                                                });
                                            },
                                            div { class: "flex items-start justify-between gap-2",
                                                if selecting_messages_snapshot {
                                                    span {
                                                        class: if selected_for_bulk {
                                                            "mt-0.5 flex h-4 w-4 items-center justify-center rounded border border-primary bg-primary text-primary-content"
                                                        } else {
                                                            "mt-0.5 h-4 w-4 rounded border border-base-content/30 bg-base-100"
                                                        },
                                                        if selected_for_bulk {
                                                            IconCheck { class: "h-3 w-3".to_string() }
                                                        }
                                                    }
                                                }
                                                div { class: "min-w-0 flex-1",
                                                    div { class: "flex min-w-0 items-center gap-2",
                                                        if !is_seen {
                                                            span { class: "h-2 w-2 rounded-full bg-primary", title: "Unread" }
                                                        }
                                                        if is_flagged {
                                                            IconStar { class: "h-3.5 w-3.5 text-warning".to_string() }
                                                        }
                                                        div { class: "{subject_class}", "{message_subject(&message)}" }
                                                    }
                                                    div { class: "truncate text-xs text-base-content/60", "{message_sender(&message)}" }
                                                }
                                                div { class: "flex shrink-0 items-center gap-1 text-xs text-base-content/50",
                                                    if message.has_attachments {
                                                        IconPaperclip { class: "h-3.5 w-3.5".to_string() }
                                                    }
                                                    span { "{short_date(message.internal_date.as_deref().or(message.date.as_deref()))}" }
                                                }
                                            }
                                            div { class: "mt-1 line-clamp-2 text-xs text-base-content/60",
                                                "{message.snippet.clone().unwrap_or_default()}"
                                            }
                                        }
                                    }
                                }
                            }
                            if !messages_snapshot.is_empty() {
                                if loading_more_messages() {
                                    div { class: "flex items-center justify-center gap-2 border-t border-base-300 px-3 py-3 text-xs text-base-content/60",
                                        span { class: "loading loading-spinner loading-xs" }
                                        span { "Loading cached messages" }
                                    }
                                } else if backfilling_messages() {
                                    div { class: "flex items-center justify-center gap-2 border-t border-base-300 px-3 py-3 text-xs text-base-content/60",
                                        span { class: "loading loading-spinner loading-xs" }
                                        span { "Syncing older messages" }
                                    }
                                } else if message_has_more() || can_backfill_active_folder {
                                    ScrollSentinel { on_visible: move |_| trigger_load_more_messages() }
                                    div { class: "flex justify-center border-t border-base-300 px-3 py-3",
                                        button {
                                            class: "btn btn-sm btn-ghost",
                                            disabled: syncing(),
                                            onclick: move |_| trigger_load_more_messages(),
                                            if message_has_more() {
                                                "Load more"
                                            } else {
                                                "Sync older messages"
                                            }
                                        }
                                    }
                                } else if active_folder.is_some() {
                                    div { class: "border-t border-base-300 px-3 py-3 text-center text-xs text-base-content/50",
                                        "No more messages"
                                    }
                                }
                            }
                        }
                    }

                    section { class: "{mail_detail_pane_class(mobile_mail_pane_snapshot)}",
                        if show_compose() {
                            if let Some(account) = active_account.clone() {
                                {
                                    let account_id = account.id.clone();
                                    let account_from_label =
                                        format!("{} <{}>", account.display_name, account.email_address);
                                    let active_identities = active_identities.clone();
                                    let sender_select_value = if compose_identity_id().trim().is_empty()
                                        && !active_default_identity_id.is_empty()
                                    {
                                        active_default_identity_id.clone()
                                    } else {
                                        compose_identity_id()
                                    };
                                    let compose_title = compose_title(compose_mode());
                                    let draft_status = if saving_draft() {
                                        "Saving draft"
                                    } else if uploading_draft_attachment() {
                                        "Updating attachments"
                                    } else if compose_draft_id().is_empty() {
                                        ""
                                    } else {
                                        "Draft saved"
                                    };
                                    let can_discard = !compose_draft_id().is_empty()
                                        || !compose_to().trim().is_empty()
                                        || !compose_subject().trim().is_empty()
                                        || !compose_body().trim().is_empty()
                                        || !compose_body_html().trim().is_empty()
                                        || !compose_attachments().is_empty();
                                    let on_attachment_pick = {
                                        let account_id = account_id.clone();
                                        move |_: Event<FormData>| {
                                            let account_id = account_id.clone();
                                            let file_list = web_sys::window()
                                                .and_then(|w| w.document())
                                                .and_then(|d| d.get_element_by_id(MAIL_COMPOSE_ATTACHMENT_INPUT_ID))
                                                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                                                .and_then(|i| i.files());
                                            let Some(file_list) = file_list else { return };
                                            if file_list.length() == 0 {
                                                return;
                                            }
                                            let files: Vec<web_sys::File> = (0..file_list.length())
                                                .filter_map(|i| file_list.item(i))
                                                .collect();
                                            let draft_id = compose_draft_id();
                                            let identity_id = compose_identity_id();
                                            let mode = compose_mode();
                                            let source_message_id = compose_source_message_id();
                                            let to = compose_to();
                                            let cc = compose_cc();
                                            let bcc = compose_bcc();
                                            let subject = compose_subject();
                                            let body = compose_body();
                                            let body_html = compose_body_html();
                                            let in_reply_to = compose_in_reply_to();
                                            let references = compose_references();
                                            spawn(async move {
                                                uploading_draft_attachment.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let mut active_draft_id = draft_id;
                                                saving_draft.set(true);
                                                let req = UpsertMailDraftRequest {
                                                    identity_id: nonempty_string(identity_id),
                                                    mode,
                                                    source_message_id: nonempty_string(source_message_id),
                                                    to: parse_compose_addresses(&to),
                                                    cc: parse_compose_addresses(&cc),
                                                    bcc: parse_compose_addresses(&bcc),
                                                    subject,
                                                    body_text: body,
                                                    body_html: nonempty_string(body_html),
                                                    in_reply_to: nonempty_string(in_reply_to),
                                                    references,
                                                };
                                                let saved_result = if active_draft_id.trim().is_empty() {
                                                    use_mail::create_draft(&account_id, &req).await
                                                } else {
                                                    use_mail::update_draft(&active_draft_id, &req).await
                                                };
                                                match saved_result {
                                                    Ok(saved) => {
                                                        active_draft_id = saved.id.clone();
                                                        compose_draft_id.set(saved.id.clone());
                                                        compose_attachments.set(saved.attachments.clone());
                                                        drafts.set(upsert_draft(drafts(), saved));
                                                    }
                                                    Err(e) => {
                                                        error.set(Some(e));
                                                        saving_draft.set(false);
                                                        uploading_draft_attachment.set(false);
                                                        reset_compose_attachment_input();
                                                        return;
                                                    }
                                                }
                                                saving_draft.set(false);

                                                for file in files {
                                                    match use_mail::upload_draft_attachment(&active_draft_id, &file).await {
                                                        Ok(attachment) => {
                                                            let mut current = compose_attachments();
                                                            current.push(attachment);
                                                            compose_attachments.set(current);
                                                        }
                                                        Err(e) => {
                                                            error.set(Some(e));
                                                            break;
                                                        }
                                                    }
                                                }
                                                if let Ok(rows) = use_mail::list_drafts(&account_id).await {
                                                    drafts.set(rows);
                                                }
                                                uploading_draft_attachment.set(false);
                                                reset_compose_attachment_input();
                                            });
                                        }
                                    };
                                    rsx! {
                                        div { class: "flex min-h-0 flex-1 flex-col",
                                            div { class: "border-b border-base-300 px-4 py-3",
                                                div { class: "flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between",
                                                    div { class: "flex min-w-0 items-start gap-2",
                                                        button {
                                                            class: "btn btn-ghost btn-sm h-8 min-h-8 px-2 lg:hidden",
                                                            onclick: move |_| {
                                                                show_compose.set(false);
                                                                if selected_folder.peek().is_empty() {
                                                                    mobile_mail_pane.set(MailMobilePane::Folders);
                                                                } else {
                                                                    mobile_mail_pane.set(MailMobilePane::Messages);
                                                                }
                                                            },
                                                            IconChevronRight { class: "h-4 w-4 rotate-180".to_string() }
                                                            span { "Messages" }
                                                        }
                                                        div { class: "min-w-0",
                                                            div { class: "text-lg font-semibold", "{compose_title}" }
                                                            div { class: "mt-1 truncate text-sm text-base-content/60", "{account.email_address}" }
                                                        }
                                                    }
                                                    div { class: "flex flex-wrap items-center gap-2",
                                                        if !draft_status.is_empty() {
                                                            span { class: "text-xs text-base-content/50", "{draft_status}" }
                                                        }
                                                        button {
                                                            class: "btn btn-ghost btn-sm h-8 min-h-8",
                                                            disabled: sending_message(),
                                                            onclick: move |_| {
                                                                show_compose.set(false);
                                                                if selected_message.peek().is_empty() {
                                                                    if selected_folder.peek().is_empty() {
                                                                        mobile_mail_pane.set(MailMobilePane::Folders);
                                                                    } else {
                                                                        mobile_mail_pane.set(MailMobilePane::Messages);
                                                                    }
                                                                }
                                                            },
                                                            "Close"
                                                        }
                                                    }
                                                }
                                            }

                                            div { class: "min-h-0 flex-1 overflow-y-auto p-4",
                                                div { class: "grid gap-3",
                                                    label { class: "form-control",
                                                        span { class: "label-text", "From" }
                                                        select {
                                                            class: "select select-bordered",
                                                            value: "{sender_select_value}",
                                                            onchange: move |e| compose_identity_id.set(e.value()),
                                                            if active_identities.is_empty() {
                                                                option { value: "", "{account_from_label}" }
                                                            }
                                                            for identity in active_identities.clone() {
                                                                {
                                                                    let identity_label = mail_identity_label(&identity);
                                                                    rsx! {
                                                                        option {
                                                                            value: "{identity.id}",
                                                                            "{identity_label}"
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    label { class: "form-control",
                                                        span { class: "label-text", "To" }
                                                        input {
                                                            class: "input input-bordered",
                                                            value: "{compose_to()}",
                                                            placeholder: "one@example.com, two@example.com",
                                                            oninput: move |e| compose_to.set(e.value()),
                                                        }
                                                    }
                                                    div { class: "grid gap-3 md:grid-cols-2",
                                                        label { class: "form-control",
                                                            span { class: "label-text", "Cc" }
                                                            input {
                                                                class: "input input-bordered",
                                                                value: "{compose_cc()}",
                                                                oninput: move |e| compose_cc.set(e.value()),
                                                            }
                                                        }
                                                        label { class: "form-control",
                                                            span { class: "label-text", "Bcc" }
                                                            input {
                                                                class: "input input-bordered",
                                                                value: "{compose_bcc()}",
                                                                oninput: move |e| compose_bcc.set(e.value()),
                                                            }
                                                        }
                                                    }
                                                    label { class: "form-control",
                                                        span { class: "label-text", "Subject" }
                                                        input {
                                                            class: "input input-bordered",
                                                            value: "{compose_subject()}",
                                                            oninput: move |e| compose_subject.set(e.value()),
                                                        }
                                                    }
                                                    div {
                                                        span { class: "label-text", "Message" }
                                                        div { class: "mt-1 overflow-hidden rounded border border-base-300 bg-base-100",
                                                            input {
                                                                id: MAIL_COMPOSE_ATTACHMENT_INPUT_ID,
                                                                class: "hidden",
                                                                r#type: "file",
                                                                multiple: true,
                                                                onchange: on_attachment_pick,
                                                            }
                                                            div { class: "flex flex-wrap items-center gap-1 border-b border-base-300 bg-base-200/50 px-2 py-1.5",
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0",
                                                                    title: "Bold",
                                                                    onclick: move |_| run_mail_editor_command("bold"),
                                                                    span { class: "font-bold", "B" }
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0",
                                                                    title: "Italic",
                                                                    onclick: move |_| run_mail_editor_command("italic"),
                                                                    span { class: "italic", "I" }
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0",
                                                                    title: "Underline",
                                                                    onclick: move |_| run_mail_editor_command("underline"),
                                                                    span { class: "underline", "U" }
                                                                }
                                                                div { class: "mx-1 h-5 w-px bg-base-300" }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Bulleted list",
                                                                    onclick: move |_| run_mail_editor_command("bulletList"),
                                                                    "UL"
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Numbered list",
                                                                    onclick: move |_| run_mail_editor_command("orderedList"),
                                                                    "OL"
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Quote",
                                                                    onclick: move |_| run_mail_editor_command("blockquote"),
                                                                    "Quote"
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Link",
                                                                    onclick: move |_| run_mail_editor_command("link"),
                                                                    "Link"
                                                                }
                                                                div { class: "mx-1 h-5 w-px bg-base-300" }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Undo",
                                                                    onclick: move |_| run_mail_editor_command("undo"),
                                                                    "Undo"
                                                                }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                    title: "Redo",
                                                                    onclick: move |_| run_mail_editor_command("redo"),
                                                                    "Redo"
                                                                }
                                                                div { class: "mx-1 h-5 w-px bg-base-300" }
                                                                button {
                                                                    class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0",
                                                                    title: "Attach files",
                                                                    disabled: sending_message() || saving_draft() || uploading_draft_attachment(),
                                                                    onclick: move |_| click_compose_attachment_input(),
                                                                    if uploading_draft_attachment() {
                                                                        span { class: "loading loading-spinner loading-xs" }
                                                                    } else {
                                                                        IconPaperclip { class: "h-4 w-4".to_string() }
                                                                    }
                                                                }
                                                            }
                                                            div {
                                                                id: MAIL_COMPOSE_EDITOR_ID,
                                                                class: "uc-mail-editor-shell bg-base-100",
                                                            }
                                                        }
                                                    }
                                                    if !compose_attachments().is_empty() {
                                                        div { class: "rounded border border-base-300 bg-base-200/30 p-2",
                                                            div { class: "mb-1 text-xs font-semibold uppercase text-base-content/50", "Attachments" }
                                                            div { class: "grid gap-1",
                                                                for attachment in compose_attachments() {
                                                                    {
                                                                        let attachment_id = attachment.id.clone();
                                                                        let draft_id = attachment.draft_id.clone();
                                                                        let filename = attachment.filename.clone();
                                                                        let meta = mail_draft_attachment_meta(&attachment);
                                                                        rsx! {
                                                                            div {
                                                                                key: "{attachment_id}",
                                                                                class: "flex items-center gap-2 rounded bg-base-100 px-2 py-1.5 text-sm",
                                                                                IconPaperclip { class: "h-4 w-4 shrink-0 text-base-content/50".to_string() }
                                                                                div { class: "min-w-0 flex-1",
                                                                                    div { class: "truncate font-medium", "{filename}" }
                                                                                    div { class: "truncate text-xs text-base-content/50", "{meta}" }
                                                                                }
                                                                                button {
                                                                                    class: "btn btn-ghost btn-xs h-7 min-h-7 w-7 p-0",
                                                                                    title: "Remove attachment",
                                                                                    disabled: sending_message() || uploading_draft_attachment(),
                                                                                    onclick: move |_| {
                                                                                        let draft_id = draft_id.clone();
                                                                                        let attachment_id = attachment_id.clone();
                                                                                        spawn(async move {
                                                                                            uploading_draft_attachment.set(true);
                                                                                            error.set(None);
                                                                                            match use_mail::delete_draft_attachment(&draft_id, &attachment_id).await {
                                                                                                Ok(()) => {
                                                                                                    compose_attachments.set(
                                                                                                        compose_attachments()
                                                                                                            .into_iter()
                                                                                                            .filter(|attachment| attachment.id != attachment_id)
                                                                                                            .collect(),
                                                                                                    );
                                                                                                }
                                                                                                Err(e) => error.set(Some(e)),
                                                                                            }
                                                                                            uploading_draft_attachment.set(false);
                                                                                        });
                                                                                    },
                                                                                    IconX { class: "h-4 w-4".to_string() }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            div { class: "flex flex-wrap items-center justify-end gap-2 border-t border-base-300 px-4 py-3",
                                                if can_discard {
                                                    button {
                                                        class: "btn btn-ghost text-error",
                                                        disabled: sending_message() || saving_draft() || uploading_draft_attachment(),
                                                        onclick: move |_| {
                                                            let draft_id = compose_draft_id();
                                                            spawn(async move {
                                                                error.set(None);
                                                                if !draft_id.trim().is_empty() {
                                                                    match use_mail::delete_draft(&draft_id).await {
                                                                        Ok(()) => {
                                                                            drafts.set(
                                                                                drafts()
                                                                                    .into_iter()
                                                                                    .filter(|draft| draft.id != draft_id)
                                                                                    .collect(),
                                                                            );
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(e));
                                                                            return;
                                                                        }
                                                                    }
                                                                }
                                                                compose_draft_id.set(String::new());
                                                                compose_body_html.set(String::new());
                                                                compose_attachments.set(Vec::new());
                                                                show_compose.set(false);
                                                                if selected_folder.peek().is_empty() {
                                                                    mobile_mail_pane.set(MailMobilePane::Folders);
                                                                } else {
                                                                    mobile_mail_pane.set(MailMobilePane::Messages);
                                                                }
                                                            });
                                                        },
                                                        "Discard"
                                                    }
                                                }
                                                button {
                                                    class: "btn",
                                                    disabled: sending_message() || saving_draft() || uploading_draft_attachment(),
                                                    onclick: {
                                                        let account_id = account_id.clone();
                                                        move |_| {
                                                            let account_id = account_id.clone();
                                                            let draft_id = compose_draft_id();
                                                            let identity_id = compose_identity_id();
                                                            let mode = compose_mode();
                                                            let source_message_id = compose_source_message_id();
                                                            let to = compose_to();
                                                            let cc = compose_cc();
                                                            let bcc = compose_bcc();
                                                            let subject = compose_subject();
                                                            let body = compose_body();
                                                            let body_html = compose_body_html();
                                                            let in_reply_to = compose_in_reply_to();
                                                            let references = compose_references();
                                                            spawn(async move {
                                                                saving_draft.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                let req = UpsertMailDraftRequest {
                                                                    identity_id: nonempty_string(identity_id),
                                                                    mode,
                                                                    source_message_id: nonempty_string(source_message_id),
                                                                    to: parse_compose_addresses(&to),
                                                                    cc: parse_compose_addresses(&cc),
                                                                    bcc: parse_compose_addresses(&bcc),
                                                                    subject,
                                                                    body_text: body,
                                                                    body_html: nonempty_string(body_html),
                                                                    in_reply_to: nonempty_string(in_reply_to),
                                                                    references,
                                                                };
                                                                match if draft_id.trim().is_empty() {
                                                                    use_mail::create_draft(&account_id, &req).await
                                                                } else {
                                                                    use_mail::update_draft(&draft_id, &req).await
                                                                } {
                                                                    Ok(saved) => {
                                                                        compose_draft_id.set(saved.id.clone());
                                                                        compose_attachments.set(saved.attachments.clone());
                                                                        drafts.set(upsert_draft(drafts(), saved));
                                                                        notice.set(Some("Draft saved".to_string()));
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                saving_draft.set(false);
                                                            });
                                                        }
                                                    },
                                                    if saving_draft() {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    }
                                                    "Save draft"
                                                }
                                                button {
                                                    class: "btn btn-primary gap-2",
                                                    disabled: sending_message() || saving_draft() || uploading_draft_attachment(),
                                                    onclick: {
                                                        let account_id = account_id.clone();
                                                        move |_| {
                                                            let account_id = account_id.clone();
                                                            let identity_id = compose_identity_id();
                                                            let draft_id = compose_draft_id();
                                                            let to = compose_to();
                                                            let cc = compose_cc();
                                                            let bcc = compose_bcc();
                                                            let subject = compose_subject();
                                                            let body = compose_body();
                                                            let body_html = compose_body_html();
                                                            let in_reply_to = compose_in_reply_to();
                                                            let references = compose_references();
                                                            let attachment_ids = compose_attachments()
                                                                .into_iter()
                                                                .map(|attachment| attachment.id)
                                                                .collect();
                                                            spawn(async move {
                                                                sending_message.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                let req = SendMailMessageRequest {
                                                                    identity_id: if identity_id.trim().is_empty() {
                                                                        None
                                                                    } else {
                                                                        Some(identity_id)
                                                                    },
                                                                    draft_id: nonempty_string(draft_id.clone()),
                                                                    to: parse_compose_addresses(&to),
                                                                    cc: parse_compose_addresses(&cc),
                                                                    bcc: parse_compose_addresses(&bcc),
                                                                    subject,
                                                                    body_text: body,
                                                                    body_html: nonempty_string(body_html),
                                                                    in_reply_to: nonempty_string(in_reply_to),
                                                                    references,
                                                                    attachment_ids,
                                                                };
                                                                match use_mail::send_message(&account_id, &req).await {
                                                                    Ok(sent) => {
                                                                        notice.set(Some(format!(
                                                                            "Message sent ({})",
                                                                            sent_copy_status_label(sent.sent_copy_status),
                                                                        )));
                                                                        if !draft_id.trim().is_empty() {
                                                                            drafts.set(
                                                                                drafts()
                                                                                    .into_iter()
                                                                                    .filter(|draft| draft.id != draft_id)
                                                                                    .collect(),
                                                                            );
                                                                        }
                                                                        compose_draft_id.set(String::new());
                                                                        compose_body_html.set(String::new());
                                                                        compose_attachments.set(Vec::new());
                                                                        show_compose.set(false);
                                                                        if selected_message.peek().is_empty() {
                                                                            if selected_folder.peek().is_empty() {
                                                                                mobile_mail_pane.set(MailMobilePane::Folders);
                                                                            } else {
                                                                                mobile_mail_pane.set(MailMobilePane::Messages);
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                sending_message.set(false);
                                                            });
                                                        }
                                                    },
                                                    if sending_message() {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    } else {
                                                        IconSend { class: "h-4 w-4".to_string() }
                                                    }
                                                    span { "Send" }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                div { class: "flex flex-1 flex-col items-center justify-center gap-2 text-base-content/60",
                                    IconMail { class: "h-8 w-8".to_string() }
                                    div { class: "text-sm", "Select an account to compose" }
                                }
                            }
                        } else if loading_detail() {
                            div { class: "flex items-center gap-2 border-b border-base-300 px-4 py-3 lg:hidden",
                                button {
                                    class: "btn btn-ghost btn-sm h-8 min-h-8 px-2",
                                    onclick: move |_| mobile_mail_pane.set(MailMobilePane::Messages),
                                    IconChevronRight { class: "h-4 w-4 rotate-180".to_string() }
                                    span { "Messages" }
                                }
                            }
                            div { class: "flex flex-1 items-center justify-center",
                                span { class: "loading loading-spinner loading-md" }
                            }
                        } else if let Some(row) = detail_snapshot {
                            {
                                let is_seen = message_has_flag(&row.message, "\\Seen");
                                let is_flagged = message_has_flag(&row.message, "\\Flagged");
                                let has_archive_target = folders_snapshot.iter().any(|folder| {
                                    folder.selectable
                                        && folder.id != row.message.folder_id
                                        && matches!(folder.role, Some(MailFolderRole::Archive | MailFolderRole::AllMail))
                                });
                                let has_trash_target = folders_snapshot.iter().any(|folder| {
                                    folder.selectable
                                        && folder.id != row.message.folder_id
                                        && folder.role == Some(MailFolderRole::Trash)
                                });
                                let message_id = row.message.id.clone();
                                let account_id = row.message.account_id.clone();
                                let seen_label = if is_seen { "Unread" } else { "Read" };
                                let star_label = if is_flagged { "Unstar" } else { "Star" };
                                let own_addresses =
                                    mail_own_addresses(active_account.as_ref(), &active_identities);
                                rsx! {
                                    div { class: "border-b border-base-300 px-4 py-3",
                                        div { class: "flex flex-col gap-3",
                                            div { class: "flex min-w-0 items-start gap-2",
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 px-2 lg:hidden",
                                                    onclick: move |_| mobile_mail_pane.set(MailMobilePane::Messages),
                                                    IconChevronRight { class: "h-4 w-4 rotate-180".to_string() }
                                                    span { "Messages" }
                                                }
                                                div { class: "min-w-0",
                                                    div { class: "text-lg font-semibold", "{message_subject(&row.message)}" }
                                                    div { class: "mt-2 grid gap-1 text-sm text-base-content/70",
                                                        div { "From: {message_sender(&row.message)}" }
                                                        div { "To: {message_recipients(&row.message)}" }
                                                        div { "Date: {short_date(row.message.internal_date.as_deref().or(row.message.date.as_deref()))}" }
                                                    }
                                                }
                                            }
                                            div { class: "order-first flex flex-wrap items-center gap-1",
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                                    title: "Reply",
                                                    onclick: {
                                                        let row = row.clone();
                                                        move |_| {
                                                            compose_draft_id.set(String::new());
                                                            compose_mode.set(MailComposeMode::Reply);
                                                            compose_source_message_id.set(row.message.id.clone());
                                                            compose_identity_id.set(default_mail_identity_id_for_account(
                                                                &identities.peek(),
                                                                &row.message.account_id,
                                                            ));
                                                            compose_to.set(compose_address_line(&row.message.from));
                                                            compose_cc.set(String::new());
                                                            compose_bcc.set(String::new());
                                                            compose_subject.set(reply_subject(&message_subject(&row.message)));
                                                            compose_body.set(reply_body(&row));
                                                            compose_body_html.set(reply_body_html(&row));
                                                            compose_in_reply_to.set(
                                                                row.message.message_id.clone().unwrap_or_default(),
                                                            );
                                                            compose_references.set(reply_references(&row.message));
                                                            compose_attachments.set(Vec::new());
                                                            bump_compose_editor_key(&mut compose_editor_key);
                                                            show_compose.set(true);
                                                        }
                                                    },
                                                    IconReply { class: "h-4 w-4".to_string() }
                                                    span { "Reply" }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                                    title: "Reply all",
                                                    onclick: {
                                                        let row = row.clone();
                                                        let own_addresses = own_addresses.clone();
                                                        move |_| {
                                                            let (to, cc) = reply_all_recipients(&row.message, &own_addresses);
                                                            compose_draft_id.set(String::new());
                                                            compose_mode.set(MailComposeMode::ReplyAll);
                                                            compose_source_message_id.set(row.message.id.clone());
                                                            compose_identity_id.set(default_mail_identity_id_for_account(
                                                                &identities.peek(),
                                                                &row.message.account_id,
                                                            ));
                                                            compose_to.set(compose_address_line(&to));
                                                            compose_cc.set(compose_address_line(&cc));
                                                            compose_bcc.set(String::new());
                                                            compose_subject.set(reply_subject(&message_subject(&row.message)));
                                                            compose_body.set(reply_body(&row));
                                                            compose_body_html.set(reply_body_html(&row));
                                                            compose_in_reply_to.set(
                                                                row.message.message_id.clone().unwrap_or_default(),
                                                            );
                                                            compose_references.set(reply_references(&row.message));
                                                            compose_attachments.set(Vec::new());
                                                            bump_compose_editor_key(&mut compose_editor_key);
                                                            show_compose.set(true);
                                                        }
                                                    },
                                                    IconReplyAll { class: "h-4 w-4".to_string() }
                                                    span { "All" }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                                    title: "Forward",
                                                    onclick: {
                                                        let row = row.clone();
                                                        move |_| {
                                                            compose_draft_id.set(String::new());
                                                            compose_mode.set(MailComposeMode::Forward);
                                                            compose_source_message_id.set(row.message.id.clone());
                                                            compose_identity_id.set(default_mail_identity_id_for_account(
                                                                &identities.peek(),
                                                                &row.message.account_id,
                                                            ));
                                                            compose_to.set(String::new());
                                                            compose_cc.set(String::new());
                                                            compose_bcc.set(String::new());
                                                            compose_subject.set(forward_subject(&message_subject(&row.message)));
                                                            compose_body.set(forward_body(&row));
                                                            compose_body_html.set(forward_body_html(&row));
                                                            compose_in_reply_to.set(String::new());
                                                            compose_references.set(Vec::new());
                                                            compose_attachments.set(Vec::new());
                                                            bump_compose_editor_key(&mut compose_editor_key);
                                                            show_compose.set(true);
                                                        }
                                                    },
                                                    IconForward { class: "h-4 w-4".to_string() }
                                                    span { "Forward" }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                                    title: if is_seen { "Mark unread" } else { "Mark read" },
                                                    disabled: mutating_message(),
                                                    onclick: {
                                                        let message_id = message_id.clone();
                                                        move |_| {
                                                            let message_id = message_id.clone();
                                                            let action = if is_seen {
                                                                MailMessageMutationAction::MarkUnread
                                                            } else {
                                                                MailMessageMutationAction::MarkRead
                                                            };
                                                            spawn(async move {
                                                                mutating_message.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                match use_mail::mutate_message(&message_id, action, None).await {
                                                                    Ok(result) => {
                                                                        if let Some(updated) = result.message {
                                                                            let mut current = messages();
                                                                            for item in &mut current {
                                                                                if item.id == updated.id {
                                                                                    *item = updated.clone();
                                                                                    break;
                                                                                }
                                                                            }
                                                                            messages.set(current);
                                                                            if let Some(mut current_detail) = detail() {
                                                                                if current_detail.message.id == updated.id {
                                                                                    current_detail.message = updated;
                                                                                    detail.set(Some(current_detail));
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                mutating_message.set(false);
                                                            });
                                                        }
                                                    },
                                                    if mutating_message() {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    } else {
                                                        IconEye { class: "h-4 w-4".to_string() }
                                                    }
                                                    span { "{seen_label}" }
                                                }
                                                button {
                                                    class: if is_flagged {
                                                        "btn btn-ghost btn-sm h-8 min-h-8 gap-1 text-warning"
                                                    } else {
                                                        "btn btn-ghost btn-sm h-8 min-h-8 gap-1"
                                                    },
                                                    title: if is_flagged { "Remove star" } else { "Star" },
                                                    disabled: mutating_message(),
                                                    onclick: {
                                                        let message_id = message_id.clone();
                                                        move |_| {
                                                            let message_id = message_id.clone();
                                                            let action = if is_flagged {
                                                                MailMessageMutationAction::Unstar
                                                            } else {
                                                                MailMessageMutationAction::Star
                                                            };
                                                            spawn(async move {
                                                                mutating_message.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                match use_mail::mutate_message(&message_id, action, None).await {
                                                                    Ok(result) => {
                                                                        if let Some(updated) = result.message {
                                                                            let mut current = messages();
                                                                            for item in &mut current {
                                                                                if item.id == updated.id {
                                                                                    *item = updated.clone();
                                                                                    break;
                                                                                }
                                                                            }
                                                                            messages.set(current);
                                                                            if let Some(mut current_detail) = detail() {
                                                                                if current_detail.message.id == updated.id {
                                                                                    current_detail.message = updated;
                                                                                    detail.set(Some(current_detail));
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                mutating_message.set(false);
                                                            });
                                                        }
                                                    },
                                                    IconStar { class: "h-4 w-4".to_string() }
                                                    span { "{star_label}" }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1",
                                                    title: "Archive",
                                                    disabled: mutating_message() || !has_archive_target,
                                                    onclick: {
                                                        let message_id = message_id.clone();
                                                        let account_id = account_id.clone();
                                                        move |_| {
                                                            let message_id = message_id.clone();
                                                            let account_id = account_id.clone();
                                                            spawn(async move {
                                                                mutating_message.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                match use_mail::mutate_message(&message_id, MailMessageMutationAction::Archive, None).await {
                                                                    Ok(result) => {
                                                                        if result.removed_from_folder {
                                                                            messages.set(messages().into_iter().filter(|message| message.id != message_id).collect());
                                                                            selected_message.set(String::new());
                                                                            move_target_folder.set(String::new());
                                                                            detail.set(None);
                                                                            mobile_mail_pane.set(MailMobilePane::Messages);
                                                                            if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                                                folders.set(rows);
                                                                            }
                                                                            notice.set(Some("Message archived".to_string()));
                                                                        }
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                mutating_message.set(false);
                                                            });
                                                        }
                                                    },
                                                    IconArchive { class: "h-4 w-4".to_string() }
                                                    span { "Archive" }
                                                }
                                                button {
                                                    class: "btn btn-ghost btn-sm h-8 min-h-8 gap-1 text-error",
                                                    title: "Move to trash",
                                                    disabled: mutating_message() || !has_trash_target,
                                                    onclick: {
                                                        let message_id = message_id.clone();
                                                        let account_id = account_id.clone();
                                                        move |_| {
                                                            let message_id = message_id.clone();
                                                            let account_id = account_id.clone();
                                                            spawn(async move {
                                                                mutating_message.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                match use_mail::mutate_message(&message_id, MailMessageMutationAction::Trash, None).await {
                                                                    Ok(result) => {
                                                                        if result.removed_from_folder {
                                                                            messages.set(messages().into_iter().filter(|message| message.id != message_id).collect());
                                                                            selected_message.set(String::new());
                                                                            move_target_folder.set(String::new());
                                                                            detail.set(None);
                                                                            mobile_mail_pane.set(MailMobilePane::Messages);
                                                                            if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                                                folders.set(rows);
                                                                            }
                                                                            notice.set(Some("Message moved to trash".to_string()));
                                                                        }
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                mutating_message.set(false);
                                                            });
                                                        }
                                                    },
                                                    IconTrash { class: "h-4 w-4".to_string() }
                                                    span { "Trash" }
                                                }
                                                div { class: "join",
                                                    select {
                                                        class: "select select-bordered select-sm join-item h-8 min-h-8 max-w-40",
                                                        value: "{move_target_folder()}",
                                                        disabled: mutating_message(),
                                                        onchange: move |e| move_target_folder.set(e.value()),
                                                        option { value: "", "Move to..." }
                                                        for folder in folders_snapshot.clone().into_iter().filter(|folder| folder.selectable && folder.id != row.message.folder_id) {
                                                            option { value: "{folder.id}", "{folder.display_path}" }
                                                        }
                                                    }
                                                    button {
                                                        class: "btn btn-outline btn-sm join-item h-8 min-h-8 px-2",
                                                        title: "Move",
                                                        disabled: mutating_message() || move_target_folder().is_empty(),
                                                        onclick: {
                                                            let message_id = message_id.clone();
                                                            let account_id = account_id.clone();
                                                            move |_| {
                                                                let message_id = message_id.clone();
                                                                let account_id = account_id.clone();
                                                                let target_folder = move_target_folder();
                                                                if target_folder.is_empty() {
                                                                    return;
                                                                }
                                                                spawn(async move {
                                                                    mutating_message.set(true);
                                                                    error.set(None);
                                                                    notice.set(None);
                                                                    match use_mail::mutate_message(
                                                                        &message_id,
                                                                        MailMessageMutationAction::Move,
                                                                        Some(target_folder),
                                                                    ).await {
                                                                        Ok(result) => {
                                                                            if result.removed_from_folder {
                                                                                messages.set(messages().into_iter().filter(|message| message.id != message_id).collect());
                                                                                selected_message.set(String::new());
                                                                                move_target_folder.set(String::new());
                                                                                detail.set(None);
                                                                                mobile_mail_pane.set(MailMobilePane::Messages);
                                                                                if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                                                    folders.set(rows);
                                                                                }
                                                                                if let Some(path) = result.destination_folder_path {
                                                                                    notice.set(Some(format!("Message moved to {path}")));
                                                                                } else {
                                                                                    notice.set(Some("Message moved".to_string()));
                                                                                }
                                                                            }
                                                                        }
                                                                        Err(e) => error.set(Some(e)),
                                                                    }
                                                                    mutating_message.set(false);
                                                                });
                                                            }
                                                        },
                                                        IconMoveRight { class: "h-4 w-4".to_string() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    div { class: "min-h-0 flex-1 overflow-y-auto p-4",
                                        if !row.attachments.is_empty() {
                                            div { class: "mb-4 rounded border border-base-300 bg-base-200/60 p-3",
                                                div { class: "mb-2 text-xs font-semibold uppercase text-base-content/60",
                                                    "Attachments"
                                                }
                                                div { class: "grid gap-2",
                                                    for attachment in row.attachments.clone() {
                                                        {
                                                            let name = mail_attachment_name(&attachment);
                                                            let meta = mail_attachment_meta(&attachment);
                                                            let href = format!("/api/mail/attachments/{}/download", attachment.id);
                                                            let open_href = format!("/api/mail/attachments/{}/open", attachment.id);
                                                            let attachment_for_save = attachment.clone();
                                                            rsx! {
                                                                div {
                                                                    class: "flex items-center gap-3 rounded border border-base-300 bg-base-100 px-3 py-2 text-sm",
                                                                    IconFileText { class: "h-4 w-4 text-base-content/60".to_string() }
                                                                    div { class: "min-w-0 flex-1",
                                                                        div { class: "truncate font-medium", "{name}" }
                                                                        div { class: "truncate text-xs text-base-content/50", "{meta}" }
                                                                    }
                                                                    div { class: "flex shrink-0 items-center gap-1",
                                                                        a {
                                                                            class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                            href: "{open_href}",
                                                                            target: "_blank",
                                                                            rel: "noreferrer",
                                                                            title: "Open attachment",
                                                                            IconEye { class: "h-4 w-4".to_string() }
                                                                        }
                                                                        a {
                                                                            class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                            href: "{href}",
                                                                            target: "_blank",
                                                                            rel: "noreferrer",
                                                                            title: "Download attachment",
                                                                            IconDownload { class: "h-4 w-4".to_string() }
                                                                        }
                                                                        button {
                                                                            class: "btn btn-ghost btn-xs h-8 min-h-8 px-2",
                                                                            title: "Save to Files",
                                                                            onclick: move |_| {
                                                                                attachment_save_parent.set(None);
                                                                                attachment_save_folders.set(Vec::new());
                                                                                attachment_save_breadcrumb.set(Vec::new());
                                                                                saving_attachment.set(Some(attachment_for_save.clone()));
                                                                            },
                                                                            IconFolderOpen { class: "h-4 w-4".to_string() }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if let Some(body) = row.body_html.as_ref() {
                                            MailMessageHtmlBody {
                                                body: body.clone(),
                                                message_id: row.message.id.clone(),
                                                remote_images_loaded_messages,
                                            }
                                        } else if let Some(body) = row.body_text.as_ref() {
                                            pre { class: "whitespace-pre-wrap break-words font-sans text-sm leading-6", "{body}" }
                                        } else {
                                            div { class: "flex h-full items-center justify-center text-sm text-base-content/60",
                                                "Body unavailable"
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            div { class: "flex flex-1 flex-col items-center justify-center gap-2 text-base-content/60",
                                IconFileText { class: "h-8 w-8".to_string() }
                                div { class: "text-sm", "Select a message" }
                                button {
                                    class: "btn btn-sm btn-ghost lg:hidden",
                                    onclick: move |_| mobile_mail_pane.set(MailMobilePane::Messages),
                                    "Back to messages"
                                }
                            }
                        }
                    }
                }
            }

            if let Some(attachment) = saving_attachment() {
                {
                    let attachment_name = mail_attachment_name(&attachment);
                    let attachment_meta = mail_attachment_meta(&attachment);
                    let destination_label = attachment_save_breadcrumb()
                        .last()
                        .map(|folder| folder.name.clone())
                        .unwrap_or_else(|| "Files".to_string());
                    rsx! {
                        div { class: "modal modal-open",
                            div {
                                class: "modal-box max-w-md",
                                onclick: move |event| event.stop_propagation(),

                                h2 { class: "text-lg font-semibold", "Save Attachment" }
                                div { class: "mt-2 rounded border border-base-300 bg-base-200/60 px-3 py-2 text-sm",
                                    div { class: "truncate font-medium", "{attachment_name}" }
                                    div { class: "truncate text-xs text-base-content/50", "{attachment_meta}" }
                                }

                                div { class: "mt-4 text-sm breadcrumbs px-0",
                                    ul {
                                        li {
                                            a {
                                                class: "cursor-pointer",
                                                onclick: move |_| attachment_save_parent.set(None),
                                                "Files"
                                            }
                                        }
                                        for folder in attachment_save_breadcrumb() {
                                            li {
                                                a {
                                                    class: "cursor-pointer",
                                                    onclick: {
                                                        let id = folder.id.clone();
                                                        move |_| attachment_save_parent.set(Some(id.clone()))
                                                    },
                                                    "{folder.name}"
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "min-h-32 max-h-64 overflow-y-auto rounded border border-base-300",
                                    if attachment_save_loading() {
                                        div { class: "flex h-32 items-center justify-center",
                                            span { class: "loading loading-spinner loading-md" }
                                        }
                                    } else if attachment_save_folders().is_empty() {
                                        div { class: "flex h-32 items-center justify-center text-sm text-base-content/40",
                                            "No subfolders here"
                                        }
                                    } else {
                                        ul { class: "menu menu-sm p-1",
                                            for folder in attachment_save_folders() {
                                                li {
                                                    a {
                                                        onclick: {
                                                            let id = folder.id.clone();
                                                            move |_| attachment_save_parent.set(Some(id.clone()))
                                                        },
                                                        IconFolder {}
                                                        span { "{folder.name}" }
                                                        IconChevronRight { class: "ml-auto h-4 w-4 opacity-40".to_string() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "mt-3 text-xs text-base-content/60",
                                    "Destination: {destination_label}"
                                }

                                div { class: "modal-action",
                                    button {
                                        class: "btn btn-ghost",
                                        disabled: attachment_save_busy(),
                                        onclick: move |_| saving_attachment.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-primary gap-2",
                                        disabled: attachment_save_busy(),
                                        onclick: {
                                            let attachment_id = attachment.id.clone();
                                            move |_| {
                                                let attachment_id = attachment_id.clone();
                                                let parent = attachment_save_parent();
                                                spawn(async move {
                                                    attachment_save_busy.set(true);
                                                    error.set(None);
                                                    notice.set(None);
                                                    match use_mail::save_attachment(
                                                        &attachment_id,
                                                        parent.as_deref(),
                                                        None,
                                                    ).await {
                                                        Ok(file) => {
                                                            notice.set(Some(format!("Saved {} to Files", file.name)));
                                                            saving_attachment.set(None);
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                    attachment_save_busy.set(false);
                                                });
                                            }
                                        },
                                        if attachment_save_busy() {
                                            span { class: "loading loading-spinner loading-xs" }
                                        } else {
                                            IconFolderOpen { class: "h-4 w-4".to_string() }
                                        }
                                        span { "Save Here" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if show_setup() {
                div { class: "modal modal-open",
                    div { class: "modal-box max-w-4xl",
                        h2 { class: "text-xl font-semibold", "Mail account" }
                        div { class: "mt-4 grid gap-4 md:grid-cols-2",
                            label { class: "form-control",
                                span { class: "label-text", "Display name" }
                                input { class: "input input-bordered", value: "{display_name}", oninput: move |e| display_name.set(e.value()) }
                            }
                            label { class: "form-control",
                                span { class: "label-text", "Email address" }
                                input { class: "input input-bordered", r#type: "email", value: "{email_address}", oninput: move |e| email_address.set(e.value()) }
                            }
                            label { class: "form-control",
                                span { class: "label-text", "IMAP host" }
                                input { class: "input input-bordered", value: "{imap_host}", oninput: move |e| imap_host.set(e.value()) }
                            }
                            label { class: "form-control",
                                span { class: "label-text", "IMAP username" }
                                input { class: "input input-bordered", value: "{imap_username}", oninput: move |e| imap_username.set(e.value()) }
                            }
                            div { class: "grid grid-cols-[1fr_9rem] gap-3",
                                label { class: "form-control",
                                    span { class: "label-text", "IMAP security" }
                                    select { class: "select select-bordered", value: "{imap_security}", onchange: move |e| imap_security.set(e.value()),
                                        option { value: "tls", "TLS" }
                                        option { value: "start_tls", "STARTTLS" }
                                        option { value: "plain", "Plain" }
                                    }
                                }
                                label { class: "form-control",
                                    span { class: "label-text", "Port" }
                                    input { class: "input input-bordered", value: "{imap_port}", oninput: move |e| imap_port.set(e.value()) }
                                }
                            }
                            label { class: "form-control",
                                span { class: "label-text", "SMTP host" }
                                input { class: "input input-bordered", value: "{smtp_host}", oninput: move |e| smtp_host.set(e.value()) }
                            }
                            label { class: "form-control",
                                span { class: "label-text", "SMTP username" }
                                input { class: "input input-bordered", value: "{smtp_username}", oninput: move |e| smtp_username.set(e.value()) }
                            }
                            div { class: "grid grid-cols-[1fr_9rem] gap-3",
                                label { class: "form-control",
                                    span { class: "label-text", "SMTP security" }
                                    select { class: "select select-bordered", value: "{smtp_security}", onchange: move |e| smtp_security.set(e.value()),
                                        option { value: "tls", "TLS" }
                                        option { value: "start_tls", "STARTTLS" }
                                        option { value: "plain", "Plain" }
                                    }
                                }
                                label { class: "form-control",
                                    span { class: "label-text", "Port" }
                                    input { class: "input input-bordered", value: "{smtp_port}", oninput: move |e| smtp_port.set(e.value()) }
                                }
                            }
                            label { class: "form-control md:col-span-2",
                                span { class: "label-text", "App password" }
                                input { class: "input input-bordered", r#type: "password", value: "{password}", oninput: move |e| password.set(e.value()) }
                            }
                        }
                        div { class: "modal-action",
                            button { class: "btn", disabled: creating(), onclick: move |_| show_setup.set(false), "Cancel" }
                            button {
                                class: "btn btn-primary",
                                disabled: creating(),
                                onclick: move |_| {
                                    let display = display_name();
                                    let email = email_address();
                                    let imap_host_val = imap_host();
                                    let imap_user = if imap_username().trim().is_empty() { email.clone() } else { imap_username() };
                                    let smtp_host_val = smtp_host();
                                    let smtp_user = if smtp_username().trim().is_empty() { email.clone() } else { smtp_username() };
                                    let imap_port_val = imap_port().parse::<u16>().unwrap_or(993);
                                    let smtp_port_val = smtp_port().parse::<u16>().unwrap_or(465);
                                    let imap_sec = security_from_value(&imap_security());
                                    let smtp_sec = security_from_value(&smtp_security());
                                    let pass = password();
                                    spawn(async move {
                                        creating.set(true);
                                        error.set(None);
                                        let req = CreateMailAccountRequest {
                                            display_name: if display.trim().is_empty() { email.clone() } else { display },
                                            email_address: email,
                                            imap: MailServerSettings {
                                                host: imap_host_val,
                                                port: imap_port_val,
                                                security: imap_sec,
                                                username: imap_user,
                                            },
                                            smtp: MailServerSettings {
                                                host: smtp_host_val,
                                                port: smtp_port_val,
                                                security: smtp_sec,
                                                username: smtp_user,
                                            },
                                            sync_enabled: false,
                                            sync_interval_secs: None,
                                        };
                                        match use_mail::create_account(&req).await {
                                            Ok(account) => {
                                                if !pass.trim().is_empty() {
                                                    let _ = use_mail::set_credential(&account.id, &pass).await;
                                                }
                                                selected_account.set(account.id.clone());
                                                selected_folder.set(String::new());
                                                selected_message.set(String::new());
                                                selecting_messages.set(false);
                                                selected_message_ids.set(Vec::new());
                                                move_target_folder.set(String::new());
                                                messages.set(Vec::new());
                                                message_next_cursor.set(None);
                                                message_has_more.set(false);
                                                detail.set(None);
                                                mobile_mail_pane.set(MailMobilePane::Folders);
                                                if let Ok(list) = use_mail::list_accounts().await {
                                                    accounts.set(list);
                                                }
                                                let next = account_dirty.peek().0 + 1;
                                                account_dirty.set(MailAccountDirtyTick(next));
                                                if let Ok(rows) = use_mail::list_identities().await {
                                                    identities.set(rows);
                                                }
                                                match use_mail::refresh_folders(&account.id).await {
                                                    Ok(rows) => folders.set(rows),
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                show_setup.set(false);
                                                nav.replace(Route::MailAccount {
                                                    account_id: account.id.clone(),
                                                });
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        creating.set(false);
                                    });
                                },
                                if creating() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "Create"
                            }
                        }
                    }
                    div { class: "modal-backdrop", onclick: move |_| show_setup.set(false) }
                }
            }

            if let Some(account) = account_settings() {
                {
                    let account_id = account.id.clone();
                    let account_identities = identities_snapshot
                        .iter()
                        .filter(|identity| identity.account_id == account_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    let diagnostics_for_account = {
                        let id = account_id.clone();
                        provider_diagnostics().filter(|diagnostics| diagnostics.account_id == id)
                    };
                    rsx! {
                        div { class: "modal modal-open",
                            div { class: "modal-box max-w-4xl",
                                h2 { class: "text-xl font-semibold", "Account settings" }
                                div { class: "mt-1 truncate text-sm text-base-content/60", "{account.email_address}" }

                                div { class: "mt-4 grid gap-4 md:grid-cols-2",
                                    label { class: "form-control",
                                        span { class: "label-text", "Display name" }
                                        input {
                                            class: "input input-bordered",
                                            value: "{account_display_name()}",
                                            oninput: move |e| account_display_name.set(e.value()),
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "Email address" }
                                        input {
                                            class: "input input-bordered",
                                            r#type: "email",
                                            value: "{account_email_address()}",
                                            oninput: move |e| account_email_address.set(e.value()),
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "IMAP host" }
                                        input {
                                            class: "input input-bordered",
                                            value: "{account_imap_host()}",
                                            oninput: move |e| account_imap_host.set(e.value()),
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "IMAP username" }
                                        input {
                                            class: "input input-bordered",
                                            value: "{account_imap_username()}",
                                            oninput: move |e| account_imap_username.set(e.value()),
                                        }
                                    }
                                    div { class: "grid grid-cols-[1fr_9rem] gap-3",
                                        label { class: "form-control",
                                            span { class: "label-text", "IMAP security" }
                                            select {
                                                class: "select select-bordered",
                                                value: "{account_imap_security()}",
                                                onchange: move |e| account_imap_security.set(e.value()),
                                                option { value: "tls", "TLS" }
                                                option { value: "start_tls", "STARTTLS" }
                                                option { value: "plain", "Plain" }
                                            }
                                        }
                                        label { class: "form-control",
                                            span { class: "label-text", "Port" }
                                            input {
                                                class: "input input-bordered",
                                                value: "{account_imap_port()}",
                                                oninput: move |e| account_imap_port.set(e.value()),
                                            }
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "SMTP host" }
                                        input {
                                            class: "input input-bordered",
                                            value: "{account_smtp_host()}",
                                            oninput: move |e| account_smtp_host.set(e.value()),
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "SMTP username" }
                                        input {
                                            class: "input input-bordered",
                                            value: "{account_smtp_username()}",
                                            oninput: move |e| account_smtp_username.set(e.value()),
                                        }
                                    }
                                    div { class: "grid grid-cols-[1fr_9rem] gap-3",
                                        label { class: "form-control",
                                            span { class: "label-text", "SMTP security" }
                                            select {
                                                class: "select select-bordered",
                                                value: "{account_smtp_security()}",
                                                onchange: move |e| account_smtp_security.set(e.value()),
                                                option { value: "tls", "TLS" }
                                                option { value: "start_tls", "STARTTLS" }
                                                option { value: "plain", "Plain" }
                                            }
                                        }
                                        label { class: "form-control",
                                            span { class: "label-text", "Port" }
                                            input {
                                                class: "input input-bordered",
                                                value: "{account_smtp_port()}",
                                                oninput: move |e| account_smtp_port.set(e.value()),
                                            }
                                        }
                                    }
                                    label { class: "form-control md:col-span-2",
                                        span { class: "label-text", "New app password" }
                                        input {
                                            class: "input input-bordered",
                                            r#type: "password",
                                            value: "{account_password()}",
                                            placeholder: "Leave empty to keep the stored credential",
                                            oninput: move |e| account_password.set(e.value()),
                                        }
                                    }
                                    label { class: "label cursor-pointer justify-start gap-3 rounded border border-base-300 px-3",
                                        input {
                                            class: "toggle toggle-sm",
                                            r#type: "checkbox",
                                            checked: account_sync_enabled(),
                                            onchange: move |e| account_sync_enabled.set(e.checked()),
                                        }
                                        span {
                                            span { class: "block text-sm font-medium", "Automatic sync" }
                                            span { class: "block text-xs text-base-content/60", "Periodically sync this account in the background. Manual sync remains available." }
                                        }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "Sync interval" }
                                        input {
                                            class: "input input-bordered",
                                            r#type: "number",
                                            min: "{MAIL_MIN_SYNC_INTERVAL_MINUTES}",
                                            max: "{MAIL_MAX_SYNC_INTERVAL_MINUTES}",
                                            value: "{account_sync_interval_minutes()}",
                                            placeholder: "Server default",
                                            oninput: move |e| account_sync_interval_minutes.set(e.value()),
                                        }
                                        span { class: "label-text-alt text-base-content/60", "Minutes; blank uses the server default." }
                                    }
                                    label { class: "form-control",
                                        span { class: "label-text", "List order" }
                                        input {
                                            class: "input input-bordered",
                                            r#type: "number",
                                            value: "{account_sort_order()}",
                                            placeholder: "0",
                                            oninput: move |e| account_sort_order.set(e.value()),
                                        }
                                        span { class: "label-text-alt text-base-content/60", "Lower numbers appear first in the account list." }
                                    }
                                }

                                div { class: "mt-6 border-t border-base-300 pt-5",
                                    div { class: "flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between",
                                        div {
                                            div { class: "text-sm font-semibold", "Provider diagnostics" }
                                        }
                                        button {
                                            class: "btn btn-sm btn-outline gap-2",
                                            disabled: loading_provider_diagnostics(),
                                            onclick: {
                                                let account_id = account_id.clone();
                                                move |_| {
                                                    let account_id = account_id.clone();
                                                    spawn(async move {
                                                        loading_provider_diagnostics.set(true);
                                                        error.set(None);
                                                        notice.set(None);
                                                        match use_mail::diagnostics(&account_id).await {
                                                            Ok(result) => provider_diagnostics.set(Some(result)),
                                                            Err(e) => error.set(Some(e)),
                                                        }
                                                        loading_provider_diagnostics.set(false);
                                                    });
                                                }
                                            },
                                            if loading_provider_diagnostics() {
                                                span { class: "loading loading-spinner loading-xs" }
                                            } else {
                                                IconRefreshCw { class: "h-4 w-4".to_string() }
                                            }
                                            span { "Run diagnostics" }
                                        }
                                    }

                                    if loading_provider_diagnostics() && diagnostics_for_account.is_none() {
                                        div { class: "mt-3 rounded border border-base-300 px-3 py-4 text-sm text-base-content/60",
                                            span { class: "loading loading-spinner loading-xs mr-2" }
                                            "Checking provider settings"
                                        }
                                    }

                                    if let Some(diagnostics) = diagnostics_for_account.as_ref() {
                                        {
                                            let imap_capabilities = diagnostics
                                                .imap
                                                .capabilities
                                                .iter()
                                                .take(12)
                                                .cloned()
                                                .collect::<Vec<_>>();
                                            let smtp_capabilities = diagnostics
                                                .smtp
                                                .capabilities
                                                .iter()
                                                .take(12)
                                                .cloned()
                                                .collect::<Vec<_>>();
                                            let role_rows = diagnostics.roles.clone();
                                            let folder_rows = diagnostics.folders.clone();
                                            let recent_errors = diagnostics.recent_errors.clone();
                                            rsx! {
                                                div { class: "mt-3 grid gap-3 md:grid-cols-2",
                                                    div { class: "rounded border border-base-300 bg-base-100 p-3",
                                                        div { class: "flex items-center justify-between gap-3",
                                                            div { class: "text-sm font-medium", "IMAP" }
                                                            span { class: "{provider_status_badge_class(diagnostics.imap.ok)}",
                                                                "{provider_status_label(diagnostics.imap.ok)}"
                                                            }
                                                        }
                                                        div { class: "mt-1 truncate text-xs text-base-content/60",
                                                            "{diagnostics.imap.username}@{diagnostics.imap.host}:{diagnostics.imap.port} ({security_label(diagnostics.imap.security)})"
                                                        }
                                                        if let Some(message) = diagnostics.imap.error.as_ref() {
                                                            div { class: "mt-2 whitespace-pre-wrap break-words text-xs text-error", "{message}" }
                                                        }
                                                        if !imap_capabilities.is_empty() {
                                                            div { class: "mt-2 flex flex-wrap gap-1",
                                                                for capability in imap_capabilities {
                                                                    span { class: "badge badge-ghost badge-sm", "{capability}" }
                                                                }
                                                                if diagnostics.imap.capabilities.len() > 12 {
                                                                    span { class: "badge badge-ghost badge-sm",
                                                                        "+{diagnostics.imap.capabilities.len() - 12}"
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }

                                                    div { class: "rounded border border-base-300 bg-base-100 p-3",
                                                        div { class: "flex items-center justify-between gap-3",
                                                            div { class: "text-sm font-medium", "SMTP" }
                                                            span { class: "{provider_status_badge_class(diagnostics.smtp.ok)}",
                                                                "{provider_status_label(diagnostics.smtp.ok)}"
                                                            }
                                                        }
                                                        div { class: "mt-1 truncate text-xs text-base-content/60",
                                                            "{diagnostics.smtp.username}@{diagnostics.smtp.host}:{diagnostics.smtp.port} ({security_label(diagnostics.smtp.security)})"
                                                        }
                                                        if let Some(message) = diagnostics.smtp.error.as_ref() {
                                                            div { class: "mt-2 whitespace-pre-wrap break-words text-xs text-error", "{message}" }
                                                        }
                                                        if !smtp_capabilities.is_empty() {
                                                            div { class: "mt-2 flex flex-wrap gap-1",
                                                                for capability in smtp_capabilities {
                                                                    span { class: "badge badge-ghost badge-sm", "{capability}" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                div { class: "mt-3 rounded border border-base-300 bg-base-100 p-3",
                                                    div { class: "flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between",
                                                        div {
                                                            div { class: "text-sm font-medium", "Sent copy" }
                                                            div { class: "mt-1 text-xs text-base-content/60",
                                                                "{diagnostics.sent_copy.detail}"
                                                            }
                                                        }
                                                        span { class: "{sent_copy_diagnostics_badge_class(diagnostics.sent_copy.status)}",
                                                            "{sent_copy_diagnostics_label(diagnostics.sent_copy.status)}"
                                                        }
                                                    }
                                                    if let Some(path) = diagnostics.sent_copy.sent_folder_path.as_ref() {
                                                        div { class: "mt-2 truncate text-xs text-base-content/60", "Folder: {path}" }
                                                    }
                                                }

                                                div { class: "mt-3 rounded border border-base-300 bg-base-100 p-3",
                                                    div { class: "text-sm font-medium", "Folder roles" }
                                                    div { class: "mt-2 flex flex-wrap gap-1.5",
                                                        for row in role_rows {
                                                            span { class: "{role_diagnostics_badge_class(row.status)}",
                                                                if let Some(path) = row.folder_path.as_ref() {
                                                                    "{folder_role_name(row.role)}: {path}"
                                                                } else {
                                                                    "{folder_role_name(row.role)}: missing"
                                                                }
                                                            }
                                                        }
                                                    }
                                                    details { class: "mt-3",
                                                        summary { class: "cursor-pointer text-xs font-medium text-base-content/70",
                                                            "Folder inference ({folder_rows.len()} cached)"
                                                        }
                                                        div { class: "mt-2 max-h-44 overflow-auto rounded border border-base-300",
                                                            for folder in folder_rows {
                                                                div { class: "grid grid-cols-[minmax(0,1fr)_auto] gap-2 border-b border-base-300 px-2 py-1.5 text-xs last:border-b-0",
                                                                    div { class: "min-w-0",
                                                                        div { class: "truncate", "{folder.path}" }
                                                                        if !folder.attributes.is_empty() {
                                                                            div { class: "truncate text-base-content/50", "{folder.attributes.join(\", \")}" }
                                                                        }
                                                                    }
                                                                    div { class: "text-right text-base-content/60",
                                                                        if let Some(role) = folder.role {
                                                                            "{folder_role_name(role)}"
                                                                        } else {
                                                                            "None"
                                                                        }
                                                                        span { " / {folder_role_source_label(folder.role_source)}" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                div { class: "mt-3 rounded border border-base-300 bg-base-100 p-3",
                                                    div { class: "text-sm font-medium", "Recent provider errors" }
                                                    if recent_errors.is_empty() {
                                                        div { class: "mt-2 text-xs text-base-content/60", "No recent provider errors are recorded." }
                                                    } else {
                                                        div { class: "mt-2 grid gap-2",
                                                            for item in recent_errors {
                                                                div { class: "rounded bg-base-200/60 px-2 py-1.5 text-xs",
                                                                    div { class: "flex flex-wrap items-center gap-2 text-base-content/60",
                                                                        span { class: "font-medium text-base-content/80", "{item.scope}" }
                                                                        span { "{item.operation}" }
                                                                        if let Some(at) = item.at.as_ref() {
                                                                            span { "{short_date(Some(at))}" }
                                                                        }
                                                                        if let Some(path) = item.folder_path.as_ref() {
                                                                            span { class: "truncate", "{path}" }
                                                                        }
                                                                    }
                                                                    div { class: "mt-1 whitespace-pre-wrap break-words text-error", "{item.message}" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                div { class: "mt-2 text-xs text-base-content/50",
                                                    "Generated {short_date(Some(&diagnostics.generated_at))}"
                                                    if diagnostics.sync_in_progress {
                                                        span { " while sync is in progress" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "mt-6 border-t border-base-300 pt-5",
                                    div { class: "flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between",
                                        div {
                                            div { class: "text-sm font-semibold", "Sender identities" }
                                            div { class: "mt-1 text-xs text-base-content/60",
                                                "Each identity can use a different From address, Reply-To, and signature."
                                            }
                                        }
                                        button {
                                            class: "btn btn-sm btn-outline gap-2",
                                            disabled: saving_identity(),
                                            onclick: {
                                                let account = account.clone();
                                                let account_identities = account_identities.clone();
                                                move |_| {
                                                    identity_editing_id.set(String::new());
                                                    identity_display_name.set(account.display_name.clone());
                                                    identity_email_address.set(account.email_address.clone());
                                                    identity_reply_to.set(String::new());
                                                    identity_signature.set(String::new());
                                                    identity_is_default.set(account_identities.is_empty());
                                                    identity_editor_open.set(true);
                                                }
                                            },
                                            IconPlus { class: "h-4 w-4".to_string() }
                                            span { "Add identity" }
                                        }
                                    }

                                    div { class: "mt-3 grid gap-2",
                                        if account_identities.is_empty() {
                                            div { class: "rounded border border-dashed border-base-300 px-3 py-4 text-sm text-base-content/60",
                                                "No sender identities yet. The account address will be used until one is added."
                                            }
                                        }
                                        for identity in account_identities.clone() {
                                            {
                                                let identity_id = identity.id.clone();
                                                let identity_for_edit = identity.clone();
                                                let deleting_this_identity = deleting_identity_id() == identity_id;
                                                rsx! {
                                                    div {
                                                        key: "{identity_id}",
                                                        class: "flex flex-col gap-3 rounded border border-base-300 bg-base-100 px-3 py-2 sm:flex-row sm:items-center sm:justify-between",
                                                        div { class: "min-w-0",
                                                            div { class: "flex min-w-0 flex-wrap items-center gap-2",
                                                                span { class: "truncate text-sm font-medium", "{mail_identity_label(&identity)}" }
                                                                if identity.is_default {
                                                                    span { class: "badge badge-primary badge-sm", "Default" }
                                                                }
                                                            }
                                                            if let Some(reply_to) = identity.reply_to.as_ref().filter(|value| !value.trim().is_empty()) {
                                                                div { class: "mt-1 truncate text-xs text-base-content/60", "Reply-To: {reply_to}" }
                                                            }
                                                            if identity.signature.as_ref().is_some_and(|value| !value.trim().is_empty()) {
                                                                div { class: "mt-1 text-xs text-base-content/50", "Signature configured" }
                                                            }
                                                        }
                                                        div { class: "flex shrink-0 items-center gap-1",
                                                            if !identity.is_default {
                                                                button {
                                                                    class: "btn btn-ghost btn-xs",
                                                                    disabled: saving_identity() || deleting_this_identity,
                                                                    onclick: {
                                                                        let identity_id = identity_id.clone();
                                                                        move |_| {
                                                                            let identity_id = identity_id.clone();
                                                                            spawn(async move {
                                                                                saving_identity.set(true);
                                                                                error.set(None);
                                                                                notice.set(None);
                                                                                let req = UpdateMailIdentityRequest {
                                                                                    display_name: None,
                                                                                    email_address: None,
                                                                                    reply_to: None,
                                                                                    signature: None,
                                                                                    is_default: Some(true),
                                                                                };
                                                                                match use_mail::update_identity(&identity_id, &req).await {
                                                                                    Ok(_) => {
                                                                                        match use_mail::list_identities().await {
                                                                                            Ok(rows) => identities.set(rows),
                                                                                            Err(e) => error.set(Some(e)),
                                                                                        }
                                                                                    }
                                                                                    Err(e) => error.set(Some(e)),
                                                                                }
                                                                                saving_identity.set(false);
                                                                            });
                                                                        }
                                                                    },
                                                                    "Make default"
                                                                }
                                                            }
                                                            button {
                                                                class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0",
                                                                title: "Edit identity",
                                                                disabled: saving_identity() || deleting_this_identity,
                                                                onclick: move |_| {
                                                                    identity_editing_id.set(identity_for_edit.id.clone());
                                                                    identity_display_name.set(identity_for_edit.display_name.clone());
                                                                    identity_email_address.set(identity_for_edit.email_address.clone());
                                                                    identity_reply_to.set(identity_for_edit.reply_to.clone().unwrap_or_default());
                                                                    identity_signature.set(identity_for_edit.signature.clone().unwrap_or_default());
                                                                    identity_is_default.set(identity_for_edit.is_default);
                                                                    identity_editor_open.set(true);
                                                                },
                                                                IconPencil { class: "h-4 w-4".to_string() }
                                                            }
                                                            button {
                                                                class: "btn btn-ghost btn-xs h-8 min-h-8 w-8 p-0 text-error",
                                                                title: "Delete identity",
                                                                disabled: saving_identity() || deleting_this_identity,
                                                                onclick: {
                                                                    let identity_id = identity_id.clone();
                                                                    let account_id = account_id.clone();
                                                                    move |_| {
                                                                        let identity_id = identity_id.clone();
                                                                        let account_id = account_id.clone();
                                                                        spawn(async move {
                                                                            deleting_identity_id.set(identity_id.clone());
                                                                            error.set(None);
                                                                            notice.set(None);
                                                                            match use_mail::delete_identity(&identity_id).await {
                                                                                Ok(()) => {
                                                                                    match use_mail::list_identities().await {
                                                                                        Ok(rows) => {
                                                                                            let next_identity = default_mail_identity_id_for_account(
                                                                                                &rows,
                                                                                                &account_id,
                                                                                            );
                                                                                            identities.set(rows);
                                                                                            if compose_identity_id.peek().as_str() == identity_id.as_str() {
                                                                                                compose_identity_id.set(next_identity);
                                                                                            }
                                                                                            if identity_editing_id.peek().as_str() == identity_id.as_str() {
                                                                                                identity_editor_open.set(false);
                                                                                                identity_editing_id.set(String::new());
                                                                                            }
                                                                                            notice.set(Some("Identity deleted".to_string()));
                                                                                        }
                                                                                        Err(e) => error.set(Some(e)),
                                                                                    }
                                                                                }
                                                                                Err(e) => error.set(Some(e)),
                                                                            }
                                                                            deleting_identity_id.set(String::new());
                                                                        });
                                                                    }
                                                                },
                                                                if deleting_this_identity {
                                                                    span { class: "loading loading-spinner loading-xs" }
                                                                } else {
                                                                    IconTrash { class: "h-4 w-4".to_string() }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if identity_editor_open() {
                                        div { class: "mt-4 rounded border border-base-300 bg-base-200/30 p-3",
                                            div { class: "text-sm font-semibold",
                                                if identity_editing_id().is_empty() {
                                                    "New identity"
                                                } else {
                                                    "Edit identity"
                                                }
                                            }
                                            div { class: "mt-3 grid gap-3 md:grid-cols-2",
                                                label { class: "form-control",
                                                    span { class: "label-text", "Display name" }
                                                    input {
                                                        class: "input input-bordered",
                                                        value: "{identity_display_name()}",
                                                        oninput: move |e| identity_display_name.set(e.value()),
                                                    }
                                                }
                                                label { class: "form-control",
                                                    span { class: "label-text", "Email address" }
                                                    input {
                                                        class: "input input-bordered",
                                                        r#type: "email",
                                                        value: "{identity_email_address()}",
                                                        oninput: move |e| identity_email_address.set(e.value()),
                                                    }
                                                }
                                                label { class: "form-control md:col-span-2",
                                                    span { class: "label-text", "Reply-To" }
                                                    input {
                                                        class: "input input-bordered",
                                                        r#type: "email",
                                                        value: "{identity_reply_to()}",
                                                        placeholder: "Optional",
                                                        oninput: move |e| identity_reply_to.set(e.value()),
                                                    }
                                                }
                                                label { class: "form-control md:col-span-2",
                                                    span { class: "label-text", "Signature" }
                                                    textarea {
                                                        class: "textarea textarea-bordered",
                                                        rows: 3,
                                                        value: "{identity_signature()}",
                                                        placeholder: "Optional",
                                                        oninput: move |e| identity_signature.set(e.value()),
                                                    }
                                                }
                                                label { class: "label cursor-pointer justify-start gap-3 rounded border border-base-300 px-3 md:col-span-2",
                                                    input {
                                                        class: "toggle toggle-sm",
                                                        r#type: "checkbox",
                                                        checked: identity_is_default(),
                                                        onchange: move |e| identity_is_default.set(e.checked()),
                                                    }
                                                    span {
                                                        span { class: "block text-sm font-medium", "Use as default sender" }
                                                        span { class: "block text-xs text-base-content/60", "Compose uses this identity when no specific sender is selected." }
                                                    }
                                                }
                                            }
                                            div { class: "mt-3 flex justify-end gap-2",
                                                button {
                                                    class: "btn btn-sm",
                                                    disabled: saving_identity(),
                                                    onclick: move |_| {
                                                        identity_editor_open.set(false);
                                                        identity_editing_id.set(String::new());
                                                    },
                                                    "Cancel"
                                                }
                                                button {
                                                    class: "btn btn-sm btn-primary",
                                                    disabled: saving_identity(),
                                                    onclick: {
                                                        let account_id = account_id.clone();
                                                        move |_| {
                                                            let account_id = account_id.clone();
                                                            let editing_id = identity_editing_id();
                                                            let display = identity_display_name();
                                                            let email = identity_email_address();
                                                            let reply_to = nonempty_string(identity_reply_to());
                                                            let signature = nonempty_string(identity_signature());
                                                            let is_default = identity_is_default();
                                                            spawn(async move {
                                                                saving_identity.set(true);
                                                                error.set(None);
                                                                notice.set(None);
                                                                let result = if editing_id.trim().is_empty() {
                                                                    let req = CreateMailIdentityRequest {
                                                                        account_id,
                                                                        display_name: display,
                                                                        email_address: email,
                                                                        reply_to,
                                                                        signature,
                                                                        is_default,
                                                                    };
                                                                    use_mail::create_identity(&req).await
                                                                } else {
                                                                    let req = UpdateMailIdentityRequest {
                                                                        display_name: Some(display),
                                                                        email_address: Some(email),
                                                                        reply_to: Some(reply_to),
                                                                        signature: Some(signature),
                                                                        is_default: if is_default { Some(true) } else { None },
                                                                    };
                                                                    use_mail::update_identity(&editing_id, &req).await
                                                                };
                                                                match result {
                                                                    Ok(saved) => {
                                                                        match use_mail::list_identities().await {
                                                                            Ok(rows) => {
                                                                                identities.set(rows);
                                                                                if saved.is_default || compose_identity_id.peek().trim().is_empty() {
                                                                                    compose_identity_id.set(saved.id.clone());
                                                                                }
                                                                            }
                                                                            Err(e) => error.set(Some(e)),
                                                                        }
                                                                        identity_editor_open.set(false);
                                                                        identity_editing_id.set(String::new());
                                                                        notice.set(Some("Identity saved".to_string()));
                                                                    }
                                                                    Err(e) => error.set(Some(e)),
                                                                }
                                                                saving_identity.set(false);
                                                            });
                                                        }
                                                    },
                                                    if saving_identity() {
                                                        span { class: "loading loading-spinner loading-xs" }
                                                    }
                                                    "Save identity"
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "modal-action items-center justify-between",
                                    button {
                                        class: if confirming_account_delete() {
                                            "btn btn-error gap-2"
                                        } else {
                                            "btn btn-outline btn-error gap-2"
                                        },
                                        disabled: saving_account_settings() || deleting_account(),
                                        onclick: {
                                            let account_id = account_id.clone();
                                            move |_| {
                                                if !confirming_account_delete() {
                                                    confirming_account_delete.set(true);
                                                    return;
                                                }
                                                let account_id = account_id.clone();
                                                spawn(async move {
                                                    deleting_account.set(true);
                                                    error.set(None);
                                                    notice.set(None);
                                                    match use_mail::delete_account(&account_id).await {
                                                        Ok(_) => {
                                                            match use_mail::list_accounts().await {
                                                                Ok(list) => {
                                                                    let next = list.first().map(|a| a.id.clone()).unwrap_or_default();
                                                                    accounts.set(list);
                                                                    let next_dirty = account_dirty.peek().0 + 1;
                                                                    account_dirty.set(MailAccountDirtyTick(next_dirty));
                                                                    if let Ok(rows) = use_mail::list_identities().await {
                                                                        identities.set(rows);
                                                                    }
                                                                    selected_account.set(next.clone());
                                                                    selected_folder.set(String::new());
                                                                    selected_message.set(String::new());
                                                                    selecting_messages.set(false);
                                                                    selected_message_ids.set(Vec::new());
                                                                    move_target_folder.set(String::new());
                                                                    messages.set(Vec::new());
                                                                    message_next_cursor.set(None);
                                                                    message_has_more.set(false);
                                                                    detail.set(None);
                                                                    mobile_mail_pane.set(MailMobilePane::Folders);
                                                                    if next.is_empty() {
                                                                        folders.set(Vec::new());
                                                                    } else {
                                                                        match use_mail::list_folders(&next).await {
                                                                            Ok(rows) => folders.set(rows),
                                                                            Err(e) => error.set(Some(e)),
                                                                        }
                                                                    }
                                                                    account_settings.set(None);
                                                                    identity_editor_open.set(false);
                                                                    identity_editing_id.set(String::new());
                                                                    notice.set(Some("Mail account deleted".to_string()));
                                                                    if next.is_empty() {
                                                                        nav.replace(Route::Mail {});
                                                                    } else {
                                                                        nav.replace(Route::MailAccount {
                                                                            account_id: next.clone(),
                                                                        });
                                                                    }
                                                                }
                                                                Err(e) => error.set(Some(e)),
                                                            }
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                    deleting_account.set(false);
                                                    confirming_account_delete.set(false);
                                                });
                                            }
                                        },
                                        if deleting_account() {
                                            span { class: "loading loading-spinner loading-xs" }
                                        } else {
                                            IconTrash { class: "h-4 w-4".to_string() }
                                        }
                                        if confirming_account_delete() {
                                            span { "Confirm delete" }
                                        } else {
                                            span { "Delete" }
                                        }
                                    }
                                    div { class: "flex gap-2",
                                        button {
                                            class: "btn",
                                            disabled: saving_account_settings() || deleting_account(),
                                            onclick: move |_| account_settings.set(None),
                                            "Cancel"
                                        }
                                        button {
                                            class: "btn btn-primary",
                                            disabled: saving_account_settings() || deleting_account(),
                                            onclick: {
                                                let account_id = account_id.clone();
                                                move |_| {
                                                    let account_id = account_id.clone();
                                                    let display = account_display_name();
                                                    let email = account_email_address();
                                                    let imap_host_val = account_imap_host();
                                                    let imap_user = account_imap_username();
                                                    let smtp_host_val = account_smtp_host();
                                                    let smtp_user = account_smtp_username();
                                                    let imap_port_val = account_imap_port().parse::<u16>().unwrap_or(993);
                                                    let smtp_port_val = account_smtp_port().parse::<u16>().unwrap_or(465);
                                                    let imap_sec = security_from_value(&account_imap_security());
                                                    let smtp_sec = security_from_value(&account_smtp_security());
                                                    let pass = account_password();
                                                    let sync_enabled = account_sync_enabled();
                                                    let sort_order = account_sort_order().trim().parse::<i32>().unwrap_or(0);
                                                    let sync_interval_secs = match sync_interval_secs_from_minutes(
                                                        &account_sync_interval_minutes(),
                                                    ) {
                                                        Ok(value) => value,
                                                        Err(e) => {
                                                            error.set(Some(e));
                                                            return;
                                                        }
                                                    };
                                                    spawn(async move {
                                                        saving_account_settings.set(true);
                                                        error.set(None);
                                                        notice.set(None);
                                                        let req = UpdateMailAccountRequest {
                                                            display_name: Some(display),
                                                            email_address: Some(email),
                                                            imap: Some(MailServerSettings {
                                                                host: imap_host_val,
                                                                port: imap_port_val,
                                                                security: imap_sec,
                                                                username: imap_user,
                                                            }),
                                                            smtp: Some(MailServerSettings {
                                                                host: smtp_host_val,
                                                                port: smtp_port_val,
                                                                security: smtp_sec,
                                                                username: smtp_user,
                                                            }),
                                                            sort_order: Some(sort_order),
                                                            sync_enabled: Some(sync_enabled),
                                                            sync_interval_secs: Some(sync_interval_secs),
                                                        };
                                                        match use_mail::update_account(&account_id, &req).await {
                                                            Ok(updated) => {
                                                                if !pass.trim().is_empty() {
                                                                    if let Err(e) = use_mail::set_credential(&account_id, &pass).await {
                                                                        error.set(Some(e));
                                                                        saving_account_settings.set(false);
                                                                        return;
                                                                    }
                                                                }
                                                                let mut current = accounts();
                                                                for item in &mut current {
                                                                    if item.id == updated.id {
                                                                        *item = updated.clone();
                                                                        break;
                                                                    }
                                                                }
                                                                current.sort_by(|a, b| {
                                                                    a.sort_order
                                                                        .cmp(&b.sort_order)
                                                                        .then_with(|| a.email_address.cmp(&b.email_address))
                                                                });
                                                                accounts.set(current);
                                                                let next = account_dirty.peek().0 + 1;
                                                                account_dirty.set(MailAccountDirtyTick(next));
                                                                selected_account.set(updated.id.clone());
                                                                selecting_messages.set(false);
                                                                selected_message_ids.set(Vec::new());
                                                                account_settings.set(None);
                                                                notice.set(Some("Account settings saved".to_string()));
                                                            }
                                                            Err(e) => error.set(Some(e)),
                                                        }
                                                        saving_account_settings.set(false);
                                                    });
                                                }
                                            },
                                            if saving_account_settings() {
                                                span { class: "loading loading-spinner loading-xs" }
                                            }
                                            "Save"
                                        }
                                    }
                                }
                            }
                            div { class: "modal-backdrop", onclick: move |_| account_settings.set(None) }
                        }
                    }
                }
            }

            if let Some(folder) = settings_folder() {
                {
                    let auto_label = match folder.role {
                        Some(role) => format!("Automatic ({})", folder_role_name(role)),
                        None => "Automatic".to_string(),
                    };
                    rsx! {
                        div { class: "modal modal-open",
                            div { class: "modal-box max-w-xl",
                                h2 { class: "text-xl font-semibold", "Folder settings" }
                                div { class: "mt-1 truncate text-sm text-base-content/60", "{folder.display_path}" }

                                div { class: "mt-5 grid gap-4",
                                    label { class: "form-control",
                                        span { class: "label-text", "Folder role" }
                                        select {
                                            class: "select select-bordered",
                                            value: "{folder_role_value()}",
                                            onchange: move |e| folder_role_value.set(e.value()),
                                            option { value: "auto", "{auto_label}" }
                                            option { value: "none", "None" }
                                            option { value: "inbox", "Inbox" }
                                            option { value: "sent", "Sent" }
                                            option { value: "drafts", "Drafts" }
                                            option { value: "archive", "Archive" }
                                            option { value: "trash", "Trash" }
                                            option { value: "spam", "Spam" }
                                            option { value: "all_mail", "All mail" }
                                        }
                                    }
                                    label { class: "label cursor-pointer justify-start gap-3 rounded border border-base-300 px-3",
                                        input {
                                            class: "toggle toggle-sm",
                                            r#type: "checkbox",
                                            checked: folder_sync_enabled(),
                                            onchange: move |e| folder_sync_enabled.set(e.checked()),
                                        }
                                        span {
                                            span { class: "block text-sm font-medium", "Include in account sync" }
                                            span { class: "block text-xs text-base-content/60", "Manual folder sync remains available." }
                                        }
                                    }
                                }

                                div { class: "modal-action",
                                    button {
                                        class: "btn",
                                        disabled: saving_folder_settings(),
                                        onclick: move |_| settings_folder.set(None),
                                        "Cancel"
                                    }
                                    button {
                                        class: "btn btn-primary",
                                        disabled: saving_folder_settings(),
                                        onclick: move |_| {
                                            let folder = folder.clone();
                                            let role_value = folder_role_value();
                                            let sync_enabled = folder_sync_enabled();
                                            spawn(async move {
                                                saving_folder_settings.set(true);
                                                error.set(None);
                                                notice.set(None);
                                                let req = UpdateMailFolderRequest {
                                                    role: if role_value == "auto" {
                                                        None
                                                    } else {
                                                        folder_role_from_value(&role_value)
                                                    },
                                                    infer_role: role_value == "auto",
                                                    clear_role: role_value == "none",
                                                    sync_enabled: Some(sync_enabled),
                                                };
                                                match use_mail::update_folder(&folder.account_id, &folder.id, &req).await {
                                                    Ok(updated) => {
                                                        let mut current = folders();
                                                        for item in &mut current {
                                                            if item.id == updated.id {
                                                                *item = updated.clone();
                                                                break;
                                                            }
                                                        }
                                                        folders.set(current);
                                                        settings_folder.set(None);
                                                        notice.set(Some("Folder settings saved".to_string()));
                                                    }
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                saving_folder_settings.set(false);
                                            });
                                        },
                                        if saving_folder_settings() {
                                            span { class: "loading loading-spinner loading-xs" }
                                        }
                                        "Save"
                                    }
                                }
                            }
                            div { class: "modal-backdrop", onclick: move |_| settings_folder.set(None) }
                        }
                    }
                }
            }

            if error().is_some() || notice().is_some() {
                div { class: "{toast_stack_class}",
                    if let Some(e) = error() {
                        div { class: "pointer-events-auto rounded-xl bg-error p-3 text-sm text-error-content shadow-xl",
                            div { class: "flex items-start gap-3",
                                div { class: "min-w-0 flex-1",
                                    div { class: "font-medium", "Error" }
                                    div { class: "mt-1 whitespace-pre-wrap break-words", "{e}" }
                                }
                                button {
                                    class: "btn btn-ghost btn-xs h-7 min-h-7 w-7 shrink-0 p-0 text-error-content",
                                    title: "Close",
                                    onclick: move |_| error.set(None),
                                    IconX { class: "h-4 w-4".to_string() }
                                }
                            }
                        }
                    }
                    if let Some(message) = notice() {
                        div { class: "pointer-events-auto rounded-xl bg-info p-3 text-sm text-info-content shadow-xl",
                            div { class: "flex items-start gap-3",
                                div { class: "min-w-0 flex-1",
                                    div { class: "font-medium", "Notice" }
                                    div { class: "mt-1 whitespace-pre-wrap break-words", "{message}" }
                                }
                                button {
                                    class: "btn btn-ghost btn-xs h-7 min-h-7 w-7 shrink-0 p-0 text-info-content",
                                    title: "Close",
                                    onclick: move |_| notice.set(None),
                                    IconX { class: "h-4 w-4".to_string() }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(message) = sync_status_message {
                div { class: "pointer-events-none fixed inset-x-0 bottom-0 z-50",
                    div { class: "pointer-events-auto flex w-full min-w-0 items-center gap-2 border-y border-info/30 bg-base-100/95 px-4 py-2 text-sm text-info shadow-lg backdrop-blur",
                        span { class: "loading loading-spinner loading-xs" }
                        span { class: "truncate", "{message}" }
                    }
                }
            }
        }
    }
}

#[component]
fn MailMessageHtmlBody(
    body: String,
    message_id: String,
    mut remote_images_loaded_messages: Signal<Vec<String>>,
) -> Element {
    let has_remote_images = mail_html_has_blocked_remote_images(&body);
    let remote_images_loaded =
        mail_remote_images_loaded(&remote_images_loaded_messages(), &message_id);
    let display_body = mail_body_html_for_display(&body, remote_images_loaded);

    rsx! {
        if has_remote_images && !remote_images_loaded {
            div { class: "mb-4 flex flex-col gap-2 border border-base-300 bg-base-100 p-3 text-sm sm:flex-row sm:items-center sm:justify-between",
                div { class: "text-base-content/70", "Remote images are blocked for this message." }
                button {
                    class: "btn btn-xs btn-outline gap-2 self-start sm:self-auto",
                    onclick: move |_| {
                        let mut loaded = remote_images_loaded_messages();
                        if !loaded.iter().any(|id| id == &message_id) {
                            loaded.push(message_id.clone());
                        }
                        remote_images_loaded_messages.set(loaded);
                    },
                    IconEye { class: "h-4 w-4".to_string() }
                    span { "Load images" }
                }
            }
        }
        div {
            class: "max-w-none break-words text-sm leading-6 [&_a]:text-primary [&_blockquote]:border-l-2 [&_blockquote]:border-base-300 [&_blockquote]:pl-3 [&_blockquote]:text-base-content/70 [&_img]:max-w-full [&_table]:max-w-full",
            dangerous_inner_html: "{display_body}",
        }
    }
}

fn security_from_value(value: &str) -> MailSecurity {
    match value {
        "start_tls" => MailSecurity::StartTls,
        "plain" => MailSecurity::Plain,
        _ => MailSecurity::Tls,
    }
}

fn security_to_value(value: MailSecurity) -> &'static str {
    match value {
        MailSecurity::StartTls => "start_tls",
        MailSecurity::Plain => "plain",
        MailSecurity::Tls => "tls",
    }
}

fn security_label(value: MailSecurity) -> &'static str {
    match value {
        MailSecurity::StartTls => "STARTTLS",
        MailSecurity::Plain => "Plain",
        MailSecurity::Tls => "TLS",
    }
}

fn provider_status_badge_class(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "badge badge-success badge-sm",
        Some(false) => "badge badge-error badge-sm",
        None => "badge badge-ghost badge-sm",
    }
}

fn provider_status_label(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "Connected",
        Some(false) => "Failed",
        None => "Not checked",
    }
}

fn role_diagnostics_badge_class(status: MailProviderRoleStatus) -> &'static str {
    match status {
        MailProviderRoleStatus::Found => "badge badge-outline badge-sm",
        MailProviderRoleStatus::Missing => "badge badge-warning badge-sm",
    }
}

fn sent_copy_diagnostics_badge_class(status: MailSentCopyDiagnosticStatus) -> &'static str {
    match status {
        MailSentCopyDiagnosticStatus::Ready => "badge badge-success badge-sm",
        MailSentCopyDiagnosticStatus::MissingSentFolder => "badge badge-warning badge-sm",
    }
}

fn sent_copy_diagnostics_label(status: MailSentCopyDiagnosticStatus) -> &'static str {
    match status {
        MailSentCopyDiagnosticStatus::Ready => "Ready",
        MailSentCopyDiagnosticStatus::MissingSentFolder => "No Sent folder",
    }
}

fn folder_role_source_label(source: MailFolderRoleSource) -> &'static str {
    match source {
        MailFolderRoleSource::Inferred => "auto",
        MailFolderRoleSource::User => "manual",
    }
}

fn sorted_mail_folders(folders: &[MailFolderResponse]) -> Vec<MailFolderResponse> {
    let folder_paths = folders
        .iter()
        .map(|folder| folder.path.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut by_parent = std::collections::HashMap::<Option<String>, Vec<MailFolderResponse>>::new();
    for folder in folders {
        let parent = folder
            .parent_path
            .as_ref()
            .filter(|path| folder_paths.contains(path.as_str()))
            .cloned();
        by_parent.entry(parent).or_default().push(folder.clone());
    }
    for children in by_parent.values_mut() {
        sort_mail_folder_siblings(children);
    }

    let mut out = Vec::with_capacity(folders.len());
    append_mail_folder_children(None, &mut by_parent, &mut out);
    while !by_parent.is_empty() {
        let next_parent = by_parent.keys().next().cloned().unwrap_or(None);
        append_mail_folder_children(next_parent.as_deref(), &mut by_parent, &mut out);
    }
    out
}

fn append_mail_folder_children(
    parent: Option<&str>,
    by_parent: &mut std::collections::HashMap<Option<String>, Vec<MailFolderResponse>>,
    out: &mut Vec<MailFolderResponse>,
) {
    let key = parent.map(str::to_string);
    let Some(children) = by_parent.remove(&key) else {
        return;
    };
    for child in children {
        let path = child.path.clone();
        out.push(child);
        append_mail_folder_children(Some(&path), by_parent, out);
    }
}

fn sort_mail_folder_siblings(folders: &mut [MailFolderResponse]) {
    folders.sort_by(|a, b| {
        folder_role_rank(a.role)
            .cmp(&folder_role_rank(b.role))
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
            .then_with(|| {
                a.path
                    .to_ascii_lowercase()
                    .cmp(&b.path.to_ascii_lowercase())
            })
    });
}

fn folder_role_rank(role: Option<MailFolderRole>) -> u8 {
    match role {
        Some(MailFolderRole::Inbox) => 0,
        Some(MailFolderRole::Sent) => 10,
        Some(MailFolderRole::Drafts) => 20,
        Some(MailFolderRole::Archive) => 30,
        Some(MailFolderRole::Spam) => 40,
        Some(MailFolderRole::Trash) => 50,
        Some(MailFolderRole::AllMail) => 60,
        None => 100,
    }
}

fn folder_role_label(role: Option<MailFolderRole>) -> Option<&'static str> {
    role.map(folder_role_name)
}

fn folder_role_name(role: MailFolderRole) -> &'static str {
    match role {
        MailFolderRole::Inbox => "Inbox",
        MailFolderRole::Sent => "Sent",
        MailFolderRole::Drafts => "Drafts",
        MailFolderRole::Trash => "Trash",
        MailFolderRole::Archive => "Archive",
        MailFolderRole::Spam => "Spam",
        MailFolderRole::AllMail => "All mail",
    }
}

fn folder_role_value_for(folder: &MailFolderResponse) -> String {
    if folder.role_source == MailFolderRoleSource::Inferred {
        return "auto".to_string();
    }
    match folder.role {
        Some(MailFolderRole::Inbox) => "inbox",
        Some(MailFolderRole::Sent) => "sent",
        Some(MailFolderRole::Drafts) => "drafts",
        Some(MailFolderRole::Trash) => "trash",
        Some(MailFolderRole::Archive) => "archive",
        Some(MailFolderRole::Spam) => "spam",
        Some(MailFolderRole::AllMail) => "all_mail",
        None => "none",
    }
    .to_string()
}

fn folder_role_from_value(value: &str) -> Option<MailFolderRole> {
    match value {
        "inbox" => Some(MailFolderRole::Inbox),
        "sent" => Some(MailFolderRole::Sent),
        "drafts" => Some(MailFolderRole::Drafts),
        "trash" => Some(MailFolderRole::Trash),
        "archive" => Some(MailFolderRole::Archive),
        "spam" => Some(MailFolderRole::Spam),
        "all_mail" => Some(MailFolderRole::AllMail),
        _ => None,
    }
}

fn folder_depth(folder: &MailFolderResponse) -> usize {
    folder
        .delimiter
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|delimiter| folder.path.matches(delimiter).count())
        .unwrap_or(0)
}

fn append_unique_messages(
    mut current: Vec<MailMessageSummaryResponse>,
    incoming: Vec<MailMessageSummaryResponse>,
) -> Vec<MailMessageSummaryResponse> {
    for message in incoming {
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.id == message.id)
        {
            *existing = message;
        } else {
            current.push(message);
        }
    }
    current
}

fn sync_interval_minutes_value(seconds: u64) -> String {
    seconds.div_ceil(60).to_string()
}

fn sync_interval_secs_from_minutes(input: &str) -> Result<Option<u64>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let minutes = trimmed
        .parse::<u64>()
        .map_err(|_| "Sync interval must be a whole number of minutes".to_string())?;
    if !(MAIL_MIN_SYNC_INTERVAL_MINUTES..=MAIL_MAX_SYNC_INTERVAL_MINUTES).contains(&minutes) {
        return Err(format!(
            "Sync interval must be between {} minute and {} minutes",
            MAIL_MIN_SYNC_INTERVAL_MINUTES, MAIL_MAX_SYNC_INTERVAL_MINUTES
        ));
    }
    Ok(Some(minutes * 60))
}

fn account_sync_error_notice(result: &MailAccountSyncResponse) -> Option<String> {
    if result.errors == 0 {
        return None;
    }

    let failed_folders = result
        .folders
        .iter()
        .filter_map(|folder| {
            folder
                .error
                .as_ref()
                .map(|error| format!("{}: {}", folder.folder_path, error))
        })
        .take(3)
        .collect::<Vec<_>>();
    let details = if failed_folders.is_empty() {
        String::new()
    } else {
        format!(" {}", failed_folders.join("; "))
    };
    Some(format!(
        "Account sync finished with {} folder error(s).{}",
        result.errors, details
    ))
}

fn nonempty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn mail_editor_detail_string(detail: &wasm_bindgen::JsValue, key: &str) -> String {
    js_sys::Reflect::get(detail, &wasm_bindgen::JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

fn run_mail_editor_command(action: &str) {
    let editor_id = serde_json::to_string(MAIL_COMPOSE_EDITOR_ID)
        .unwrap_or_else(|_| "\"mail-compose-editor\"".to_string());
    let action = serde_json::to_string(action).unwrap_or_else(|_| "\"\"".to_string());
    let _ = js_sys::eval(&format!(
        "window.UncloudMailEditor && window.UncloudMailEditor.command({editor_id}, {action});"
    ));
}

fn click_compose_attachment_input() {
    if let Some(input) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(MAIL_COMPOSE_ATTACHMENT_INPUT_ID))
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
    {
        input.click();
    }
}

fn reset_compose_attachment_input() {
    if let Some(input) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(MAIL_COMPOSE_ATTACHMENT_INPUT_ID))
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
    {
        input.set_value("");
    }
}

fn bump_compose_editor_key(compose_editor_key: &mut Signal<u64>) {
    let next = compose_editor_key.peek().saturating_add(1);
    compose_editor_key.set(next);
}

fn upsert_draft(
    mut current: Vec<MailDraftResponse>,
    incoming: MailDraftResponse,
) -> Vec<MailDraftResponse> {
    if let Some(existing) = current.iter_mut().find(|draft| draft.id == incoming.id) {
        *existing = incoming;
    } else {
        current.push(incoming);
    }
    current.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    current
}

fn compose_title(mode: MailComposeMode) -> &'static str {
    match mode {
        MailComposeMode::New => "Compose",
        MailComposeMode::Reply => "Reply",
        MailComposeMode::ReplyAll => "Reply all",
        MailComposeMode::Forward => "Forward",
    }
}

fn draft_subject_label(draft: &MailDraftResponse) -> String {
    draft
        .subject
        .trim()
        .is_empty()
        .then(|| "(no subject)".to_string())
        .unwrap_or_else(|| draft.subject.clone())
}

fn compose_address_line(addresses: &[MailAddressDto]) -> String {
    addresses
        .iter()
        .map(format_mail_address)
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn mail_own_addresses(
    account: Option<&MailAccountResponse>,
    identities: &[MailIdentityResponse],
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(account) = account {
        push_unique_address_key(&mut out, &account.email_address);
    }
    for identity in identities {
        push_unique_address_key(&mut out, &identity.email_address);
    }
    out
}

fn default_mail_identity_id(identities: &[MailIdentityResponse]) -> String {
    identities
        .iter()
        .find(|identity| identity.is_default)
        .or_else(|| identities.first())
        .map(|identity| identity.id.clone())
        .unwrap_or_default()
}

fn default_mail_identity_id_for_account(
    identities: &[MailIdentityResponse],
    account_id: &str,
) -> String {
    identities
        .iter()
        .find(|identity| identity.account_id == account_id && identity.is_default)
        .or_else(|| {
            identities
                .iter()
                .find(|identity| identity.account_id == account_id)
        })
        .map(|identity| identity.id.clone())
        .unwrap_or_default()
}

fn push_unique_address_key(out: &mut Vec<String>, address: &str) {
    let key = address_key(address);
    if !key.is_empty() && !out.iter().any(|existing| existing == &key) {
        out.push(key);
    }
}

fn address_key(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

fn reply_all_recipients(
    message: &MailMessageSummaryResponse,
    own_addresses: &[String],
) -> (Vec<MailAddressDto>, Vec<MailAddressDto>) {
    let mut to = Vec::new();
    let mut cc = Vec::new();
    for address in &message.from {
        push_reply_recipient(&mut to, &cc, own_addresses, address);
    }
    if to.is_empty() {
        for address in &message.to {
            push_reply_recipient(&mut to, &cc, own_addresses, address);
        }
    } else {
        for address in &message.to {
            push_reply_recipient(&mut cc, &to, own_addresses, address);
        }
    }
    for address in &message.cc {
        push_reply_recipient(&mut cc, &to, own_addresses, address);
    }
    (to, cc)
}

fn push_reply_recipient(
    target: &mut Vec<MailAddressDto>,
    other: &[MailAddressDto],
    own_addresses: &[String],
    address: &MailAddressDto,
) {
    let key = address_key(&address.address);
    if key.is_empty()
        || own_addresses.iter().any(|own| own == &key)
        || target
            .iter()
            .any(|existing| address_key(&existing.address) == key)
        || other
            .iter()
            .any(|existing| address_key(&existing.address) == key)
    {
        return;
    }
    target.push(address.clone());
}

fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        trimmed.to_string()
    } else {
        format!("Re: {trimmed}")
    }
}

fn forward_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("fwd:"))
        || trimmed
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("fw:"))
    {
        trimmed.to_string()
    } else {
        format!("Fwd: {trimmed}")
    }
}

fn reply_references(message: &MailMessageSummaryResponse) -> Vec<String> {
    let mut references = message.references.clone();
    if references.is_empty() {
        if let Some(in_reply_to) = message
            .in_reply_to
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            references.push(in_reply_to.to_string());
        }
    }
    if let Some(message_id) = message
        .message_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if !references.iter().any(|value| value == message_id) {
            references.push(message_id.to_string());
        }
    }
    references
}

fn reply_body(row: &MailMessageDetailResponse) -> String {
    let source = plain_reply_source(row);
    let quoted = quote_plain_text(&source);
    let date = short_date(
        row.message
            .internal_date
            .as_deref()
            .or(row.message.date.as_deref()),
    );
    let sender = message_sender(&row.message);
    format!("\n\nOn {date}, {sender} wrote:\n{quoted}")
}

fn reply_body_html(row: &MailMessageDetailResponse) -> String {
    let date = escape_html(&short_date(
        row.message
            .internal_date
            .as_deref()
            .or(row.message.date.as_deref()),
    ));
    let sender = escape_html(&message_sender(&row.message));
    format!(
        "<p></p><p>On {date}, {sender} wrote:</p><blockquote>{}</blockquote>",
        html_reply_source(row)
    )
}

fn forward_body(row: &MailMessageDetailResponse) -> String {
    let source = plain_reply_source(row);
    let mut out = String::from("\n\n---------- Forwarded message ---------\n");
    out.push_str(&format!("From: {}\n", message_sender(&row.message)));
    out.push_str(&format!(
        "Date: {}\n",
        short_date(
            row.message
                .internal_date
                .as_deref()
                .or(row.message.date.as_deref())
        )
    ));
    out.push_str(&format!("Subject: {}\n", message_subject(&row.message)));
    out.push_str(&format!("To: {}\n\n", message_recipients(&row.message)));
    out.push_str(&source);
    out
}

fn forward_body_html(row: &MailMessageDetailResponse) -> String {
    let source = html_reply_source(row);
    let date = escape_html(&short_date(
        row.message
            .internal_date
            .as_deref()
            .or(row.message.date.as_deref()),
    ));
    let sender = escape_html(&message_sender(&row.message));
    let subject = escape_html(&message_subject(&row.message));
    let recipients = escape_html(&message_recipients(&row.message));
    format!(
        concat!(
            "<p></p>",
            "<p>---------- Forwarded message ---------</p>",
            "<p><strong>From:</strong> {sender}<br>",
            "<strong>Date:</strong> {date}<br>",
            "<strong>Subject:</strong> {subject}<br>",
            "<strong>To:</strong> {recipients}</p>",
            "{source}",
        ),
        sender = sender,
        date = date,
        subject = subject,
        recipients = recipients,
        source = source,
    )
}

fn plain_reply_source(row: &MailMessageDetailResponse) -> String {
    row.body_text
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| row.message.snippet.clone())
        .unwrap_or_default()
}

fn html_reply_source(row: &MailMessageDetailResponse) -> String {
    row.body_html
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| plain_text_to_html(&plain_reply_source(row)))
}

fn plain_text_to_html(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .split("\n\n")
        .map(|part| {
            let lines = part
                .lines()
                .map(escape_html)
                .collect::<Vec<_>>()
                .join("<br>");
            format!("<p>{lines}</p>")
        })
        .collect::<Vec<_>>()
        .join("")
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn quote_plain_text(input: &str) -> String {
    if input.trim().is_empty() {
        return ">".to_string();
    }
    input
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_subject(message: &MailMessageSummaryResponse) -> String {
    message
        .subject
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "(no subject)".to_string())
}

fn mail_identity_label(identity: &MailIdentityResponse) -> String {
    format!("{} <{}>", identity.display_name, identity.email_address)
}

fn sent_copy_status_label(status: MailSentCopyStatus) -> &'static str {
    match status {
        MailSentCopyStatus::ProviderSaved => "provider saved Sent copy",
        MailSentCopyStatus::Appended => "Sent copy appended",
        MailSentCopyStatus::SkippedNoSentFolder => "no Sent folder configured",
        MailSentCopyStatus::Failed => "Sent copy failed",
    }
}

fn mail_attachment_name(attachment: &MailAttachmentResponse) -> String {
    attachment
        .filename
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "Attachment".to_string())
}

fn mail_attachment_meta(attachment: &MailAttachmentResponse) -> String {
    let mut parts = Vec::new();
    if let Some(size) = attachment.size_bytes {
        parts.push(uncloud_common::validation::format_bytes(size as i64));
    }
    if let Some(content_type) = attachment
        .content_type
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(content_type.clone());
    }
    if parts.is_empty() {
        "download".to_string()
    } else {
        parts.join(" | ")
    }
}

fn mail_draft_attachment_meta(attachment: &MailDraftAttachmentResponse) -> String {
    let mut parts = vec![uncloud_common::validation::format_bytes(
        attachment.size_bytes as i64,
    )];
    if !attachment.content_type.trim().is_empty() {
        parts.push(attachment.content_type.clone());
    }
    parts.join(" | ")
}

fn mail_html_has_blocked_remote_images(body: &str) -> bool {
    body.contains("data-uc-remote-src=")
}

fn mail_remote_images_loaded(message_ids: &[String], message_id: &str) -> bool {
    message_ids.iter().any(|id| id == message_id)
}

fn mail_body_html_for_display(body: &str, load_remote_images: bool) -> String {
    if load_remote_images {
        body.replace("data-uc-remote-src=", "src=")
    } else {
        body.to_string()
    }
}

fn mail_folders_pane_class(active: MailMobilePane) -> &'static str {
    match active {
        MailMobilePane::Folders => {
            "flex h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:h-auto lg:min-h-0"
        }
        _ => {
            "hidden h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:flex lg:h-auto lg:min-h-0"
        }
    }
}

fn mail_messages_pane_class(active: MailMobilePane) -> &'static str {
    match active {
        MailMobilePane::Messages => {
            "flex h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:h-auto lg:min-h-0"
        }
        _ => {
            "hidden h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:flex lg:h-auto lg:min-h-0"
        }
    }
}

fn mail_detail_pane_class(active: MailMobilePane) -> &'static str {
    match active {
        MailMobilePane::Detail => {
            "flex h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:h-auto lg:min-h-0"
        }
        _ => {
            "hidden h-[calc(100vh-12rem)] min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:flex lg:h-auto lg:min-h-0"
        }
    }
}

fn message_sender(message: &MailMessageSummaryResponse) -> String {
    message
        .from
        .first()
        .map(format_mail_address)
        .unwrap_or_else(|| "Unknown sender".to_string())
}

fn message_recipients(message: &MailMessageSummaryResponse) -> String {
    if message.to.is_empty() {
        return "Undisclosed recipients".to_string();
    }
    message
        .to
        .iter()
        .map(format_mail_address)
        .collect::<Vec<_>>()
        .join(", ")
}

fn message_has_flag(message: &MailMessageSummaryResponse, flag: &str) -> bool {
    message.flags.iter().any(|value| {
        value
            .trim()
            .trim_start_matches('\\')
            .eq_ignore_ascii_case(flag.trim_start_matches('\\'))
    })
}

fn message_id_selected(selected: &[String], id: &str) -> bool {
    selected.iter().any(|value| value == id)
}

fn toggle_message_id(mut selected: Vec<String>, id: &str) -> Vec<String> {
    if let Some(index) = selected.iter().position(|value| value == id) {
        selected.remove(index);
    } else {
        selected.push(id.to_string());
    }
    selected
}

fn loaded_message_ids(messages: &[MailMessageSummaryResponse]) -> Vec<String> {
    messages.iter().map(|message| message.id.clone()).collect()
}

/// Keep a multi-select intact across a bulk action, dropping only the ids whose
/// messages left the folder (archive/trash). Read/unread leave every message in
/// place, so the whole selection survives; archive/trash that partially fails
/// keeps the still-present rows selected for a follow-up action.
fn retain_loaded_selection(
    selected: Vec<String>,
    messages: &[MailMessageSummaryResponse],
) -> Vec<String> {
    selected
        .into_iter()
        .filter(|id| messages.iter().any(|message| &message.id == id))
        .collect()
}

fn replace_messages(
    mut current: Vec<MailMessageSummaryResponse>,
    updated: Vec<MailMessageSummaryResponse>,
) -> Vec<MailMessageSummaryResponse> {
    for row in updated {
        if let Some(existing) = current.iter_mut().find(|message| message.id == row.id) {
            *existing = row;
        }
    }
    current
}

fn remove_messages(
    current: Vec<MailMessageSummaryResponse>,
    removed: &[String],
) -> Vec<MailMessageSummaryResponse> {
    current
        .into_iter()
        .filter(|message| !message_id_selected(removed, &message.id))
        .collect()
}

fn mark_messages_read(
    messages: Vec<MailMessageSummaryResponse>,
) -> Vec<MailMessageSummaryResponse> {
    messages.into_iter().map(mark_message_read).collect()
}

fn mark_message_read(mut message: MailMessageSummaryResponse) -> MailMessageSummaryResponse {
    if !message_has_flag(&message, "\\Seen") {
        message.flags.push("\\Seen".to_string());
    }
    message
}

fn bulk_action_success_label(action: MailMessageMutationAction, count: usize) -> String {
    let noun = if count == 1 { "message" } else { "messages" };
    match action {
        MailMessageMutationAction::MarkRead => format!("Marked {count} {noun} as read"),
        MailMessageMutationAction::MarkUnread => format!("Marked {count} {noun} as unread"),
        MailMessageMutationAction::Archive => format!("Archived {count} {noun}"),
        MailMessageMutationAction::Trash => format!("Moved {count} {noun} to trash"),
        MailMessageMutationAction::Star => format!("Starred {count} {noun}"),
        MailMessageMutationAction::Unstar => format!("Unstarred {count} {noun}"),
        MailMessageMutationAction::Move => format!("Moved {count} {noun}"),
    }
}

fn parse_compose_addresses(input: &str) -> Vec<MailAddressDto> {
    input
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|address| MailAddressDto {
            name: None,
            address: address.to_string(),
        })
        .collect()
}

fn format_mail_address(address: &MailAddressDto) -> String {
    let email = address.address.trim();
    match address.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() && !name.eq_ignore_ascii_case(email) => {
            format!("{name} <{email}>")
        }
        _ => email.to_string(),
    }
}

fn short_date(value: Option<&str>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    value
        .replace('T', " ")
        .split('.')
        .next()
        .unwrap_or(value)
        .trim_end_matches('Z')
        .chars()
        .take(16)
        .collect()
}
