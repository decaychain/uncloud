use dioxus::prelude::*;
use uncloud_common::{
    CreateMailAccountRequest, FolderResponse, MailAccountResponse, MailAccountSyncResponse,
    MailAddressDto, MailAttachmentResponse, MailFolderResponse, MailFolderRole,
    MailFolderRoleSource, MailIdentityResponse, MailMessageDetailResponse,
    MailMessageMutationAction, MailMessageSummaryResponse, MailSecurity, MailSentCopyStatus,
    MailServerSettings, SendMailMessageRequest, UpdateMailAccountRequest, UpdateMailFolderRequest,
};

use crate::components::icons::{
    IconArchive, IconChevronRight, IconDownload, IconEye, IconFileText, IconFolder, IconFolderOpen,
    IconMail, IconMoveRight, IconPaperclip, IconPlus, IconRefreshCw, IconSend, IconSettings,
    IconStar, IconTrash, IconX,
};
use crate::components::scroll_sentinel::ScrollSentinel;
use crate::hooks::{use_files, use_mail};

const MAIL_MESSAGE_PAGE_SIZE: u32 = 50;
const MAIL_BACKFILL_PAGE_SIZE: u32 = 50;
const MAIL_STATUS_POLL_MS: u32 = 15_000;
const MAIL_NOTICE_TOAST_TIMEOUT_MS: u32 = 6_000;
const MAIL_MIN_SYNC_INTERVAL_MINUTES: u64 = 1;
const MAIL_MAX_SYNC_INTERVAL_MINUTES: u64 = 7 * 24 * 60;

#[component]
pub fn MailPage() -> Element {
    let mut accounts = use_signal(Vec::<MailAccountResponse>::new);
    let mut identities = use_signal(Vec::<MailIdentityResponse>::new);
    let mut folders = use_signal(Vec::<MailFolderResponse>::new);
    let mut messages = use_signal(Vec::<MailMessageSummaryResponse>::new);
    let mut detail = use_signal(|| None::<MailMessageDetailResponse>);
    let mut selected_account = use_signal(String::new);
    let mut selected_folder = use_signal(String::new);
    let mut selected_message = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut syncing = use_signal(|| false);
    let mut loading_more_messages = use_signal(|| false);
    let mut backfilling_messages = use_signal(|| false);
    let mut loading_detail = use_signal(|| false);
    let mut mutating_message = use_signal(|| false);
    let mut move_target_folder = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<String>);
    let mut notice_auto_dismiss_token = use_signal(|| 0u64);
    let mut sync_status = use_signal(|| None::<String>);
    let mut show_setup = use_signal(|| false);
    let mut show_compose = use_signal(|| false);
    let mut sending_message = use_signal(|| false);
    let mut compose_identity_id = use_signal(String::new);
    let mut compose_to = use_signal(String::new);
    let mut compose_cc = use_signal(String::new);
    let mut compose_bcc = use_signal(String::new);
    let mut compose_subject = use_signal(String::new);
    let mut compose_body = use_signal(String::new);
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
    let mut account_enabled = use_signal(|| true);
    let mut account_sync_enabled = use_signal(|| false);
    let mut account_sync_interval_minutes = use_signal(String::new);
    let mut saving_account_settings = use_signal(|| false);
    let mut confirming_account_delete = use_signal(|| false);
    let mut deleting_account = use_signal(|| false);
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

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match use_mail::list_accounts().await {
                Ok(list) => {
                    let first = list.first().map(|a| a.id.clone()).unwrap_or_default();
                    accounts.set(list);
                    selected_account.set(first.clone());
                    match use_mail::list_identities().await {
                        Ok(rows) => identities.set(rows),
                        Err(e) => error.set(Some(e)),
                    }
                    if !first.is_empty() {
                        match use_mail::list_folders(&first).await {
                            Ok(rows) => folders.set(rows),
                            Err(e) => error.set(Some(e)),
                        }
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

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
                        || rows
                            .iter()
                            .any(|folder| folder.account_id == account_id && folder.sync_in_progress);
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
                        if let Ok(page) =
                            use_mail::list_messages(&account_id, &folder_id, MAIL_MESSAGE_PAGE_SIZE, None)
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
    let messages_snapshot = messages();
    let detail_snapshot = detail();
    let selected_account_id = selected_account();
    let selected_folder_id = selected_folder();
    let selected_message_id = selected_message();
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
    let can_backfill_active_folder = active_folder
        .as_ref()
        .map(|folder| folder.selectable && !folder.sync_completed)
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
        let last_cached_cursor = messages.peek().last().map(|message| message.uid.to_string());
        spawn(async move {
            backfilling_messages.set(true);
            sync_status.set(Some("Syncing older messages".to_string()));
            error.set(None);
            match use_mail::sync_folder(&account_id, &folder_id, Some(MAIL_BACKFILL_PAGE_SIZE)).await {
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
                                        account_enabled.set(account.enabled);
                                        account_sync_enabled.set(account.sync_enabled);
                                        account_sync_interval_minutes.set(
                                            account
                                                .sync_interval_secs
                                                .map(sync_interval_minutes_value)
                                                .unwrap_or_default(),
                                        );
                                        confirming_account_delete.set(false);
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
                            compose_identity_id.set(String::new());
                            compose_to.set(String::new());
                            compose_cc.set(String::new());
                            compose_bcc.set(String::new());
                            compose_subject.set(String::new());
                            compose_body.set(String::new());
                            show_compose.set(true);
                        },
                        IconSend { class: "w-4 h-4".to_string() }
                        span { "Compose" }
                    }
                    button {
                        class: "btn btn-sm btn-outline gap-2",
                        disabled: syncing() || selected_account_id.is_empty(),
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
                        class: "btn btn-sm btn-outline",
                        disabled: selected_account_id.is_empty() || syncing(),
                        onclick: move |_| {
                            let account_id = selected_account();
                            if account_id.is_empty() {
                                return;
                            }
                            spawn(async move {
                                error.set(None);
                                notice.set(None);
                                match use_mail::test_imap(&account_id).await {
                                    Ok(_) => notice.set(Some("IMAP test passed".to_string())),
                                    Err(e) => error.set(Some(e)),
                                }
                            });
                        },
                        "Test IMAP"
                    }
                    button {
                        class: "btn btn-sm btn-outline",
                        disabled: selected_account_id.is_empty() || syncing(),
                        onclick: move |_| {
                            let account_id = selected_account();
                            if account_id.is_empty() {
                                return;
                            }
                            spawn(async move {
                                error.set(None);
                                notice.set(None);
                                match use_mail::test_smtp(&account_id).await {
                                    Ok(_) => notice.set(Some("SMTP test passed".to_string())),
                                    Err(e) => error.set(Some(e)),
                                }
                            });
                        },
                        "Test SMTP"
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
                    section { class: "flex min-h-[18rem] flex-col border border-base-300 bg-base-100 lg:min-h-0",
                        div { class: "border-b border-base-300 px-3 py-2",
                            div { class: "text-xs font-semibold uppercase text-base-content/60", "Accounts" }
                        }
                        div { class: "max-h-52 overflow-y-auto border-b border-base-300 lg:max-h-60",
                            for account in accounts_snapshot.clone() {
                                {
                                    let id = account.id.clone();
                                    let active = id == selected_account_id;
                                    rsx! {
                                        button {
                                            key: "{id}",
                                            class: if active {
                                                "block w-full border-l-4 border-primary bg-primary/10 px-3 py-2 text-left"
                                            } else {
                                                "block w-full border-l-4 border-transparent px-3 py-2 text-left hover:bg-base-200"
                                            },
                                            onclick: move |_| {
                                                let account_id = id.clone();
                                                spawn(async move {
                                                    selected_account.set(account_id.clone());
                                                    selected_folder.set(String::new());
                                                    selected_message.set(String::new());
                                                    move_target_folder.set(String::new());
                                                    messages.set(Vec::new());
                                                    message_next_cursor.set(None);
                                                    message_has_more.set(false);
                                                    detail.set(None);
                                                    error.set(None);
                                                    match use_mail::list_folders(&account_id).await {
                                                        Ok(rows) => folders.set(rows),
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            div { class: "truncate text-sm font-medium", "{account.display_name}" }
                                            div { class: "truncate text-xs text-base-content/60", "{account.email_address}" }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center justify-between border-b border-base-300 px-3 py-2",
                            div { class: "text-xs font-semibold uppercase text-base-content/60", "Folders" }
                            button {
                                class: "btn btn-ghost btn-xs",
                                disabled: selected_account_id.is_empty(),
                                onclick: move |_| {
                                    let account_id = selected_account();
                                    if account_id.is_empty() {
                                        return;
                                    }
                                    spawn(async move {
                                        error.set(None);
                                        match use_mail::refresh_folders(&account_id).await {
                                            Ok(rows) => folders.set(rows),
                                            Err(e) => error.set(Some(e)),
                                        }
                                    });
                                },
                                IconRefreshCw { class: "w-4 h-4".to_string() }
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
                                                    spawn(async move {
                                                        selected_folder.set(folder_id.clone());
                                                        selected_message.set(String::new());
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
                                                if let Some(unseen) = folder.unseen {
                                                    if unseen > 0 {
                                                        span { class: "badge badge-sm", "{unseen}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    section { class: "flex min-h-[24rem] flex-col border border-base-300 bg-base-100 lg:min-h-0",
                        div { class: "flex items-center justify-between border-b border-base-300 px-3 py-2",
                            div {
                                div { class: "text-xs font-semibold uppercase text-base-content/60", "Messages" }
                                div { class: "truncate text-sm",
                                    if let Some(folder) = active_folder.as_ref() {
                                        "{folder.path}"
                                    } else {
                                        "Select a folder"
                                    }
                                }
                            }
                            div { class: "flex items-center gap-1",
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
                                    disabled: selected_account_id.is_empty() || selected_folder_id.is_empty() || syncing(),
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
                                            class: if active {
                                                "block w-full border-l-4 border-primary bg-primary/10 px-3 py-3 text-left"
                                            } else if !is_seen {
                                                "block w-full border-l-4 border-transparent bg-base-200/50 px-3 py-3 text-left hover:bg-base-200"
                                            } else {
                                                "block w-full border-l-4 border-transparent px-3 py-3 text-left hover:bg-base-200"
                                            },
                                            onclick: move |_| {
                                                let message_id = id.clone();
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

                    section { class: "flex min-h-[28rem] flex-col border border-base-300 bg-base-100 lg:min-h-0",
                        if loading_detail() {
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
                                rsx! {
                                    div { class: "border-b border-base-300 px-4 py-3",
                                        div { class: "flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between",
                                            div { class: "min-w-0",
                                                div { class: "text-lg font-semibold", "{message_subject(&row.message)}" }
                                                div { class: "mt-2 grid gap-1 text-sm text-base-content/70",
                                                    div { "From: {message_sender(&row.message)}" }
                                                    div { "To: {message_recipients(&row.message)}" }
                                                    div { "Date: {short_date(row.message.internal_date.as_deref().or(row.message.date.as_deref()))}" }
                                                }
                                            }
                                            div { class: "flex flex-wrap items-center gap-1",
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
                                                            option { value: "{folder.id}", "{folder.path}" }
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
                                            div {
                                                class: "max-w-none break-words text-sm leading-6 [&_a]:text-primary [&_blockquote]:border-l-2 [&_blockquote]:border-base-300 [&_blockquote]:pl-3 [&_blockquote]:text-base-content/70 [&_img]:max-w-full [&_table]:max-w-full",
                                                dangerous_inner_html: "{body}",
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

            if show_compose() {
                if let Some(account) = active_account.clone() {
                    {
                        let account_id = account.id.clone();
                        let account_from_label =
                            format!("{} <{}>", account.display_name, account.email_address);
                        let active_identities = active_identities.clone();
                        rsx! {
                            div { class: "modal modal-open",
                                div { class: "modal-box max-w-3xl",
                                    h2 { class: "text-xl font-semibold", "Compose" }
                                    div { class: "mt-1 truncate text-sm text-base-content/60", "{account.email_address}" }

                                    div { class: "mt-4 grid gap-3",
                                        label { class: "form-control",
                                            span { class: "label-text", "From" }
                                            select {
                                                class: "select select-bordered",
                                                value: "{compose_identity_id()}",
                                                onchange: move |e| compose_identity_id.set(e.value()),
                                                option { value: "", "{account_from_label}" }
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
                                        label { class: "form-control",
                                            span { class: "label-text", "Message" }
                                            textarea {
                                                class: "textarea textarea-bordered min-h-56 font-mono text-sm",
                                                value: "{compose_body()}",
                                                oninput: move |e| compose_body.set(e.value()),
                                            }
                                        }
                                    }

                                    div { class: "modal-action",
                                        button {
                                            class: "btn",
                                            disabled: sending_message(),
                                            onclick: move |_| show_compose.set(false),
                                            "Cancel"
                                        }
                                        button {
                                            class: "btn btn-primary gap-2",
                                            disabled: sending_message(),
                                            onclick: {
                                                let account_id = account_id.clone();
                                                move |_| {
                                                    let account_id = account_id.clone();
                                                    let identity_id = compose_identity_id();
                                                    let to = compose_to();
                                                    let cc = compose_cc();
                                                    let bcc = compose_bcc();
                                                    let subject = compose_subject();
                                                    let body = compose_body();
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
                                                            to: parse_compose_addresses(&to),
                                                            cc: parse_compose_addresses(&cc),
                                                            bcc: parse_compose_addresses(&bcc),
                                                            subject,
                                                            body_text: body,
                                                        };
                                                        match use_mail::send_message(&account_id, &req).await {
                                                            Ok(sent) => {
                                                                notice.set(Some(format!(
                                                                    "Message sent ({})",
                                                                    sent_copy_status_label(sent.sent_copy_status),
                                                                )));
                                                                show_compose.set(false);
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
                                div { class: "modal-backdrop", onclick: move |_| show_compose.set(false) }
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
                                            enabled: true,
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
                                                move_target_folder.set(String::new());
                                                messages.set(Vec::new());
                                                message_next_cursor.set(None);
                                                message_has_more.set(false);
                                                detail.set(None);
                                                if let Ok(list) = use_mail::list_accounts().await {
                                                    accounts.set(list);
                                                }
                                                match use_mail::refresh_folders(&account.id).await {
                                                    Ok(rows) => folders.set(rows),
                                                    Err(e) => error.set(Some(e)),
                                                }
                                                show_setup.set(false);
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
                                            checked: account_enabled(),
                                            onchange: move |e| account_enabled.set(e.checked()),
                                        }
                                        span {
                                            span { class: "block text-sm font-medium", "Enabled" }
                                            span { class: "block text-xs text-base-content/60", "Disabled accounts remain configured but hidden from future automatic sync." }
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
                                            span { class: "block text-sm font-medium", "Include in scheduled sync" }
                                            span { class: "block text-xs text-base-content/60", "Manual sync remains available from the mail view." }
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
                                                                    selected_account.set(next.clone());
                                                                    selected_folder.set(String::new());
                                                                    selected_message.set(String::new());
                                                                    move_target_folder.set(String::new());
                                                                    messages.set(Vec::new());
                                                                    message_next_cursor.set(None);
                                                                    message_has_more.set(false);
                                                                    detail.set(None);
                                                                    if next.is_empty() {
                                                                        folders.set(Vec::new());
                                                                    } else {
                                                                        match use_mail::list_folders(&next).await {
                                                                            Ok(rows) => folders.set(rows),
                                                                            Err(e) => error.set(Some(e)),
                                                                        }
                                                                    }
                                                                    account_settings.set(None);
                                                                    notice.set(Some("Mail account deleted".to_string()));
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
                                                    let enabled = account_enabled();
                                                    let sync_enabled = account_sync_enabled();
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
                                                            enabled: Some(enabled),
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
                                                                accounts.set(current);
                                                                selected_account.set(updated.id.clone());
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
                                div { class: "mt-1 truncate text-sm text-base-content/60", "{folder.path}" }

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
