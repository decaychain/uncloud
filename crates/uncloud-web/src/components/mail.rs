use dioxus::prelude::*;
use uncloud_common::{
    CreateMailAccountRequest, MailAccountResponse, MailFolderResponse, MailMessageDetailResponse,
    MailMessageSummaryResponse, MailSecurity, MailServerSettings,
};

use crate::components::icons::{IconFileText, IconFolder, IconMail, IconPlus, IconRefreshCw};
use crate::hooks::use_mail;

#[component]
pub fn MailPage() -> Element {
    let mut accounts = use_signal(Vec::<MailAccountResponse>::new);
    let mut folders = use_signal(Vec::<MailFolderResponse>::new);
    let mut messages = use_signal(Vec::<MailMessageSummaryResponse>::new);
    let mut detail = use_signal(|| None::<MailMessageDetailResponse>);
    let mut selected_account = use_signal(String::new);
    let mut selected_folder = use_signal(String::new);
    let mut selected_message = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut syncing = use_signal(|| false);
    let mut loading_detail = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<String>);
    let mut show_setup = use_signal(|| false);

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
        spawn(async move {
            loading.set(true);
            match use_mail::list_accounts().await {
                Ok(list) => {
                    let first = list.first().map(|a| a.id.clone()).unwrap_or_default();
                    accounts.set(list);
                    selected_account.set(first.clone());
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

    let accounts_snapshot = accounts();
    let folders_snapshot = folders();
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

    rsx! {
        div { class: "space-y-3",
            div { class: "flex flex-col gap-3 md:flex-row md:items-center md:justify-between",
                div {
                    h1 { class: "text-2xl font-semibold tracking-normal", "Mail" }
                    div { class: "text-sm text-base-content/60",
                        if let Some(account) = active_account.as_ref() {
                            "{account.email_address}"
                        } else {
                            "No account selected"
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
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
                                error.set(None);
                                match use_mail::sync_account(&account_id, Some(25)).await {
                                    Ok(_) => {
                                        notice.set(Some("Account sync finished".to_string()));
                                        if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                            folders.set(rows);
                                        }
                                        let folder_id = selected_folder();
                                        if !folder_id.is_empty() {
                                            if let Ok(rows) = use_mail::list_messages(&account_id, &folder_id, 100).await {
                                                messages.set(rows);
                                            }
                                        }
                                    }
                                    Err(e) => error.set(Some(e)),
                                }
                                syncing.set(false);
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

            if let Some(e) = error() {
                div { class: "alert alert-error py-2 text-sm", "{e}" }
            }
            if let Some(message) = notice() {
                div { class: "alert alert-info py-2 text-sm", "{message}" }
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
                                                    messages.set(Vec::new());
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
                                    rsx! {
                                        button {
                                            key: "{id}",
                                            class: if active {
                                                "flex w-full items-center gap-2 bg-primary/10 px-2 py-1.5 text-left text-sm text-primary"
                                            } else if folder.selectable {
                                                "flex w-full items-center gap-2 px-2 py-1.5 text-left text-sm hover:bg-base-200"
                                            } else {
                                                "flex w-full items-center gap-2 px-2 py-1.5 text-left text-sm text-base-content/45"
                                            },
                                            style: "padding-left: {0.5 + depth as f32 * 1.0}rem",
                                            disabled: !folder.selectable,
                                            onclick: move |_| {
                                                let account_id = account_id.clone();
                                                let folder_id = id.clone();
                                                spawn(async move {
                                                    selected_folder.set(folder_id.clone());
                                                    selected_message.set(String::new());
                                                    detail.set(None);
                                                    error.set(None);
                                                    match use_mail::list_messages(&account_id, &folder_id, 100).await {
                                                        Ok(rows) => messages.set(rows),
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            IconFolder { class: "h-4 w-4".to_string() }
                                            span { class: "min-w-0 flex-1 truncate", "{folder.name}" }
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
                                        error.set(None);
                                        match use_mail::sync_folder(&account_id, &folder_id, Some(50)).await {
                                            Ok(_) => {
                                                notice.set(Some("Folder sync finished".to_string()));
                                                if let Ok(rows) = use_mail::list_folders(&account_id).await {
                                                    folders.set(rows);
                                                }
                                                if let Ok(rows) = use_mail::list_messages(&account_id, &folder_id, 100).await {
                                                    messages.set(rows);
                                                }
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        syncing.set(false);
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
                        div { class: "min-h-0 flex-1 overflow-y-auto",
                            if messages_snapshot.is_empty() {
                                div { class: "flex h-52 items-center justify-center px-4 text-sm text-base-content/60",
                                    "No cached messages"
                                }
                            }
                            for message in messages_snapshot.clone() {
                                {
                                    let id = message.id.clone();
                                    let active = id == selected_message_id;
                                    rsx! {
                                        button {
                                            key: "{id}",
                                            class: if active {
                                                "block w-full border-l-4 border-primary bg-primary/10 px-3 py-3 text-left"
                                            } else {
                                                "block w-full border-l-4 border-transparent px-3 py-3 text-left hover:bg-base-200"
                                            },
                                            onclick: move |_| {
                                                let message_id = id.clone();
                                                spawn(async move {
                                                    selected_message.set(message_id.clone());
                                                    loading_detail.set(true);
                                                    error.set(None);
                                                    match use_mail::get_message(&message_id).await {
                                                        Ok(row) => detail.set(Some(row)),
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                    loading_detail.set(false);
                                                });
                                            },
                                            div { class: "flex items-start justify-between gap-2",
                                                div { class: "min-w-0 flex-1",
                                                    div { class: "truncate text-sm font-medium", "{message_subject(&message)}" }
                                                    div { class: "truncate text-xs text-base-content/60", "{message_sender(&message)}" }
                                                }
                                                div { class: "shrink-0 text-xs text-base-content/50", "{short_date(message.internal_date.as_deref().or(message.date.as_deref()))}" }
                                            }
                                            div { class: "mt-1 line-clamp-2 text-xs text-base-content/60",
                                                "{message.snippet.clone().unwrap_or_default()}"
                                            }
                                        }
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
                            div { class: "border-b border-base-300 px-4 py-3",
                                div { class: "text-lg font-semibold", "{message_subject(&row.message)}" }
                                div { class: "mt-2 grid gap-1 text-sm text-base-content/70",
                                    div { "From: {message_sender(&row.message)}" }
                                    div { "To: {message_recipients(&row.message)}" }
                                    div { "Date: {short_date(row.message.internal_date.as_deref().or(row.message.date.as_deref()))}" }
                                }
                            }
                            div { class: "min-h-0 flex-1 overflow-y-auto p-4",
                                if let Some(body) = row.body_text {
                                    pre { class: "whitespace-pre-wrap break-words font-sans text-sm leading-6", "{body}" }
                                } else {
                                    div { class: "flex h-full items-center justify-center text-sm text-base-content/60",
                                        "Body unavailable"
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
                                        };
                                        match use_mail::create_account(&req).await {
                                            Ok(account) => {
                                                if !pass.trim().is_empty() {
                                                    let _ = use_mail::set_credential(&account.id, &pass).await;
                                                }
                                                selected_account.set(account.id.clone());
                                                selected_folder.set(String::new());
                                                selected_message.set(String::new());
                                                messages.set(Vec::new());
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

fn folder_depth(folder: &MailFolderResponse) -> usize {
    folder
        .delimiter
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|delimiter| folder.path.matches(delimiter).count())
        .unwrap_or(0)
}

fn message_subject(message: &MailMessageSummaryResponse) -> String {
    message
        .subject
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "(no subject)".to_string())
}

fn message_sender(message: &MailMessageSummaryResponse) -> String {
    message
        .from
        .first()
        .map(|address| {
            address
                .name
                .as_ref()
                .filter(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| address.address.clone())
        })
        .unwrap_or_else(|| "Unknown sender".to_string())
}

fn message_recipients(message: &MailMessageSummaryResponse) -> String {
    if message.to.is_empty() {
        return "Undisclosed recipients".to_string();
    }
    message
        .to
        .iter()
        .map(|address| {
            address
                .name
                .as_ref()
                .filter(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| address.address.clone())
        })
        .collect::<Vec<_>>()
        .join(", ")
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
