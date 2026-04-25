use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use uncloud_common::{
    ServerEvent, SyncClientOs, SyncEventResponse, SyncEventSource, SyncOperation,
};

use crate::hooks::{
    use_events::use_events,
    use_sync_events::{SyncEventsFilter, list_sync_events},
};

const PAGE_LIMIT: u32 = 100;
const LIVE_CAP: usize = 1000; // keep the signal bounded under heavy firehose

#[component]
pub fn ActivitySection() -> Element {
    let mut events = use_signal(Vec::<SyncEventResponse>::new);
    let mut loading = use_signal(|| true);
    let mut loading_more = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut has_more = use_signal(|| false);

    let mut q = use_signal(String::new);
    let mut client = use_signal(String::new);
    let mut source_filter = use_signal(|| None::<SyncEventSource>);

    // Generation counter so a stale response doesn't clobber a fresh one.
    let mut fetch_gen = use_signal(|| 0u32);

    use_effect(move || {
        let qv = q();
        let cv = client();
        let srcs: Vec<SyncEventSource> = source_filter().into_iter().collect();

        let my_fetch_gen = *fetch_gen.peek() + 1;
        *fetch_gen.write() = my_fetch_gen;

        spawn(async move {
            loading.set(true);
            let filter = SyncEventsFilter {
                q: qv,
                client: cv,
                sources: srcs,
                before: None,
                limit: PAGE_LIMIT,
            };
            match list_sync_events(filter).await {
                Ok(resp) => {
                    if *fetch_gen.peek() == my_fetch_gen {
                        events.set(resp.events);
                        has_more.set(resp.has_more);
                        error.set(None);
                    }
                }
                Err(e) => {
                    if *fetch_gen.peek() == my_fetch_gen {
                        error.set(Some(e));
                    }
                }
            }
            if *fetch_gen.peek() == my_fetch_gen {
                loading.set(false);
            }
        });
    });

    // Live updates — prepend matching events.
    use_events(move |ev| {
        if let ServerEvent::SyncEventAppended { event: se } = ev {
            if matches_filter(
                &se,
                &q.peek(),
                &client.peek(),
                source_filter.peek().as_ref(),
            ) {
                let mut list = events.write();
                list.insert(0, se);
                if list.len() > LIVE_CAP {
                    list.truncate(LIVE_CAP);
                }
            }
        }
    });

    let on_load_more = move |_| {
        let oldest = events.peek().last().map(|e| e.timestamp);
        let Some(before) = oldest else {
            return;
        };
        let qv = q.peek().clone();
        let cv = client.peek().clone();
        let srcs: Vec<SyncEventSource> = source_filter.peek().into_iter().collect();
        spawn(async move {
            loading_more.set(true);
            let filter = SyncEventsFilter {
                q: qv,
                client: cv,
                sources: srcs,
                before: Some(before),
                limit: PAGE_LIMIT,
            };
            match list_sync_events(filter).await {
                Ok(resp) => {
                    let mut list = events.write();
                    list.extend(resp.events);
                    has_more.set(resp.has_more);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading_more.set(false);
        });
    };

    let reset_filters = move |_| {
        q.set(String::new());
        client.set(String::new());
        source_filter.set(None);
    };

    rsx! {
        div { class: "card bg-base-100 shadow",
            div { class: "card-body gap-4",
                h2 { class: "card-title text-lg", "Activity" }
                p { class: "text-sm text-base-content/60 -mt-2",
                    "Recent change-inducing operations against your files — uploads, renames, moves, deletes, restores, content replacements. Pushed live; server purges events older than the retention window."
                }

                // Filter bar
                div { class: "flex flex-wrap items-end gap-2",
                    div { class: "form-control flex-1 min-w-48",
                        label { class: "label py-0",
                            span { class: "label-text text-xs", "Path contains" }
                        }
                        input {
                            class: "input input-bordered input-sm",
                            r#type: "text",
                            placeholder: "e.g. vacation/cat.jpg",
                            value: "{q()}",
                            oninput: move |e| q.set(e.value()),
                        }
                    }
                    div { class: "form-control flex-1 min-w-40",
                        label { class: "label py-0",
                            span { class: "label-text text-xs", "Client contains" }
                        }
                        input {
                            class: "input input-bordered input-sm",
                            r#type: "text",
                            placeholder: "e.g. laptop",
                            value: "{client()}",
                            oninput: move |e| client.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label py-0",
                            span { class: "label-text text-xs", "Source" }
                        }
                        select {
                            class: "select select-bordered select-sm",
                            value: source_filter().map(source_code).unwrap_or("").to_string(),
                            onchange: move |e| {
                                let v = e.value();
                                source_filter.set(parse_source_code(&v));
                            },
                            option { value: "", "All" }
                            option { value: "user_web", "Web" }
                            option { value: "user_desktop", "Desktop" }
                            option { value: "user_mobile", "Mobile" }
                            option { value: "sync", "Sync" }
                            option { value: "admin", "Admin" }
                            option { value: "public", "Public" }
                            option { value: "system", "System" }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label py-0", span { class: "label-text text-xs invisible", "_" } }
                        button {
                            class: "btn btn-ghost btn-sm",
                            r#type: "button",
                            onclick: reset_filters,
                            "Reset"
                        }
                    }
                }

                if let Some(err) = error() {
                    div { class: "alert alert-error text-sm", span { "{err}" } }
                }

                if loading() {
                    div { class: "flex justify-center py-6",
                        span { class: "loading loading-spinner loading-md" }
                    }
                } else if events().is_empty() {
                    p { class: "text-base-content/50 text-sm py-4 text-center",
                        "No activity matches the current filter."
                    }
                } else {
                    div { class: "overflow-x-auto",
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { class: "whitespace-nowrap", "Time" }
                                    th { "Operation" }
                                    th { "Path" }
                                    th { "Source" }
                                    th { "Client" }
                                }
                            }
                            tbody {
                                for ev in events() {
                                    {
                                        let key = ev.id.clone();
                                        rsx! {
                                            tr { key: "{key}",
                                                td { class: "whitespace-nowrap text-xs text-base-content/70",
                                                    {format_ts(ev.timestamp)}
                                                }
                                                td {
                                                    span {
                                                        class: "badge badge-sm {operation_badge_class(ev.operation)}",
                                                        {operation_label(ev.operation)}
                                                    }
                                                    if let Some(n) = ev.affected_count {
                                                        span { class: "ml-1 text-xs text-base-content/60",
                                                            "({n})"
                                                        }
                                                    }
                                                }
                                                td { class: "font-mono text-xs break-all",
                                                    {ev.path.clone()}
                                                    if let Some(np) = ev.new_path.clone() {
                                                        span { class: "mx-1 text-base-content/50", " → " }
                                                        span { "{np}" }
                                                    }
                                                }
                                                td { class: "text-xs",
                                                    span {
                                                        class: "badge badge-ghost badge-sm",
                                                        {source_label(ev.source)}
                                                    }
                                                }
                                                td { class: "text-xs text-base-content/70",
                                                    {
                                                        match (ev.client_id.clone(), ev.client_os) {
                                                            (Some(c), Some(os)) => format!("{} · {}", c, os_label(os)),
                                                            (Some(c), None) => c,
                                                            (None, Some(os)) => os_label(os).to_string(),
                                                            (None, None) => "—".to_string(),
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

                    if has_more() {
                        div { class: "card-actions justify-center pt-2",
                            button {
                                class: "btn btn-sm btn-outline",
                                disabled: loading_more(),
                                onclick: on_load_more,
                                if loading_more() {
                                    span { class: "loading loading-spinner loading-xs" }
                                }
                                "Load more"
                            }
                        }
                    }
                }
            }
        }
    }
}

fn matches_filter(
    ev: &SyncEventResponse,
    q: &str,
    client: &str,
    src: Option<&SyncEventSource>,
) -> bool {
    if !q.is_empty() {
        let ql = q.to_lowercase();
        let hit_path = ev.path.to_lowercase().contains(&ql);
        let hit_new = ev
            .new_path
            .as_deref()
            .map(|p| p.to_lowercase().contains(&ql))
            .unwrap_or(false);
        if !hit_path && !hit_new {
            return false;
        }
    }
    if !client.is_empty() {
        let cl = client.to_lowercase();
        let hit = ev
            .client_id
            .as_deref()
            .map(|c| c.to_lowercase().contains(&cl))
            .unwrap_or(false);
        if !hit {
            return false;
        }
    }
    if let Some(s) = src {
        if ev.source != *s {
            return false;
        }
    }
    true
}

fn format_ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn operation_label(op: SyncOperation) -> &'static str {
    match op {
        SyncOperation::Created => "Created",
        SyncOperation::Renamed => "Renamed",
        SyncOperation::Moved => "Moved",
        SyncOperation::Deleted => "Deleted",
        SyncOperation::Restored => "Restored",
        SyncOperation::PermanentlyDeleted => "Purged",
        SyncOperation::ContentReplaced => "Replaced",
        SyncOperation::Copied => "Copied",
    }
}

fn operation_badge_class(op: SyncOperation) -> &'static str {
    match op {
        SyncOperation::Created => "badge-success",
        SyncOperation::Renamed | SyncOperation::Moved => "badge-info",
        SyncOperation::Deleted => "badge-warning",
        SyncOperation::PermanentlyDeleted => "badge-error",
        SyncOperation::Restored => "badge-success",
        SyncOperation::ContentReplaced => "badge-info",
        SyncOperation::Copied => "badge-ghost",
    }
}

fn source_label(s: SyncEventSource) -> &'static str {
    match s {
        SyncEventSource::UserWeb => "Web",
        SyncEventSource::UserDesktop => "Desktop",
        SyncEventSource::UserMobile => "Mobile",
        SyncEventSource::Sync => "Sync",
        SyncEventSource::Admin => "Admin",
        SyncEventSource::Public => "Public",
        SyncEventSource::System => "System",
    }
}

fn source_code(s: SyncEventSource) -> &'static str {
    crate::hooks::use_sync_events::source_code(s)
}

fn parse_source_code(v: &str) -> Option<SyncEventSource> {
    match v {
        "user_web" => Some(SyncEventSource::UserWeb),
        "user_desktop" => Some(SyncEventSource::UserDesktop),
        "user_mobile" => Some(SyncEventSource::UserMobile),
        "sync" => Some(SyncEventSource::Sync),
        "admin" => Some(SyncEventSource::Admin),
        "public" => Some(SyncEventSource::Public),
        "system" => Some(SyncEventSource::System),
        _ => None,
    }
}

fn os_label(os: SyncClientOs) -> &'static str {
    match os {
        SyncClientOs::Linux => "Linux",
        SyncClientOs::Windows => "Windows",
        SyncClientOs::Macos => "macOS",
        SyncClientOs::Android => "Android",
        SyncClientOs::Ios => "iOS",
        SyncClientOs::Unknown => "Unknown",
    }
}
