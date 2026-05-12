//! Finance tracker — foundation UI (v0).
//!
//! Single page with three internal tabs: Transactions, Accounts, Categories.
//! Deliberately primitive: DaisyUI tables + modal forms. Per-currency
//! totals shown on Accounts. CSV import wired through the Transactions tab.

use std::collections::HashMap;

use dioxus::prelude::*;
use uncloud_common::{
    AccountResponse, CreateAccountRequest, CreateFinanceCategoryRequest, CreateTransactionRequest,
    FinanceCategoryResponse, ImportCsvResponse, ImportProfileInfo, TransactionResponse,
    UpdateAccountRequest, UpdateFinanceCategoryRequest, UpdateTransactionRequest,
};

use crate::hooks::use_finance;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Transactions,
    Accounts,
    Categories,
}

#[component]
pub fn FinancePage() -> Element {
    let mut tab = use_signal(|| Tab::Transactions);

    rsx! {
        div { class: "p-4 lg:p-6 max-w-6xl mx-auto",
            div { role: "tablist", class: "tabs tabs-boxed mb-4 w-fit",
                button {
                    role: "tab",
                    class: if tab() == Tab::Transactions { "tab tab-active" } else { "tab" },
                    onclick: move |_| tab.set(Tab::Transactions),
                    "Transactions"
                }
                button {
                    role: "tab",
                    class: if tab() == Tab::Accounts { "tab tab-active" } else { "tab" },
                    onclick: move |_| tab.set(Tab::Accounts),
                    "Accounts"
                }
                button {
                    role: "tab",
                    class: if tab() == Tab::Categories { "tab tab-active" } else { "tab" },
                    onclick: move |_| tab.set(Tab::Categories),
                    "Categories"
                }
            }

            match tab() {
                Tab::Transactions => rsx! { TransactionsTab {} },
                Tab::Accounts => rsx! { AccountsTab {} },
                Tab::Categories => rsx! { CategoriesTab {} },
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Money formatting
// ─────────────────────────────────────────────────────────────────────────

/// Format minor units (cents) as "1234.56". Negative shows a leading minus.
fn format_money(minor: i64, currency: &str) -> String {
    let neg = minor < 0;
    let abs = minor.unsigned_abs();
    let major = abs / 100;
    let cents = abs % 100;
    if neg {
        format!("-{}.{:02} {}", major, cents, currency)
    } else {
        format!("{}.{:02} {}", major, cents, currency)
    }
}

/// Parse "1234.56" / "1234,56" / "-1234" / "1234" into minor units.
fn parse_money(input: &str) -> Result<i64, String> {
    let s = input.trim().replace(',', ".");
    if s.is_empty() {
        return Err("Amount is required".into());
    }
    let (sign, body) = if let Some(rest) = s.strip_prefix('-') {
        (-1i64, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (1i64, rest)
    } else {
        (1i64, s.as_str())
    };
    let (major_str, cents_str) = match body.split_once('.') {
        Some((m, c)) => (m, c),
        None => (body, "00"),
    };
    let major: i64 = major_str.parse().map_err(|_| "Invalid amount".to_string())?;
    let cents_padded = if cents_str.len() == 1 {
        format!("{}0", cents_str)
    } else if cents_str.len() >= 2 {
        cents_str[..2].to_string()
    } else {
        "00".to_string()
    };
    let cents: i64 = cents_padded.parse().map_err(|_| "Invalid amount".to_string())?;
    Ok(sign * (major * 100 + cents))
}

fn today_iso() -> String {
    js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default()
        .split('T').next().unwrap_or("").to_string()
}

// ─────────────────────────────────────────────────────────────────────────
// Accounts tab
// ─────────────────────────────────────────────────────────────────────────

#[component]
fn AccountsTab() -> Element {
    let mut accounts: Signal<Vec<AccountResponse>> = use_signal(Vec::new);
    let mut balances: Signal<HashMap<String, i64>> = use_signal(HashMap::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut show_create = use_signal(|| false);
    let mut edit_target: Signal<Option<AccountResponse>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match use_finance::list_accounts().await {
                Ok(list) => {
                    let mut b = HashMap::new();
                    for a in &list {
                        if let Ok(bal) = use_finance::account_balance(&a.id).await {
                            b.insert(a.id.clone(), bal.balance_minor);
                        }
                    }
                    balances.set(b);
                    accounts.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    // Per-currency totals across non-archived accounts
    let totals: HashMap<String, i64> = {
        let mut t: HashMap<String, i64> = HashMap::new();
        for a in accounts().iter() {
            if a.archived_at.is_some() {
                continue;
            }
            let bal = balances().get(&a.id).copied().unwrap_or(a.opening_balance_minor);
            *t.entry(a.currency.clone()).or_insert(0) += bal;
        }
        t
    };

    rsx! {
        div { class: "flex justify-between items-center mb-4",
            h2 { class: "text-xl font-semibold", "Accounts" }
            button {
                class: "btn btn-primary btn-sm",
                onclick: move |_| show_create.set(true),
                "Add account"
            }
        }

        if !totals.is_empty() {
            div { class: "card bg-base-200 mb-4",
                div { class: "card-body py-3",
                    div { class: "text-sm font-medium opacity-70", "Net per currency" }
                    div { class: "flex flex-wrap gap-4 mt-1",
                        {totals.iter().map(|(cur, amt)| rsx! {
                            div { key: "{cur}", class: "text-lg font-mono",
                                "{format_money(*amt, cur)}"
                            }
                        })}
                    }
                }
            }
        }

        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && accounts().is_empty() {
            div { class: "flex justify-center py-8", span { class: "loading loading-spinner loading-lg" } }
        } else if accounts().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No accounts yet." }
                p { class: "text-sm mt-2", "Add an account to start tracking transactions." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead {
                        tr {
                            th { "Name" }
                            th { "Type" }
                            th { "Currency" }
                            th { class: "text-right", "Balance" }
                            th { "" }
                        }
                    }
                    tbody {
                        {accounts().iter().map(|a| {
                            let a_edit = a.clone();
                            let a_id = a.id.clone();
                            let bal = balances().get(&a.id).copied().unwrap_or(a.opening_balance_minor);
                            let archived = a.archived_at.is_some();
                            rsx! {
                                tr { key: "{a.id}", class: if archived { "opacity-50" } else { "" },
                                    td {
                                        "{a.name}"
                                        if archived { span { class: "badge badge-ghost ml-2 text-xs", "archived" } }
                                    }
                                    td { "{a.account_type}" }
                                    td { "{a.currency}" }
                                    td { class: "text-right font-mono", "{format_money(bal, &a.currency)}" }
                                    td { class: "text-right",
                                        button {
                                            class: "btn btn-ghost btn-xs",
                                            onclick: move |_| edit_target.set(Some(a_edit.clone())),
                                            "Edit"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs text-error",
                                            onclick: move |_| {
                                                let id = a_id.clone();
                                                spawn(async move {
                                                    match use_finance::delete_account(&id).await {
                                                        Ok(_) => refresh += 1,
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        })}
                    }
                }
            }
        }

        if show_create() {
            AccountFormModal {
                initial: None,
                on_close: move |_| show_create.set(false),
                on_saved: move |_| { show_create.set(false); refresh += 1; },
            }
        }
        if let Some(a) = edit_target() {
            AccountFormModal {
                key: "{a.id}",
                initial: Some(a.clone()),
                on_close: move |_| edit_target.set(None),
                on_saved: move |_| { edit_target.set(None); refresh += 1; },
            }
        }
    }
}

#[component]
fn AccountFormModal(
    initial: Option<AccountResponse>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let editing = initial.is_some();
    let mut name = use_signal(|| initial.as_ref().map(|a| a.name.clone()).unwrap_or_default());
    let mut account_type = use_signal(|| {
        initial.as_ref().map(|a| a.account_type.clone()).unwrap_or_else(|| "checking".to_string())
    });
    let mut currency = use_signal(|| {
        initial.as_ref().map(|a| a.currency.clone()).unwrap_or_else(|| "EUR".to_string())
    });
    let mut opening = use_signal(|| {
        initial.as_ref().map(|a| format_amount_input(a.opening_balance_minor)).unwrap_or_else(|| "0.00".to_string())
    });
    let mut archived = use_signal(|| initial.as_ref().map(|a| a.archived_at.is_some()).unwrap_or(false));
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut saving = use_signal(|| false);
    let initial_id = initial.as_ref().map(|a| a.id.clone());
    let editing_locked_currency = editing;

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-3",
                    if editing { "Edit account" } else { "Add account" }
                }
                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3 text-sm", "{e}" }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Name" } }
                    input {
                        class: "input input-bordered",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Type" } }
                    select {
                        class: "select select-bordered",
                        value: "{account_type}",
                        onchange: move |e| account_type.set(e.value()),
                        option { value: "checking", "Checking" }
                        option { value: "savings", "Savings" }
                        option { value: "credit_card", "Credit card" }
                        option { value: "cash", "Cash" }
                        option { value: "other", "Other" }
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label",
                        span { class: "label-text", "Currency (ISO 4217)" }
                        if editing_locked_currency {
                            span { class: "label-text-alt opacity-60", "locked after creation" }
                        }
                    }
                    input {
                        class: "input input-bordered uppercase",
                        value: "{currency}",
                        disabled: editing_locked_currency,
                        maxlength: 3,
                        oninput: move |e| currency.set(e.value().to_uppercase()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label",
                        span { class: "label-text", "Opening balance" }
                        span { class: "label-text-alt opacity-60", "decimal" }
                    }
                    input {
                        class: "input input-bordered",
                        value: "{opening}",
                        oninput: move |e| opening.set(e.value()),
                    }
                }
                if editing {
                    div { class: "form-control mt-3",
                        label { class: "label cursor-pointer justify-start gap-2",
                            input {
                                r#type: "checkbox",
                                class: "checkbox checkbox-sm",
                                checked: archived(),
                                onchange: move |e| archived.set(e.checked()),
                            }
                            span { class: "label-text", "Archived" }
                        }
                    }
                }
                div { class: "modal-action",
                    button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    button {
                        class: "btn btn-primary",
                        disabled: saving(),
                        onclick: move |_| {
                            let id_opt = initial_id.clone();
                            spawn(async move {
                                error.set(None);
                                saving.set(true);
                                let opening_minor = match parse_money(&opening()) {
                                    Ok(v) => v,
                                    Err(e) => { error.set(Some(e)); saving.set(false); return; }
                                };
                                let result = if let Some(id) = id_opt {
                                    let req = UpdateAccountRequest {
                                        name: Some(name()),
                                        account_type: Some(account_type()),
                                        opening_balance_minor: Some(opening_minor),
                                        archived: Some(archived()),
                                    };
                                    use_finance::update_account(&id, &req).await.map(|_| ())
                                } else {
                                    let req = CreateAccountRequest {
                                        name: name(),
                                        account_type: account_type(),
                                        currency: currency(),
                                        opening_balance_minor: opening_minor,
                                    };
                                    use_finance::create_account(&req).await.map(|_| ())
                                };
                                match result {
                                    Ok(_) => on_saved.call(()),
                                    Err(e) => { error.set(Some(e)); saving.set(false); }
                                }
                            });
                        },
                        if saving() { "Saving…" } else { "Save" }
                    }
                }
            }
        }
    }
}

fn format_amount_input(minor: i64) -> String {
    let neg = minor < 0;
    let abs = minor.unsigned_abs();
    let major = abs / 100;
    let cents = abs % 100;
    if neg {
        format!("-{}.{:02}", major, cents)
    } else {
        format!("{}.{:02}", major, cents)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Categories tab
// ─────────────────────────────────────────────────────────────────────────

#[component]
fn CategoriesTab() -> Element {
    let mut categories: Signal<Vec<FinanceCategoryResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut show_create = use_signal(|| false);
    let mut edit_target: Signal<Option<FinanceCategoryResponse>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match use_finance::list_categories().await {
                Ok(list) => { categories.set(list); error.set(None); }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "flex justify-between items-center mb-4",
            h2 { class: "text-xl font-semibold", "Categories" }
            button {
                class: "btn btn-primary btn-sm",
                onclick: move |_| show_create.set(true),
                "Add category"
            }
        }
        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && categories().is_empty() {
            div { class: "flex justify-center py-8", span { class: "loading loading-spinner loading-lg" } }
        } else if categories().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No categories yet." }
                p { class: "text-sm mt-2", "Categories are reused across transactions and pending settlements." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead { tr { th { "Name" } th { "Parent" } th { "" } } }
                    tbody {
                        {categories().iter().map(|c| {
                            let parent_name = c.parent_id.as_ref().and_then(|pid| {
                                categories().iter().find(|p| &p.id == pid).map(|p| p.name.clone())
                            }).unwrap_or_default();
                            let c_edit = c.clone();
                            let c_id = c.id.clone();
                            rsx! {
                                tr { key: "{c.id}",
                                    td { "{c.name}" }
                                    td { class: "opacity-60", "{parent_name}" }
                                    td { class: "text-right",
                                        button {
                                            class: "btn btn-ghost btn-xs",
                                            onclick: move |_| edit_target.set(Some(c_edit.clone())),
                                            "Edit"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs text-error",
                                            onclick: move |_| {
                                                let id = c_id.clone();
                                                spawn(async move {
                                                    match use_finance::delete_category(&id).await {
                                                        Ok(_) => refresh += 1,
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        })}
                    }
                }
            }
        }
        if show_create() {
            CategoryFormModal {
                initial: None,
                all_categories: categories(),
                on_close: move |_| show_create.set(false),
                on_saved: move |_| { show_create.set(false); refresh += 1; },
            }
        }
        if let Some(c) = edit_target() {
            CategoryFormModal {
                key: "{c.id}",
                initial: Some(c.clone()),
                all_categories: categories(),
                on_close: move |_| edit_target.set(None),
                on_saved: move |_| { edit_target.set(None); refresh += 1; },
            }
        }
    }
}

#[component]
fn CategoryFormModal(
    initial: Option<FinanceCategoryResponse>,
    all_categories: Vec<FinanceCategoryResponse>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let editing = initial.is_some();
    let mut name = use_signal(|| initial.as_ref().map(|c| c.name.clone()).unwrap_or_default());
    let mut parent_id = use_signal(|| initial.as_ref().and_then(|c| c.parent_id.clone()).unwrap_or_default());
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut saving = use_signal(|| false);
    let initial_id = initial.as_ref().map(|c| c.id.clone());
    let self_id = initial_id.clone();
    let parent_candidates: Vec<FinanceCategoryResponse> = all_categories
        .into_iter()
        .filter(|c| c.parent_id.is_none() && Some(&c.id) != self_id.as_ref())
        .collect();

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-3",
                    if editing { "Edit category" } else { "Add category" }
                }
                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3 text-sm", "{e}" }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Name" } }
                    input {
                        class: "input input-bordered",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Parent (optional)" } }
                    select {
                        class: "select select-bordered",
                        value: "{parent_id}",
                        onchange: move |e| parent_id.set(e.value()),
                        option { value: "", "(top-level)" }
                        {parent_candidates.iter().map(|c| rsx! {
                            option { key: "{c.id}", value: "{c.id}", "{c.name}" }
                        })}
                    }
                }
                div { class: "modal-action",
                    button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    button {
                        class: "btn btn-primary",
                        disabled: saving(),
                        onclick: move |_| {
                            let id_opt = initial_id.clone();
                            spawn(async move {
                                error.set(None);
                                saving.set(true);
                                let result = if let Some(id) = id_opt {
                                    let req = UpdateFinanceCategoryRequest {
                                        name: Some(name()),
                                        parent_id: Some(parent_id()),
                                        colour: None,
                                    };
                                    use_finance::update_category(&id, &req).await.map(|_| ())
                                } else {
                                    let p = parent_id();
                                    let req = CreateFinanceCategoryRequest {
                                        name: name(),
                                        parent_id: if p.is_empty() { None } else { Some(p) },
                                        colour: None,
                                    };
                                    use_finance::create_category(&req).await.map(|_| ())
                                };
                                match result {
                                    Ok(_) => on_saved.call(()),
                                    Err(e) => { error.set(Some(e)); saving.set(false); }
                                }
                            });
                        },
                        if saving() { "Saving…" } else { "Save" }
                    }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Transactions tab
// ─────────────────────────────────────────────────────────────────────────

#[component]
fn TransactionsTab() -> Element {
    let mut transactions: Signal<Vec<TransactionResponse>> = use_signal(Vec::new);
    let mut total: Signal<u64> = use_signal(|| 0);
    let mut accounts: Signal<Vec<AccountResponse>> = use_signal(Vec::new);
    let mut categories: Signal<Vec<FinanceCategoryResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut show_create = use_signal(|| false);
    let mut show_import = use_signal(|| false);
    let mut edit_target: Signal<Option<TransactionResponse>> = use_signal(|| None);
    let mut filter_account: Signal<String> = use_signal(String::new);
    let mut only_uncat = use_signal(|| false);
    let mut skip = use_signal(|| 0u32);
    let page_size: u32 = 50;

    use_effect(move || {
        let _ = refresh();
        let _ = filter_account();
        let _ = only_uncat();
        let _ = skip();
        spawn(async move {
            loading.set(true);
            if accounts().is_empty() {
                if let Ok(a) = use_finance::list_accounts().await {
                    accounts.set(a);
                }
            }
            if categories().is_empty() {
                if let Ok(c) = use_finance::list_categories().await {
                    categories.set(c);
                }
            }
            let acc_filter = filter_account();
            let acc_opt = if acc_filter.is_empty() { None } else { Some(acc_filter.as_str()) };
            match use_finance::list_transactions(acc_opt, only_uncat(), page_size, skip()).await {
                Ok(resp) => {
                    transactions.set(resp.items);
                    total.set(resp.total);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let account_name = |id: &str| {
        accounts().iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| "?".into())
    };
    let category_name = |id: &str| {
        categories().iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_else(|| "?".into())
    };
    let accounts_for_select = accounts();
    let no_accounts = accounts_for_select.is_empty();

    rsx! {
        div { class: "flex justify-between items-center mb-4 gap-2 flex-wrap",
            h2 { class: "text-xl font-semibold", "Transactions" }
            div { class: "flex gap-2 items-center flex-wrap",
                select {
                    class: "select select-bordered select-sm",
                    value: "{filter_account}",
                    onchange: move |e| { filter_account.set(e.value()); skip.set(0); },
                    option { value: "", "All accounts" }
                    {accounts().iter().map(|a| rsx! {
                        option { key: "{a.id}", value: "{a.id}", "{a.name}" }
                    })}
                }
                label { class: "label cursor-pointer gap-2",
                    input {
                        r#type: "checkbox",
                        class: "checkbox checkbox-sm",
                        checked: only_uncat(),
                        onchange: move |e| { only_uncat.set(e.checked()); skip.set(0); },
                    }
                    span { class: "label-text", "Uncategorized only" }
                }
                button {
                    class: "btn btn-ghost btn-sm",
                    disabled: no_accounts,
                    onclick: move |_| show_import.set(true),
                    "Import CSV"
                }
                button {
                    class: "btn btn-primary btn-sm",
                    disabled: no_accounts,
                    onclick: move |_| show_create.set(true),
                    "Add transaction"
                }
            }
        }
        if no_accounts {
            div { class: "alert alert-info mb-3",
                "Add an account on the Accounts tab before recording transactions."
            }
        }
        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && transactions().is_empty() {
            div { class: "flex justify-center py-8", span { class: "loading loading-spinner loading-lg" } }
        } else if transactions().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No transactions match the current filters." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead {
                        tr {
                            th { "Date" }
                            th { "Account" }
                            th { "Description" }
                            th { "Category" }
                            th { class: "text-right", "Amount" }
                            th { "" }
                        }
                    }
                    tbody {
                        {transactions().iter().map(|t| {
                            let t_edit = t.clone();
                            let t_id = t.id.clone();
                            let date_short = t.date.split('T').next().unwrap_or(&t.date).to_string();
                            let cat = t.category_id.as_deref().map(|id| category_name(id));
                            rsx! {
                                tr { key: "{t.id}",
                                    td { class: "whitespace-nowrap font-mono text-sm", "{date_short}" }
                                    td { "{account_name(&t.account_id)}" }
                                    td {
                                        "{t.description}"
                                        if t.is_split { span { class: "badge badge-ghost ml-2 text-xs", "split" } }
                                    }
                                    td { class: "opacity-80",
                                        if let Some(name) = cat { "{name}" } else { span { class: "opacity-50 italic", "—" } }
                                    }
                                    td { class: "text-right font-mono",
                                        class: if t.amount_minor < 0 { "text-right font-mono text-error" } else { "text-right font-mono text-success" },
                                        "{format_money(t.amount_minor, &t.currency)}"
                                    }
                                    td { class: "text-right",
                                        button {
                                            class: "btn btn-ghost btn-xs",
                                            disabled: t.is_split,
                                            onclick: move |_| edit_target.set(Some(t_edit.clone())),
                                            "Edit"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs text-error",
                                            onclick: move |_| {
                                                let id = t_id.clone();
                                                spawn(async move {
                                                    match use_finance::delete_transaction(&id).await {
                                                        Ok(_) => refresh += 1,
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        })}
                    }
                }
            }
            div { class: "flex justify-between items-center mt-3 text-sm opacity-70",
                span { "{transactions().len()} of {total()}" }
                div { class: "join",
                    button {
                        class: "join-item btn btn-sm",
                        disabled: skip() == 0,
                        onclick: move |_| skip.set(skip().saturating_sub(page_size)),
                        "Prev"
                    }
                    button {
                        class: "join-item btn btn-sm",
                        disabled: (skip() + page_size) as u64 >= total(),
                        onclick: move |_| skip.set(skip() + page_size),
                        "Next"
                    }
                }
            }
        }

        if show_create() {
            TransactionFormModal {
                initial: None,
                accounts: accounts_for_select.clone(),
                categories: categories(),
                on_close: move |_| show_create.set(false),
                on_saved: move |_| { show_create.set(false); refresh += 1; },
            }
        }
        if let Some(t) = edit_target() {
            TransactionFormModal {
                key: "{t.id}",
                initial: Some(t.clone()),
                accounts: accounts_for_select.clone(),
                categories: categories(),
                on_close: move |_| edit_target.set(None),
                on_saved: move |_| { edit_target.set(None); refresh += 1; },
            }
        }
        if show_import() {
            ImportCsvModal {
                accounts: accounts_for_select.clone(),
                on_close: move |_| show_import.set(false),
                on_imported: move |_| { show_import.set(false); refresh += 1; },
            }
        }
    }
}

#[component]
fn TransactionFormModal(
    initial: Option<TransactionResponse>,
    accounts: Vec<AccountResponse>,
    categories: Vec<FinanceCategoryResponse>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let editing = initial.is_some();
    let mut date = use_signal(|| {
        initial.as_ref()
            .map(|t| t.date.split('T').next().unwrap_or(&t.date).to_string())
            .unwrap_or_else(today_iso)
    });
    let mut account_id = use_signal(|| {
        initial.as_ref().map(|t| t.account_id.clone())
            .or_else(|| accounts.first().map(|a| a.id.clone()))
            .unwrap_or_default()
    });
    let mut amount = use_signal(|| {
        initial.as_ref().map(|t| format_amount_input(t.amount_minor)).unwrap_or_else(|| "0.00".into())
    });
    let mut description = use_signal(|| initial.as_ref().map(|t| t.description.clone()).unwrap_or_default());
    let mut category_id = use_signal(|| initial.as_ref().and_then(|t| t.category_id.clone()).unwrap_or_default());
    let mut notes = use_signal(|| initial.as_ref().and_then(|t| t.notes.clone()).unwrap_or_default());
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut saving = use_signal(|| false);
    let initial_id = initial.as_ref().map(|t| t.id.clone());
    let account_locked = editing;

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-md",
                h3 { class: "font-bold text-lg mb-3",
                    if editing { "Edit transaction" } else { "Add transaction" }
                }
                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3 text-sm", "{e}" }
                }
                div { class: "form-control",
                    label { class: "label", span { class: "label-text", "Date" } }
                    input {
                        r#type: "date",
                        class: "input input-bordered",
                        value: "{date}",
                        oninput: move |e| date.set(e.value()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Account" } }
                    select {
                        class: "select select-bordered",
                        value: "{account_id}",
                        disabled: account_locked,
                        onchange: move |e| account_id.set(e.value()),
                        {accounts.iter().map(|a| rsx! {
                            option { key: "{a.id}", value: "{a.id}", "{a.name} ({a.currency})" }
                        })}
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label",
                        span { class: "label-text", "Amount" }
                        span { class: "label-text-alt opacity-60", "negative = debit / outflow" }
                    }
                    input {
                        class: "input input-bordered",
                        value: "{amount}",
                        oninput: move |e| amount.set(e.value()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Description" } }
                    input {
                        class: "input input-bordered",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Category" } }
                    select {
                        class: "select select-bordered",
                        value: "{category_id}",
                        onchange: move |e| category_id.set(e.value()),
                        option { value: "", "(uncategorized)" }
                        {categories.iter().map(|c| rsx! {
                            option { key: "{c.id}", value: "{c.id}", "{c.name}" }
                        })}
                    }
                }
                div { class: "form-control mt-2",
                    label { class: "label", span { class: "label-text", "Notes" } }
                    textarea {
                        class: "textarea textarea-bordered",
                        rows: 2,
                        value: "{notes}",
                        oninput: move |e| notes.set(e.value()),
                    }
                }
                div { class: "modal-action",
                    button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    button {
                        class: "btn btn-primary",
                        disabled: saving(),
                        onclick: move |_| {
                            let id_opt = initial_id.clone();
                            spawn(async move {
                                error.set(None);
                                saving.set(true);
                                let amount_minor = match parse_money(&amount()) {
                                    Ok(v) => v,
                                    Err(e) => { error.set(Some(e)); saving.set(false); return; }
                                };
                                let cat = category_id();
                                let cat_opt = if cat.is_empty() { None } else { Some(cat) };
                                let notes_val = notes();
                                let notes_opt = if notes_val.trim().is_empty() { None } else { Some(notes_val) };
                                let result = if let Some(id) = id_opt {
                                    let req = UpdateTransactionRequest {
                                        date: Some(date()),
                                        amount_minor: Some(amount_minor),
                                        description: Some(description()),
                                        category_id: Some(cat_opt),
                                        notes: Some(notes_opt),
                                    };
                                    use_finance::update_transaction(&id, &req).await.map(|_| ())
                                } else {
                                    let req = CreateTransactionRequest {
                                        account_id: account_id(),
                                        date: date(),
                                        amount_minor,
                                        description: description(),
                                        category_id: cat_opt,
                                        notes: notes_opt,
                                    };
                                    use_finance::create_transaction(&req).await.map(|_| ())
                                };
                                match result {
                                    Ok(_) => on_saved.call(()),
                                    Err(e) => { error.set(Some(e)); saving.set(false); }
                                }
                            });
                        },
                        if saving() { "Saving…" } else { "Save" }
                    }
                }
            }
        }
    }
}

#[component]
fn ImportCsvModal(
    accounts: Vec<AccountResponse>,
    on_close: EventHandler<()>,
    on_imported: EventHandler<()>,
) -> Element {
    let mut profiles: Signal<Vec<ImportProfileInfo>> = use_signal(Vec::new);
    let mut account_id = use_signal(|| accounts.first().map(|a| a.id.clone()).unwrap_or_default());
    let mut profile_id = use_signal(String::new);
    let mut file_name: Signal<Option<String>> = use_signal(|| None);
    let mut file_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
    let mut submitting = use_signal(|| false);
    let mut result: Signal<Option<ImportCsvResponse>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        spawn(async move {
            match use_finance::list_import_profiles().await {
                Ok(p) => {
                    if let Some(first) = p.first() {
                        if profile_id.peek().is_empty() {
                            profile_id.set(first.id.clone());
                        }
                    }
                    profiles.set(p);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let on_file_change = move |evt: Event<FormData>| {
        let files = evt.files();
        let Some(file) = files.into_iter().next() else {
            return;
        };
        file_name.set(Some(file.name()));
        spawn(async move {
            match file.read_bytes().await {
                Ok(bytes) => file_bytes.set(Some(bytes.to_vec())),
                Err(e) => error.set(Some(format!("Failed to read file: {e}"))),
            }
        });
    };

    let accounts_clone = accounts.clone();
    let submit = move |_| {
        if submitting() {
            return;
        }
        let acc = account_id();
        let prof = profile_id();
        let Some(bytes) = file_bytes() else {
            error.set(Some("Choose a CSV file first".into()));
            return;
        };
        let name = file_name().unwrap_or_else(|| "import.csv".to_string());
        if acc.is_empty() || prof.is_empty() {
            error.set(Some("Select an account and a profile".into()));
            return;
        }
        submitting.set(true);
        error.set(None);
        spawn(async move {
            match use_finance::import_csv(&acc, &prof, &name, bytes).await {
                Ok(resp) => result.set(Some(resp)),
                Err(e) => error.set(Some(e)),
            }
            submitting.set(false);
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-lg",
                h3 { class: "font-bold text-lg mb-3", "Import transactions from CSV" }

                if let Some(r) = result() {
                    div { class: "alert alert-success mb-3",
                        div {
                            div { class: "font-medium", "Import complete" }
                            div { class: "text-sm opacity-80",
                                "Imported: {r.imported} · Skipped (duplicates): {r.skipped} · Errors: {r.errors}"
                            }
                        }
                    }
                    if !r.error_details.is_empty() {
                        div { class: "mb-3",
                            div { class: "font-medium text-sm mb-1", "Row errors (first {r.error_details.len()}):" }
                            ul { class: "text-xs opacity-80 max-h-32 overflow-y-auto",
                                {r.error_details.iter().map(|e| rsx! {
                                    li { "line {e.line}: {e.message}" }
                                })}
                            }
                        }
                    }
                    div { class: "modal-action",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| on_imported.call(()),
                            "Done"
                        }
                    }
                } else {
                    if let Some(e) = error() {
                        div { class: "alert alert-error mb-3", "{e}" }
                    }
                    div { class: "form-control mb-3",
                        label { class: "label", span { class: "label-text", "Account" } }
                        select {
                            class: "select select-bordered",
                            value: "{account_id}",
                            onchange: move |e| account_id.set(e.value()),
                            {accounts_clone.iter().map(|a| rsx! {
                                option { key: "{a.id}", value: "{a.id}", "{a.name} ({a.currency})" }
                            })}
                        }
                    }
                    div { class: "form-control mb-3",
                        label { class: "label", span { class: "label-text", "Format" } }
                        select {
                            class: "select select-bordered",
                            value: "{profile_id}",
                            disabled: profiles().is_empty(),
                            onchange: move |e| profile_id.set(e.value()),
                            {profiles().iter().map(|p| rsx! {
                                option { key: "{p.id}", value: "{p.id}", "{p.name}" }
                            })}
                        }
                    }
                    div { class: "form-control mb-3",
                        label { class: "label", span { class: "label-text", "CSV file" } }
                        input {
                            r#type: "file",
                            class: "file-input file-input-bordered",
                            accept: ".csv,text/csv",
                            onchange: on_file_change,
                        }
                        if let Some(n) = file_name() {
                            label { class: "label",
                                span { class: "label-text-alt opacity-70", "Selected: {n}" }
                            }
                        }
                    }
                    div { class: "modal-action",
                        button {
                            class: "btn",
                            onclick: move |_| on_close.call(()),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: submitting() || file_bytes().is_none(),
                            onclick: submit,
                            if submitting() { "Importing…" } else { "Import" }
                        }
                    }
                }
            }
        }
    }
}
