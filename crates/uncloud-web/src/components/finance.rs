//! Finance tracker — foundation UI (v0).
//!
//! Single page with three internal tabs: Transactions, Accounts, Categories.
//! Deliberately primitive: DaisyUI tables + modal forms. Per-currency
//! totals shown on Accounts. CSV import wired through the Transactions tab.

use std::collections::HashMap;

use dioxus::prelude::*;
use uncloud_common::{
    AccountResponse, BalanceSnapshotResponse, CategorySummaryResponse, CreateAccountRequest,
    CreateFinanceCategoryRequest, CreateTransactionRequest, FinanceCategoryResponse,
    FinanceRuleRequest, FinanceRuleResponse, ImportCsvResponse, ImportRunResponse,
    ImportSchemaResponse, ReconcilePreviewResponse, ReconcileRequest, TestRuleMatch,
    TestRuleRequest, TestRuleResponse, TransactionResponse, UpdateAccountRequest,
    UpdateFinanceCategoryRequest, UpdateTransactionRequest,
};

use crate::components::icons::{IconGripVertical, IconMoreVertical, IconPlus};
use crate::components::scroll_sentinel::ScrollSentinel;
use crate::hooks::use_finance;

fn finance_shell(body: Element) -> Element {
    rsx! {
        div { class: "p-4 lg:p-6 max-w-6xl mx-auto",
            {body}
        }
    }
}

#[component]
pub fn FinanceTransactionsPage() -> Element {
    // Transactions span the full available width — the table benefits
    // from horizontal real estate; the other finance views keep the
    // narrower max-w-6xl from `finance_shell`.
    rsx! {
        div { class: "w-full min-w-0",
            TransactionsTab {}
        }
    }
}

#[component]
pub fn FinanceAccountsPage() -> Element {
    finance_shell(rsx! { AccountsTab {} })
}

#[component]
pub fn FinanceCategoriesPage() -> Element {
    finance_shell(rsx! { CategoriesTab {} })
}

#[component]
pub fn FinanceSchemasPage() -> Element {
    finance_shell(rsx! { SchemasTab {} })
}

#[component]
pub fn FinanceImportsPage() -> Element {
    finance_shell(rsx! { ImportsTab {} })
}

#[component]
pub fn FinanceRulesPage() -> Element {
    finance_shell(rsx! { RulesTab {} })
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

fn ymd(y: i32, m_zero_based: u32, d: u32) -> String {
    format!("{:04}-{:02}-{:02}", y, m_zero_based + 1, d)
}

fn last_day_of_month(year: i32, m_zero_based: u32) -> u32 {
    match m_zero_based {
        0 | 2 | 4 | 6 | 7 | 9 | 11 => 31,
        3 | 5 | 8 | 10 => 30,
        1 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
        _ => 30,
    }
}

/// Returns (from, to) ISO YYYY-MM-DD strings for the named preset.
/// `""` means "no bound on that side" (used for "all time").
fn date_range_for_preset(preset: &str) -> (String, String) {
    let d = js_sys::Date::new_0();
    let y = d.get_full_year() as i32;
    let m = d.get_month();
    let day = d.get_date();
    match preset {
        "this_month" => (ymd(y, m, 1), ymd(y, m, last_day_of_month(y, m))),
        "last_month" => {
            let (ly, lm) = if m == 0 { (y - 1, 11) } else { (y, m - 1) };
            (ymd(ly, lm, 1), ymd(ly, lm, last_day_of_month(ly, lm)))
        }
        "this_year" => (ymd(y, 0, 1), ymd(y, 11, 31)),
        "last_year" => (ymd(y - 1, 0, 1), ymd(y - 1, 11, 31)),
        "last_30_days" => {
            let now_ms = js_sys::Date::now();
            let past = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
                now_ms - 30.0 * 86_400_000.0,
            ));
            let pf = past.to_iso_string().as_string().unwrap_or_default();
            let from = pf.split('T').next().unwrap_or("").to_string();
            (from, ymd(y, m, day))
        }
        _ => (String::new(), String::new()),
    }
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
    let mut reconcile_target: Signal<Option<AccountResponse>> = use_signal(|| None);

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
                table { class: "table table-sm w-full",
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
                            let a_recon = a.clone();
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
                                    td { class: "text-right whitespace-nowrap",
                                        if !archived {
                                            button {
                                                class: "btn btn-ghost btn-xs",
                                                onclick: move |_| reconcile_target.set(Some(a_recon.clone())),
                                                "Reconcile"
                                            }
                                        }
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
        if let Some(a) = reconcile_target() {
            ReconcileModal {
                key: "{a.id}",
                account: a.clone(),
                on_close: move |_| reconcile_target.set(None),
                on_done: move |_| { reconcile_target.set(None); refresh += 1; },
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
    let mut iban = use_signal(|| {
        initial.as_ref().and_then(|a| a.iban.clone()).unwrap_or_default()
    });
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
                div { class: "form-control mt-2",
                    label { class: "label",
                        span { class: "label-text", "IBAN" }
                        span { class: "label-text-alt opacity-60", "optional, drives CSV auto-match" }
                    }
                    input {
                        class: "input input-bordered uppercase",
                        value: "{iban}",
                        placeholder: "DE12 …",
                        oninput: move |e| iban.set(e.value().to_uppercase()),
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
                                let iban_clean = iban().trim().to_string();
                                let iban_opt = if iban_clean.is_empty() { None } else { Some(iban_clean) };
                                let result = if let Some(id) = id_opt {
                                    let req = UpdateAccountRequest {
                                        name: Some(name()),
                                        account_type: Some(account_type()),
                                        opening_balance_minor: Some(opening_minor),
                                        archived: Some(archived()),
                                        iban: Some(iban_opt),
                                    };
                                    use_finance::update_account(&id, &req).await.map(|_| ())
                                } else {
                                    let req = CreateAccountRequest {
                                        name: name(),
                                        account_type: account_type(),
                                        currency: currency(),
                                        opening_balance_minor: opening_minor,
                                        iban: iban_opt,
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

const UNCATEGORIZED_FILTER: &str = "__uncategorized__";

#[component]
fn TransactionActions(
    transaction: TransactionResponse,
    edit_target: Signal<Option<TransactionResponse>>,
    rule_from_tx: Signal<Option<TransactionResponse>>,
    refresh: Signal<u32>,
    error: Signal<Option<String>>,
) -> Element {
    let mut edit_target = edit_target;
    let mut rule_from_tx = rule_from_tx;
    let mut refresh = refresh;
    let mut error = error;
    let t_edit = transaction.clone();
    let t_rule = transaction.clone();
    let t_id = transaction.id.clone();

    rsx! {
        div { class: "dropdown dropdown-end",
            div {
                tabindex: "0",
                role: "button",
                class: "btn btn-ghost btn-xs btn-circle",
                IconMoreVertical {}
            }
            ul {
                tabindex: "0",
                class: "menu dropdown-content bg-base-100 rounded-box shadow z-10 w-36 p-1",
                if !transaction.is_split {
                    li {
                        a {
                            onclick: move |_| edit_target.set(Some(t_edit.clone())),
                            "Edit"
                        }
                    }
                }
                li {
                    a {
                        onclick: move |_| rule_from_tx.set(Some(t_rule.clone())),
                        "Create rule..."
                    }
                }
                li {
                    a {
                        class: "text-error",
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
    }
}

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
    let mut edit_target: Signal<Option<TransactionResponse>> = use_signal(|| None);
    let mut rule_from_tx: Signal<Option<TransactionResponse>> = use_signal(|| None);
    let mut filter_account: Signal<String> = use_signal(String::new);
    let mut category_filter: Signal<Option<String>> = use_signal(|| None);
    let mut only_uncat = use_signal(|| false);
    let mut date_from: Signal<String> = use_signal(String::new);
    let mut date_to: Signal<String> = use_signal(String::new);
    let mut include_recon = use_signal(|| false);
    let mut split_view = use_signal(|| false);
    let breakdown_expanded = use_signal(|| false);
    let mut balances: Signal<HashMap<String, i64>> = use_signal(HashMap::new);
    let mut summary: Signal<Option<CategorySummaryResponse>> = use_signal(|| None);
    let mut loading_more = use_signal(|| false);
    let page_size: u32 = 50;

    // First-page fetch — runs on mount, when filters change, and when
    // the refresh nonce is bumped (after edit/delete/import/etc.).
    // Always replaces `transactions`.
    use_effect(move || {
        let _ = refresh();
        let _ = filter_account();
        let _ = category_filter();
        let _ = only_uncat();
        let _ = date_from();
        let _ = date_to();
        let _ = include_recon();
        spawn(async move {
            loading.set(true);
            // Always refresh — CSV import can auto-create a new account
            // whose id then needs to resolve to a name in the table.
            if let Ok(a) = use_finance::list_accounts().await {
                accounts.set(a);
            }
            // Refetch every refresh — the rule modal may have minted a
            // fresh category whose id then shows up in the transaction
            // list before this map knows the name (renders as "?").
            if let Ok(c) = use_finance::list_categories().await {
                categories.set(c);
            }
            let acc_filter = filter_account();
            let acc_opt = if acc_filter.is_empty() { None } else { Some(acc_filter.as_str()) };
            let cat_filter = category_filter();
            let cat_opt = cat_filter
                .as_deref()
                .filter(|c| *c != UNCATEGORIZED_FILTER);
            let uncat_filter = only_uncat()
                || cat_filter.as_deref() == Some(UNCATEGORIZED_FILTER);
            let from_val = date_from();
            let from_opt = if from_val.is_empty() { None } else { Some(from_val.as_str()) };
            let to_val = date_to();
            let to_opt = if to_val.is_empty() { None } else { Some(to_val.as_str()) };
            match use_finance::list_transactions(
                acc_opt,
                cat_opt,
                uncat_filter,
                from_opt,
                to_opt,
                include_recon(),
                page_size,
                0,
            ).await {
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

    // Refetch summary panel data (per-account balances + per-category
    // aggregation) whenever filters change or the refresh nonce bumps.
    // Reads the same Signals as the transactions fetch above so Dioxus
    // subscribes the effect to each filter.
    use_effect(move || {
        let _ = refresh();
        let _ = filter_account();
        let _ = only_uncat();
        let _ = date_from();
        let _ = date_to();
        let _ = include_recon();
        spawn(async move {
            // Per-account balances — one call per visible account.
            if let Ok(list) = use_finance::list_accounts().await {
                let mut map: HashMap<String, i64> = HashMap::new();
                for a in &list {
                    if let Ok(b) = use_finance::account_balance(&a.id).await {
                        map.insert(a.id.clone(), b.balance_minor);
                    }
                }
                balances.set(map);
            }

            // Category summary — only meaningful with a date range.
            let from_val = date_from.peek().clone();
            let to_val = date_to.peek().clone();
            if from_val.is_empty() && to_val.is_empty() {
                summary.set(None);
                return;
            }
            let acc = filter_account.peek().clone();
            let only_un = *only_uncat.peek();
            let inc_rec = *include_recon.peek();
            let acc_opt = if acc.is_empty() { None } else { Some(acc.as_str()) };
            let from_opt = if from_val.is_empty() { None } else { Some(from_val.as_str()) };
            let to_opt = if to_val.is_empty() { None } else { Some(to_val.as_str()) };
            match use_finance::transaction_category_summary(
                acc_opt, only_un, from_opt, to_opt, inc_rec,
            ).await {
                Ok(s) => summary.set(Some(s)),
                Err(_) => summary.set(None),
            }
        });
    });

    // No-arg `Fn` trigger, called from both the visible Load-more
    // button and the IntersectionObserver sentinel. Reads via `.peek()`
    // and moves mutations into the spawned future so the closure stays
    // Fn (and thus Copy, since it only captures Signals + u32).
    let trigger_load_more = move || {
        if *loading_more.peek() { return; }
        let skip_val = transactions.peek().len() as u32;
        let acc = filter_account.peek().clone();
        let cat = category_filter.peek().clone();
        let only_un = *only_uncat.peek();
        let from_val = date_from.peek().clone();
        let to_val = date_to.peek().clone();
        let inc_recon = *include_recon.peek();
        spawn(async move {
            loading_more.set(true);
            let acc_opt = if acc.is_empty() { None } else { Some(acc.as_str()) };
            let cat_opt = cat
                .as_deref()
                .filter(|c| *c != UNCATEGORIZED_FILTER);
            let uncat_filter = only_un || cat.as_deref() == Some(UNCATEGORIZED_FILTER);
            let from_opt = if from_val.is_empty() { None } else { Some(from_val.as_str()) };
            let to_opt = if to_val.is_empty() { None } else { Some(to_val.as_str()) };
            match use_finance::list_transactions(
                acc_opt,
                cat_opt,
                uncat_filter,
                from_opt,
                to_opt,
                inc_recon,
                page_size,
                skip_val,
            ).await {
                Ok(resp) => {
                    transactions.write().extend(resp.items);
                    total.set(resp.total);
                }
                Err(e) => error.set(Some(e)),
            }
            loading_more.set(false);
        });
    };

    let accounts_for_select = accounts();
    let no_accounts = accounts_for_select.is_empty();
    let selected_category_label = category_filter().map(|id| {
        if id == UNCATEGORIZED_FILTER {
            "Uncategorized".to_string()
        } else {
            categories()
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Selected category".into())
        }
    });

    let render_row = move |t: &TransactionResponse| -> Element {
        let date_short = t.date.split('T').next().unwrap_or(&t.date).to_string();
        let account = accounts()
            .iter()
            .find(|a| a.id == t.account_id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "?".into());
        let cat = t.category_id.as_deref().map(|id| {
            categories()
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "?".into())
        });
        rsx! {
            tr { key: "{t.id}",
                td { class: "whitespace-nowrap font-mono text-sm", "{date_short}" }
                td { class: "min-w-0", "{account}" }
                td { class: "min-w-0 break-words",
                    "{t.description}"
                    if t.is_split { span { class: "badge badge-ghost ml-2 text-xs", "split" } }
                    if t.source_snapshot_id.is_some() {
                        span { class: "badge badge-warning ml-2 text-xs", "reconciliation" }
                    }
                }
                td { class: "opacity-80 truncate",
                    if let Some(name) = cat { "{name}" } else { span { class: "opacity-50 italic", "—" } }
                }
                td {
                    class: if t.amount_minor < 0 { "text-right font-mono text-error" } else { "text-right font-mono text-success" },
                    "{format_money(t.amount_minor, &t.currency)}"
                }
                td { class: "text-right",
                    TransactionActions {
                        transaction: t.clone(),
                        edit_target,
                        rule_from_tx,
                        refresh,
                        error,
                    }
                }
            }
        }
    };

    let render_split_row = move |t: &TransactionResponse| -> Element {
        let date_short = t.date.split('T').next().unwrap_or(&t.date).to_string();
        let cat = t.category_id.as_deref().map(|id| {
            categories()
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "?".into())
        });
        rsx! {
            tr { key: "{t.id}",
                td { class: "whitespace-nowrap font-mono text-sm", "{date_short}" }
                td { class: "min-w-0 break-words",
                    "{t.description}"
                    if t.is_split { span { class: "badge badge-ghost ml-2 text-xs", "split" } }
                    if t.source_snapshot_id.is_some() {
                        span { class: "badge badge-warning ml-2 text-xs", "reconciliation" }
                    }
                }
                td { class: "opacity-80 truncate",
                    if let Some(name) = cat { "{name}" } else { span { class: "opacity-50 italic", "—" } }
                }
                td {
                    class: if t.amount_minor < 0 { "text-right font-mono text-error" } else { "text-right font-mono text-success" },
                    "{format_money(t.amount_minor, &t.currency)}"
                }
                td { class: "text-right",
                    TransactionActions {
                        transaction: t.clone(),
                        edit_target,
                        rule_from_tx,
                        refresh,
                        error,
                    }
                }
            }
        }
    };

    let render_mobile_item = move |t: &TransactionResponse| -> Element {
        let date_short = t.date.split('T').next().unwrap_or(&t.date).to_string();
        let account = accounts()
            .iter()
            .find(|a| a.id == t.account_id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "?".into());
        let cat = t.category_id.as_deref().map(|id| {
            categories()
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "?".into())
        });
        let amount_class = if t.amount_minor < 0 {
            "font-mono text-sm font-semibold text-error text-right break-words"
        } else {
            "font-mono text-sm font-semibold text-success text-right break-words"
        };
        rsx! {
            div {
                key: "{t.id}",
                class: "rounded-lg border border-base-300 bg-base-100 px-3 py-3 shadow-sm min-w-0",
                div { class: "grid grid-cols-[minmax(0,1fr)_auto] gap-2 items-start",
                    div { class: "min-w-0",
                        div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-base-content/60",
                            span { class: "font-mono", "{date_short}" }
                            span { class: "truncate max-w-full", "{account}" }
                        }
                        div { class: "mt-1 font-medium leading-snug break-words",
                            "{t.description}"
                        }
                    }
                    div { class: "flex items-start justify-end gap-1 min-w-0 max-w-[10rem]",
                        div { class: "{amount_class}", "{format_money(t.amount_minor, &t.currency)}" }
                        TransactionActions {
                            transaction: t.clone(),
                            edit_target,
                            rule_from_tx,
                            refresh,
                            error,
                        }
                    }
                }
                div { class: "mt-2 flex flex-wrap items-center gap-1 text-xs",
                    if let Some(name) = cat {
                        span { class: "badge badge-outline badge-sm max-w-full truncate", "{name}" }
                    } else {
                        span { class: "badge badge-ghost badge-sm opacity-70", "Uncategorized" }
                    }
                    if t.is_split { span { class: "badge badge-ghost badge-sm", "split" } }
                    if t.source_snapshot_id.is_some() {
                        span { class: "badge badge-warning badge-sm", "reconciliation" }
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "mb-4 space-y-3",
            div { class: "flex items-center justify-between gap-3",
                h2 { class: "text-xl font-semibold", "Transactions" }
                button {
                    class: "btn btn-primary btn-sm shrink-0",
                    disabled: no_accounts,
                    onclick: move |_| show_create.set(true),
                    IconPlus {}
                    span { "Add transaction" }
                }
            }
            div { class: "rounded-lg border border-base-300 bg-base-100 p-3 shadow-sm",
                div { class: "grid grid-cols-2 gap-2 md:grid-cols-4 xl:grid-cols-8 xl:items-end",
                    label { class: "form-control col-span-2 xl:col-span-2 min-w-0",
                        span { class: "block text-xs opacity-70 pb-1", "Account" }
                        select {
                            class: "select select-bordered select-sm w-full min-w-0",
                            value: "{filter_account}",
                            onchange: move |e| filter_account.set(e.value()),
                            option { value: "", "All accounts" }
                            {accounts().iter().map(|a| rsx! {
                                option { key: "{a.id}", value: "{a.id}", "{a.name}" }
                            })}
                        }
                    }
                    label { class: "form-control col-span-2 md:col-span-1 xl:col-span-1 min-w-0",
                        span { class: "block text-xs opacity-70 pb-1", "Range" }
                        select {
                            class: "select select-bordered select-sm w-full min-w-0",
                            value: "",
                            onchange: move |e| {
                                let (f, t) = date_range_for_preset(&e.value());
                                date_from.set(f);
                                date_to.set(t);
                            },
                            option { value: "", "Date range..." }
                            option { value: "this_month", "This month" }
                            option { value: "last_month", "Last month" }
                            option { value: "last_30_days", "Last 30 days" }
                            option { value: "this_year", "This year" }
                            option { value: "last_year", "Last year" }
                            option { value: "all_time", "All time" }
                        }
                    }
                    label { class: "form-control min-w-0",
                        span { class: "block text-xs opacity-70 pb-1", "From" }
                        input {
                            r#type: "date",
                            class: "input input-bordered input-sm w-full min-w-0",
                            value: "{date_from}",
                            oninput: move |e| date_from.set(e.value()),
                        }
                    }
                    label { class: "form-control min-w-0",
                        span { class: "block text-xs opacity-70 pb-1", "To" }
                        input {
                            r#type: "date",
                            class: "input input-bordered input-sm w-full min-w-0",
                            value: "{date_to}",
                            oninput: move |e| date_to.set(e.value()),
                        }
                    }
                    div { class: "col-span-2 md:col-span-4 xl:col-span-3 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2 xl:pt-6",
                        label { class: "flex h-8 items-center gap-2 rounded border border-base-300 bg-base-200/40 px-3 text-sm cursor-pointer min-w-0",
                            input {
                                r#type: "checkbox",
                                class: "checkbox checkbox-sm",
                                checked: only_uncat(),
                                onchange: move |e| {
                                    let checked = e.checked();
                                    only_uncat.set(checked);
                                    if checked {
                                        category_filter.set(None);
                                    }
                                },
                            }
                            span { class: "truncate", "Uncategorized only" }
                        }
                        label { class: "flex h-8 items-center gap-2 rounded border border-base-300 bg-base-200/40 px-3 text-sm cursor-pointer min-w-0",
                            input {
                                r#type: "checkbox",
                                class: "checkbox checkbox-sm",
                                checked: include_recon(),
                                onchange: move |e| include_recon.set(e.checked()),
                            }
                            span { class: "truncate", "Show reconciliations" }
                        }
                        {
                            let date_set = !date_from().is_empty() || !date_to().is_empty();
                            rsx! {
                                label { class: "hidden lg:flex h-8 items-center gap-2 rounded border border-base-300 bg-base-200/40 px-3 text-sm cursor-pointer min-w-0",
                                    input {
                                        r#type: "checkbox",
                                        class: "checkbox checkbox-sm",
                                        checked: split_view() && date_set,
                                        disabled: !date_set,
                                        onchange: move |e| split_view.set(e.checked()),
                                    }
                                    span {
                                        class: if date_set { "truncate" } else { "truncate opacity-50" },
                                        "Income / Expenses"
                                    }
                                }
                            }
                        }
                    }
                    if let Some(label) = selected_category_label.clone() {
                        div { class: "col-span-2 md:col-span-4 xl:col-span-8 flex flex-wrap items-center gap-2 rounded bg-primary/10 px-3 py-2 text-sm",
                            span { class: "text-base-content/70", "Category" }
                            span { class: "font-medium", "{label}" }
                            button {
                                r#type: "button",
                                class: "btn btn-ghost btn-xs",
                                onclick: move |_| category_filter.set(None),
                                "Clear"
                            }
                        }
                    }
                }
            }
        }
        if no_accounts {
            div { class: "alert alert-info mb-3",
                "No accounts yet — add one from the Accounts page, or import a CSV with an IBAN-aware schema to auto-create one."
            }
        }
        if !no_accounts {
            SummaryStrip {
                accounts: accounts_for_select.clone(),
                categories: categories(),
                balances: balances(),
                summary: summary(),
                account_filter: filter_account(),
                has_date_range: !date_from().is_empty() || !date_to().is_empty(),
                expanded: breakdown_expanded,
                selected_category_filter: category_filter(),
                on_category_filter: move |selected| {
                    category_filter.set(selected);
                    only_uncat.set(false);
                },
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
            {
                let date_set = !date_from().is_empty() || !date_to().is_empty();
                let show_split = split_view() && date_set;
                let txs = transactions();
                let (income_total, expense_total) =
                    txs.iter().fold((0i64, 0i64), |(i, e), t| {
                        if t.amount_minor >= 0 { (i + t.amount_minor, e) } else { (i, e + t.amount_minor) }
                    });
                let currency = txs.first().map(|t| t.currency.clone()).unwrap_or_default();
                rsx! {
                    div { class: "lg:hidden space-y-2",
                        {txs.iter().map(|t| {
                            let item = render_mobile_item(t);
                            rsx! { {item} }
                        })}
                    }
                    div { class: "hidden lg:block",
                        if show_split {
                            div { class: "grid grid-cols-2 gap-4",
                                div { class: "min-w-0",
                                    div { class: "flex justify-between items-baseline mb-2 gap-3",
                                        h3 { class: "font-semibold text-success", "Income" }
                                        span { class: "font-mono text-success text-right",
                                            "{format_money(income_total, &currency)}"
                                        }
                                    }
                                    div { class: "rounded-lg border border-base-300",
                                        table { class: "table table-zebra table-sm w-full table-fixed",
                                            thead { tr {
                                                th { class: "w-28", "Date" }
                                                th { "Description" }
                                                th { class: "w-36", "Category" }
                                                th { class: "w-32 text-right", "Amount" }
                                                th { class: "w-12", "" }
                                            } }
                                            tbody {
                                                {txs.iter().filter(|t| t.amount_minor >= 0).map(|t| {
                                                    let row = render_split_row(t);
                                                    rsx! { {row} }
                                                })}
                                            }
                                        }
                                    }
                                }
                                div { class: "min-w-0",
                                    div { class: "flex justify-between items-baseline mb-2 gap-3",
                                        h3 { class: "font-semibold text-error", "Expenses" }
                                        span { class: "font-mono text-error text-right",
                                            "{format_money(expense_total, &currency)}"
                                        }
                                    }
                                    div { class: "rounded-lg border border-base-300",
                                        table { class: "table table-zebra table-sm w-full table-fixed",
                                            thead { tr {
                                                th { class: "w-28", "Date" }
                                                th { "Description" }
                                                th { class: "w-36", "Category" }
                                                th { class: "w-32 text-right", "Amount" }
                                                th { class: "w-12", "" }
                                            } }
                                            tbody {
                                                {txs.iter().filter(|t| t.amount_minor < 0).map(|t| {
                                                    let row = render_split_row(t);
                                                    rsx! { {row} }
                                                })}
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            div { class: "rounded-lg border border-base-300",
                                table { class: "table table-zebra table-sm w-full table-fixed",
                                    thead {
                                        tr {
                                            th { class: "w-32", "Date" }
                                            th { class: "w-44", "Account" }
                                            th { "Description" }
                                            th { class: "w-48", "Category" }
                                            th { class: "w-40 text-right", "Amount" }
                                            th { class: "w-12", "" }
                                        }
                                    }
                                    tbody {
                                        {txs.iter().map(|t| {
                                            let row = render_row(t);
                                            rsx! { {row} }
                                        })}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "flex justify-center items-center gap-3 mt-3 text-sm opacity-70",
                span { "{transactions().len()} of {total()}" }
            }
            // Auto-load-more sentinel — fires when the bottom of the
            // list (plus a 400 px lead-in) scrolls into view. Manual
            // "Load more" button is kept as a visible affordance and
            // for browsers where IntersectionObserver glitches.
            if (transactions().len() as u64) < total() && !loading() {
                ScrollSentinel { on_visible: move |_| trigger_load_more() }
                div { class: "flex justify-center mt-2",
                    button {
                        class: "btn btn-sm btn-ghost",
                        disabled: loading_more(),
                        onclick: move |_| trigger_load_more(),
                        if loading_more() { "Loading…" } else { "Load more" }
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
        if let Some(t) = rule_from_tx() {
            {
                let cat_map: HashMap<String, String> = categories()
                    .iter()
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                let prefill = RulePrefill {
                    pattern: t.description.clone(),
                    category_id: t.category_id.clone(),
                    name: t.description.clone(),
                };
                rsx! {
                    RuleFormModal {
                        key: "{t.id}",
                        initial: None,
                        prefill: Some(prefill),
                        categories: cat_map,
                        on_close: move |_| rule_from_tx.set(None),
                        on_saved: move |_| { rule_from_tx.set(None); refresh += 1; },
                    }
                }
            }
        }
    }
}

/// Dumb summary panel above the transactions table — the parent is
/// responsible for fetching balances + summary so signal subscriptions
/// fire correctly when filter Signals change.
///
/// Always shows per-account balances (only the selected one when an
/// account filter is active). When a date range is set, additionally
/// shows total income/expense for that range, and a collapsible
/// breakdown of those amounts by category.
#[component]
fn SummaryStrip(
    accounts: Vec<AccountResponse>,
    categories: Vec<FinanceCategoryResponse>,
    balances: HashMap<String, i64>,
    summary: Option<CategorySummaryResponse>,
    account_filter: String,
    has_date_range: bool,
    expanded: Signal<bool>,
    selected_category_filter: Option<String>,
    on_category_filter: EventHandler<Option<String>>,
) -> Element {
    let mut expanded = expanded;
    let category_lookup: HashMap<String, String> = categories
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();

    let visible_accounts: Vec<&AccountResponse> = if account_filter.is_empty() {
        accounts.iter().filter(|a| a.archived_at.is_none()).collect()
    } else {
        accounts.iter().filter(|a| a.id == account_filter).collect()
    };

    // Sorted by absolute total descending so the most-active rows go first.
    let breakdown: Vec<(Option<String>, String, i64)> = summary
        .as_ref()
        .map(|s| {
            let mut v: Vec<(Option<String>, String, i64)> = s
                .items
                .iter()
                .map(|it| {
                    let name = it
                        .category_id
                        .as_ref()
                        .and_then(|id| category_lookup.get(id).cloned())
                        .unwrap_or_else(|| "Uncategorized".into());
                    let total = it.income_minor.saturating_add(it.expense_minor);
                    (it.category_id.clone(), name, total)
                })
                .collect();
            v.sort_by_key(|(_, _, t)| -(t.abs()));
            v
        })
        .unwrap_or_default();
    let breakdown_max_abs = breakdown
        .iter()
        .map(|(_, _, t)| t.abs())
        .max()
        .unwrap_or(1)
        .max(1);

    let income_total = summary.as_ref().map(|s| s.income_total_minor).unwrap_or(0);
    let expense_total = summary.as_ref().map(|s| s.expense_total_minor).unwrap_or(0);
    let net_total = income_total + expense_total;
    // Use the filtered account's currency when available; otherwise the
    // first account's. Mixed-currency totals would need per-currency
    // breakdown, which is out of scope for v1.
    let currency_hint = visible_accounts
        .first()
        .map(|a| a.currency.clone())
        .or_else(|| accounts.first().map(|a| a.currency.clone()))
        .unwrap_or_default();

    rsx! {
        div { class: "mb-4 space-y-2",
            if !visible_accounts.is_empty() {
                div { class: "flex flex-wrap gap-2 items-stretch",
                    {visible_accounts.iter().map(|a| {
                        let bal = balances.get(&a.id).copied().unwrap_or(a.opening_balance_minor);
                        rsx! {
                            div { key: "{a.id}",
                                class: "bg-base-200 rounded-lg px-3 py-2 min-w-0 w-full sm:w-auto sm:min-w-[10rem]",
                                div { class: "text-xs opacity-70 truncate", "{a.name}" }
                                div { class: "font-mono font-semibold",
                                    "{format_money(bal, &a.currency)}"
                                }
                            }
                        }
                    })}
                }
            }

            if has_date_range {
                div { class: "grid grid-cols-1 sm:grid-cols-3 lg:grid-cols-[1fr_1fr_1fr_auto] gap-2 rounded-lg border border-base-300 bg-base-100 p-2 shadow-sm",
                    div { class: "rounded bg-base-200/60 px-3 py-2 min-w-0",
                        div { class: "text-xs opacity-60", "Income" }
                        div { class: "text-lg font-semibold text-success font-mono truncate",
                            "{format_money(income_total, &currency_hint)}"
                        }
                    }
                    div { class: "rounded bg-base-200/60 px-3 py-2 min-w-0",
                        div { class: "text-xs opacity-60", "Expenses" }
                        div { class: "text-lg font-semibold text-error font-mono truncate",
                            "{format_money(expense_total, &currency_hint)}"
                        }
                    }
                    div { class: "rounded bg-base-200/60 px-3 py-2 min-w-0",
                        div { class: "text-xs opacity-60", "Net" }
                        div {
                            class: if net_total >= 0 {
                                "text-lg font-semibold font-mono text-success truncate"
                            } else {
                                "text-lg font-semibold font-mono text-error truncate"
                            },
                            "{format_money(net_total, &currency_hint)}"
                        }
                    }
                    div { class: "flex items-center sm:col-span-3 lg:col-span-1",
                        button {
                            class: "btn btn-ghost btn-sm w-full lg:w-auto",
                            onclick: move |_| expanded.set(!expanded()),
                            if expanded() { "Hide breakdown" } else { "Show breakdown" }
                        }
                    }
                }
            }

            if has_date_range && expanded() {
                div { class: "bg-base-100 rounded-lg border border-base-300 p-3",
                    if breakdown.is_empty() {
                        div { class: "text-sm opacity-60 text-center py-2",
                            "No transactions in the selected range."
                        }
                    } else {
                        ul { class: "space-y-1",
                            {breakdown.iter().map(|(category_id, name, total)| {
                                let filter_value = category_id
                                    .clone()
                                    .unwrap_or_else(|| UNCATEGORIZED_FILTER.to_string());
                                let selected = selected_category_filter
                                    .as_deref()
                                    == Some(filter_value.as_str());
                                let width_pct = (total.abs() as f64 / breakdown_max_abs as f64) * 100.0;
                                let positive = *total >= 0;
                                let bar_class = if positive {
                                    "h-2 rounded-full bg-success"
                                } else {
                                    "h-2 rounded-full bg-error"
                                };
                                let amount_class = if positive {
                                    "text-success font-mono text-sm"
                                } else {
                                    "text-error font-mono text-sm"
                                };
                                let n = name.clone();
                                let t = *total;
                                let cur = currency_hint.clone();
                                let row_class = if selected {
                                    "grid w-full grid-cols-1 sm:grid-cols-[minmax(8rem,12rem)_minmax(0,1fr)_8rem] sm:items-center gap-1 sm:gap-3 rounded bg-primary/10 px-2 py-1 text-left ring-1 ring-primary/30"
                                } else {
                                    "grid w-full grid-cols-1 sm:grid-cols-[minmax(8rem,12rem)_minmax(0,1fr)_8rem] sm:items-center gap-1 sm:gap-3 rounded px-2 py-1 text-left hover:bg-base-200"
                                };
                                rsx! {
                                    li { key: "{filter_value}",
                                        button {
                                            r#type: "button",
                                            class: "{row_class}",
                                            onclick: move |_| {
                                                if selected {
                                                    on_category_filter.call(None);
                                                } else {
                                                    on_category_filter.call(Some(filter_value.clone()));
                                                }
                                            },
                                            span { class: "truncate text-sm", "{n}" }
                                            div { class: "bg-base-200 rounded-full h-2",
                                                div {
                                                    class: "{bar_class}",
                                                    style: "width: {width_pct:.1}%",
                                                }
                                            }
                                            span { class: "{amount_class} text-right",
                                                "{format_money(t, &cur)}"
                                            }
                                        }
                                    }
                                }
                            })}
                        }
                    }
                }
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
    let mut local_cats: Signal<Vec<FinanceCategoryResponse>> = use_signal(|| categories.clone());
    let mut show_new_cat = use_signal(|| false);
    let mut new_cat_name = use_signal(String::new);
    let mut new_cat_busy = use_signal(|| false);
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
                    label { class: "label",
                        span { class: "label-text", "Category" }
                        if !show_new_cat() {
                            button {
                                r#type: "button",
                                class: "label-text-alt link link-primary",
                                onclick: move |_| { new_cat_name.set(String::new()); show_new_cat.set(true); },
                                "+ New category"
                            }
                        }
                    }
                    if show_new_cat() {
                        div { class: "join w-full",
                            input {
                                class: "input input-bordered join-item flex-1",
                                placeholder: "Category name",
                                value: "{new_cat_name}",
                                oninput: move |e| new_cat_name.set(e.value()),
                            }
                            button {
                                r#type: "button",
                                class: "btn join-item",
                                disabled: new_cat_busy() || new_cat_name().trim().is_empty(),
                                onclick: move |_| {
                                    new_cat_busy.set(true);
                                    let name = new_cat_name().trim().to_string();
                                    spawn(async move {
                                        let req = CreateFinanceCategoryRequest {
                                            name,
                                            parent_id: None,
                                            colour: None,
                                        };
                                        match use_finance::create_category(&req).await {
                                            Ok(c) => {
                                                let new_id = c.id.clone();
                                                local_cats.with_mut(|v| v.push(c));
                                                category_id.set(new_id);
                                                show_new_cat.set(false);
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        new_cat_busy.set(false);
                                    });
                                },
                                if new_cat_busy() { "…" } else { "Create" }
                            }
                            button {
                                r#type: "button",
                                class: "btn btn-ghost join-item",
                                onclick: move |_| show_new_cat.set(false),
                                "Cancel"
                            }
                        }
                    } else {
                        select {
                            class: "select select-bordered",
                            value: "{category_id}",
                            onchange: move |e| category_id.set(e.value()),
                            option { value: "", "(uncategorized)" }
                            {local_cats().iter().map(|c| rsx! {
                                option { key: "{c.id}", value: "{c.id}", "{c.name}" }
                            })}
                        }
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
    let mut schemas: Signal<Vec<ImportSchemaResponse>> = use_signal(Vec::new);
    let mut account_id = use_signal(|| accounts.first().map(|a| a.id.clone()).unwrap_or_default());
    let mut schema_id = use_signal(String::new);
    let mut file_name: Signal<Option<String>> = use_signal(|| None);
    let mut file_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
    let mut submitting = use_signal(|| false);
    let mut result: Signal<Option<ImportCsvResponse>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    use_effect(move || {
        spawn(async move {
            match use_finance::list_import_schemas().await {
                Ok(s) => {
                    if let Some(first) = s.first() {
                        if schema_id.peek().is_empty() {
                            schema_id.set(first.id.clone());
                        }
                    }
                    schemas.set(s);
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
        let sch = schema_id();
        let Some(bytes) = file_bytes() else {
            error.set(Some("Choose a CSV file first".into()));
            return;
        };
        let name = file_name().unwrap_or_else(|| "import.csv".to_string());
        if sch.is_empty() {
            error.set(Some("Pick a schema".into()));
            return;
        }
        submitting.set(true);
        error.set(None);
        spawn(async move {
            match use_finance::import_csv(&acc, &sch, &name, bytes).await {
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
                            if let Some(acc) = r.auto_created_account.as_ref() {
                                div { class: "text-sm mt-1",
                                    "Auto-created account "
                                    span { class: "font-medium", "{acc.name}" }
                                    " ({acc.currency})"
                                }
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
                        label { class: "label", span { class: "label-text", "Schema" } }
                        select {
                            class: "select select-bordered",
                            value: "{schema_id}",
                            disabled: schemas().is_empty(),
                            onchange: move |e| schema_id.set(e.value()),
                            {schemas().iter().map(|s| rsx! {
                                option { key: "{s.id}", value: "{s.id}", "{s.name}" }
                            })}
                        }
                    }
                    {
                        let selected = schemas().iter().find(|s| s.id == schema_id()).cloned();
                        let supports_auto = selected.as_ref().map(|s| s.iban_column.is_some()).unwrap_or(false);
                        rsx! {
                            div { class: "form-control mb-3",
                                label { class: "label",
                                    span { class: "label-text", "Account" }
                                    if supports_auto {
                                        span { class: "label-text-alt opacity-60", "optional — auto-matched by IBAN" }
                                    }
                                }
                                select {
                                    class: "select select-bordered",
                                    value: "{account_id}",
                                    onchange: move |e| account_id.set(e.value()),
                                    if supports_auto {
                                        option { value: "", "Auto (match by IBAN, or create)" }
                                    }
                                    {accounts_clone.iter().map(|a| rsx! {
                                        option { key: "{a.id}", value: "{a.id}", "{a.name} ({a.currency})" }
                                    })}
                                }
                            }
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

#[component]
fn SchemasTab() -> Element {
    let mut schemas: Signal<Vec<ImportSchemaResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut show_create = use_signal(|| false);
    let mut edit_target: Signal<Option<ImportSchemaResponse>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match use_finance::list_import_schemas().await {
                Ok(list) => { schemas.set(list); error.set(None); }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "flex justify-between items-center mb-4",
            h2 { class: "text-xl font-semibold", "Import schemas" }
            button {
                class: "btn btn-primary btn-sm",
                onclick: move |_| show_create.set(true),
                "New schema"
            }
        }
        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && schemas().is_empty() {
            div { class: "flex justify-center py-8", span { class: "loading loading-spinner loading-lg" } }
        } else if schemas().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No schemas yet." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead { tr {
                        th { "Name" }
                        th { "Encoding" }
                        th { "Delimiter" }
                        th { "Decimal" }
                        th { "" }
                    } }
                    tbody {
                        {schemas().iter().map(|s| {
                            let s_edit = s.clone();
                            let s_id_clone = s.id.clone();
                            let s_id_delete = s.id.clone();
                            let is_builtin = s.is_builtin;
                            rsx! {
                                tr { key: "{s.id}",
                                    td {
                                        span { "{s.name}" }
                                        if is_builtin {
                                            span { class: "badge badge-ghost badge-sm ml-2", "built-in" }
                                        }
                                    }
                                    td { class: "opacity-60", "{s.encoding}" }
                                    td { class: "opacity-60 font-mono text-xs", "{s.delimiter}" }
                                    td { class: "opacity-60", "{s.decimal_separator}" }
                                    td { class: "text-right whitespace-nowrap",
                                        if !is_builtin {
                                            button {
                                                class: "btn btn-ghost btn-xs",
                                                onclick: move |_| edit_target.set(Some(s_edit.clone())),
                                                "Edit"
                                            }
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs",
                                            onclick: move |_| {
                                                let id = s_id_clone.clone();
                                                spawn(async move {
                                                    match use_finance::clone_import_schema(&id).await {
                                                        Ok(_) => refresh += 1,
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            },
                                            "Clone"
                                        }
                                        if !is_builtin {
                                            button {
                                                class: "btn btn-ghost btn-xs text-error",
                                                onclick: move |_| {
                                                    let id = s_id_delete.clone();
                                                    spawn(async move {
                                                        match use_finance::delete_import_schema(&id).await {
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
                            }
                        })}
                    }
                }
            }
        }
        if show_create() {
            SchemaFormModal {
                initial: None,
                on_close: move |_| show_create.set(false),
                on_saved: move |_| { show_create.set(false); refresh += 1; },
            }
        }
        if let Some(s) = edit_target() {
            SchemaFormModal {
                key: "{s.id}",
                initial: Some(s.clone()),
                on_close: move |_| edit_target.set(None),
                on_saved: move |_| { edit_target.set(None); refresh += 1; },
            }
        }
    }
}

#[component]
fn SchemaFormModal(
    initial: Option<ImportSchemaResponse>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let is_edit = initial.is_some();
    let editing_id = initial.as_ref().map(|s| s.id.clone());

    let mut name = use_signal(|| initial.as_ref().map(|s| s.name.clone()).unwrap_or_default());
    let mut delimiter = use_signal(|| {
        initial.as_ref().map(|s| s.delimiter.clone()).unwrap_or_else(|| ",".into())
    });
    let mut encoding = use_signal(|| {
        initial.as_ref().map(|s| s.encoding.clone()).unwrap_or_else(|| "utf-8".into())
    });
    let mut decimal_separator = use_signal(|| {
        initial.as_ref().map(|s| s.decimal_separator.clone()).unwrap_or_else(|| "dot".into())
    });
    let mut skip_header_rows = use_signal(|| {
        initial.as_ref().map(|s| s.skip_header_rows).unwrap_or(0).to_string()
    });
    let mut has_headers = use_signal(|| {
        initial.as_ref().map(|s| s.has_headers).unwrap_or(true)
    });
    let mut date_column = use_signal(|| {
        initial.as_ref().map(|s| s.date_column).unwrap_or(0).to_string()
    });
    let mut date_format = use_signal(|| {
        initial.as_ref().map(|s| s.date_format.clone()).unwrap_or_else(|| "YYYY-MM-DD".into())
    });
    let mut amount_column = use_signal(|| {
        initial.as_ref().map(|s| s.amount_column).unwrap_or(0).to_string()
    });
    let mut amount_sign_convention = use_signal(|| {
        initial.as_ref().map(|s| s.amount_sign_convention.clone()).unwrap_or_else(|| "positive_credit".into())
    });
    let mut description_columns = use_signal(|| {
        initial.as_ref()
            .map(|s| s.description_columns.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","))
            .unwrap_or_default()
    });
    let mut currency_source = use_signal(|| {
        initial.as_ref().map(|s| s.currency_source.clone()).unwrap_or_else(|| "fixed".into())
    });
    let mut currency_column = use_signal(|| {
        initial.as_ref().and_then(|s| s.currency_column).map(|c| c.to_string()).unwrap_or_default()
    });
    let mut fixed_currency = use_signal(|| {
        initial.as_ref().and_then(|s| s.fixed_currency.clone()).unwrap_or_else(|| "EUR".into())
    });
    let mut bank_ref_column = use_signal(|| {
        initial.as_ref().and_then(|s| s.bank_ref_column).map(|c| c.to_string()).unwrap_or_default()
    });
    let mut iban_column = use_signal(|| {
        initial.as_ref().and_then(|s| s.iban_column).map(|c| c.to_string()).unwrap_or_default()
    });
    let mut raw_category_column = use_signal(|| {
        initial.as_ref().and_then(|s| s.raw_category_column).map(|c| c.to_string()).unwrap_or_default()
    });

    let mut submitting = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let submit = move |_| {
        if submitting() {
            return;
        }
        let parse_u32 = |s: &str| -> Result<u32, String> {
            s.trim().parse::<u32>().map_err(|_| format!("`{s}` must be a non-negative integer"))
        };
        let parse_opt_u32 = |s: &str| -> Result<Option<u32>, String> {
            let trimmed = s.trim();
            if trimmed.is_empty() { Ok(None) } else { parse_u32(trimmed).map(Some) }
        };
        let parse_desc = |s: &str| -> Result<Vec<u32>, String> {
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| p.parse::<u32>().map_err(|_| format!("`{p}` is not a column index")))
                .collect()
        };

        let req = match (|| -> Result<uncloud_common::ImportSchemaRequest, String> {
            Ok(uncloud_common::ImportSchemaRequest {
                name: name(),
                delimiter: delimiter(),
                encoding: encoding(),
                decimal_separator: decimal_separator(),
                skip_header_rows: parse_u32(&skip_header_rows())?,
                has_headers: has_headers(),
                date_column: parse_u32(&date_column())?,
                date_format: date_format(),
                amount_column: parse_u32(&amount_column())?,
                amount_sign_convention: amount_sign_convention(),
                description_columns: parse_desc(&description_columns())?,
                currency_source: currency_source(),
                currency_column: parse_opt_u32(&currency_column())?,
                fixed_currency: if fixed_currency().trim().is_empty() {
                    None
                } else {
                    Some(fixed_currency().trim().to_string())
                },
                bank_ref_column: parse_opt_u32(&bank_ref_column())?,
                iban_column: parse_opt_u32(&iban_column())?,
                raw_category_column: parse_opt_u32(&raw_category_column())?,
            })
        })() {
            Ok(r) => r,
            Err(e) => {
                error.set(Some(e));
                return;
            }
        };

        submitting.set(true);
        error.set(None);
        let editing_id = editing_id.clone();
        spawn(async move {
            let result = match editing_id {
                Some(id) => use_finance::update_import_schema(&id, &req).await,
                None => use_finance::create_import_schema(&req).await,
            };
            match result {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(e)),
            }
            submitting.set(false);
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-2xl",
                h3 { class: "font-bold text-lg mb-3",
                    if is_edit { "Edit schema" } else { "New schema" }
                }
                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3", "{e}" }
                }

                div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                    div { class: "form-control md:col-span-2",
                        label { class: "label", span { class: "label-text", "Name" } }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Encoding" } }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            value: "{encoding}",
                            placeholder: "utf-8, windows-1252, …",
                            oninput: move |e| encoding.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Delimiter" } }
                        input {
                            class: "input input-bordered font-mono",
                            r#type: "text",
                            maxlength: "1",
                            value: "{delimiter}",
                            oninput: move |e| delimiter.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Decimal separator" } }
                        select {
                            class: "select select-bordered",
                            value: "{decimal_separator}",
                            onchange: move |e| decimal_separator.set(e.value()),
                            option { value: "dot", "Dot (1,234.56)" }
                            option { value: "comma", "Comma (1.234,56)" }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text", "Skip header rows" }
                        }
                        input {
                            class: "input input-bordered",
                            r#type: "number",
                            min: "0",
                            value: "{skip_header_rows}",
                            oninput: move |e| skip_header_rows.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label cursor-pointer",
                            span { class: "label-text", "First row is a header" }
                            input {
                                r#type: "checkbox",
                                class: "checkbox",
                                checked: "{has_headers}",
                                oninput: move |e| has_headers.set(e.checked()),
                            }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Date column (0-based)" } }
                        input {
                            class: "input input-bordered",
                            r#type: "number",
                            min: "0",
                            value: "{date_column}",
                            oninput: move |e| date_column.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Date format" } }
                        select {
                            class: "select select-bordered",
                            value: "{date_format}",
                            onchange: move |e| date_format.set(e.value()),
                            option { value: "DD.MM.YY", "DD.MM.YY (German)" }
                            option { value: "DD.MM.YYYY", "DD.MM.YYYY" }
                            option { value: "DD/MM/YYYY", "DD/MM/YYYY" }
                            option { value: "MM/DD/YYYY", "MM/DD/YYYY (US)" }
                            option { value: "YYYY-MM-DD", "YYYY-MM-DD (ISO)" }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Amount column" } }
                        input {
                            class: "input input-bordered",
                            r#type: "number",
                            min: "0",
                            value: "{amount_column}",
                            oninput: move |e| amount_column.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Amount sign convention" } }
                        select {
                            class: "select select-bordered",
                            value: "{amount_sign_convention}",
                            onchange: move |e| amount_sign_convention.set(e.value()),
                            option { value: "positive_credit", "Positive = credit (money in)" }
                            option { value: "positive_debit", "Positive = debit (money out)" }
                        }
                    }
                    div { class: "form-control md:col-span-2",
                        label { class: "label",
                            span { class: "label-text", "Description columns" }
                            span { class: "label-text-alt opacity-60", "comma-separated, joined with \" / \"" }
                        }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            placeholder: "e.g. 11,4",
                            value: "{description_columns}",
                            oninput: move |e| description_columns.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Currency source" } }
                        select {
                            class: "select select-bordered",
                            value: "{currency_source}",
                            onchange: move |e| currency_source.set(e.value()),
                            option { value: "column", "From column" }
                            option { value: "fixed", "Fixed value" }
                        }
                    }
                    if currency_source() == "column" {
                        div { class: "form-control",
                            label { class: "label", span { class: "label-text", "Currency column" } }
                            input {
                                class: "input input-bordered",
                                r#type: "number",
                                min: "0",
                                value: "{currency_column}",
                                oninput: move |e| currency_column.set(e.value()),
                            }
                        }
                    } else {
                        div { class: "form-control",
                            label { class: "label", span { class: "label-text", "Fixed currency" } }
                            input {
                                class: "input input-bordered uppercase",
                                r#type: "text",
                                maxlength: "3",
                                value: "{fixed_currency}",
                                oninput: move |e| fixed_currency.set(e.value()),
                            }
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text", "Bank reference column" }
                            span { class: "label-text-alt opacity-60", "optional" }
                        }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            placeholder: "leave blank to hash whole row",
                            value: "{bank_ref_column}",
                            oninput: move |e| bank_ref_column.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text", "IBAN column" }
                            span { class: "label-text-alt opacity-60", "optional" }
                        }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            placeholder: "for account auto-create",
                            value: "{iban_column}",
                            oninput: move |e| iban_column.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text", "Raw category column" }
                            span { class: "label-text-alt opacity-60", "optional" }
                        }
                        input {
                            class: "input input-bordered",
                            r#type: "text",
                            placeholder: "bank-supplied transaction type",
                            value: "{raw_category_column}",
                            oninput: move |e| raw_category_column.set(e.value()),
                        }
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: submitting(),
                        onclick: submit,
                        if submitting() { "Saving…" } else if is_edit { "Save" } else { "Create" }
                    }
                }
            }
        }
    }
}

#[component]
fn ImportsTab() -> Element {
    let mut runs: Signal<Vec<ImportRunResponse>> = use_signal(Vec::new);
    let mut schemas: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut accounts: Signal<Vec<AccountResponse>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut reverting: Signal<Option<String>> = use_signal(|| None);
    let mut show_import = use_signal(|| false);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            error.set(None);

            match use_finance::list_import_runs().await {
                Ok(list) => runs.set(list),
                Err(e) => error.set(Some(e)),
            }
            if let Ok(list) = use_finance::list_import_schemas().await {
                schemas.set(list.into_iter().map(|s| (s.id, s.name)).collect());
            }
            if let Ok(list) = use_finance::list_accounts().await {
                accounts.set(list);
            }
            loading.set(false);
        });
    });

    let account_lookup = use_memo(move || {
        accounts()
            .into_iter()
            .map(|a| (a.id, a.name))
            .collect::<HashMap<String, String>>()
    });

    rsx! {
        div { class: "flex justify-between items-center mb-4",
            h2 { class: "text-xl font-semibold", "Imports" }
            button {
                class: "btn btn-primary btn-sm",
                onclick: move |_| show_import.set(true),
                "Import CSV"
            }
        }
        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && runs().is_empty() {
            div { class: "flex justify-center py-8",
                span { class: "loading loading-spinner loading-lg" }
            }
        } else if runs().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No imports yet." }
                p { class: "text-sm mt-2", "Imported CSV files will appear here so you can revert any run that turned out to be a mistake." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead { tr {
                        th { "When" }
                        th { "File" }
                        th { "Account" }
                        th { "Schema" }
                        th { class: "text-right", "Created" }
                        th { class: "text-right", "Skipped" }
                        th { class: "text-right", "Errors" }
                        th { "Status" }
                        th { "" }
                    } }
                    tbody {
                        {runs().iter().map(|r| {
                            let when = r.created_at.get(..10).unwrap_or(&r.created_at).to_string();
                            let account_name = account_lookup().get(&r.account_id).cloned().unwrap_or_else(|| "—".into());
                            let schema_name = schemas().get(&r.schema_id).cloned().unwrap_or_else(|| "—".into());
                            let status = r.status.clone();
                            let run_id = r.id.clone();
                            let is_applied = status == "applied";
                            let busy = reverting().as_deref() == Some(run_id.as_str());
                            rsx! {
                                tr { key: "{r.id}",
                                    td { class: "whitespace-nowrap", "{when}" }
                                    td { class: "max-w-xs truncate", title: "{r.source.filename}", "{r.source.filename}" }
                                    td { "{account_name}" }
                                    td { class: "opacity-70", "{schema_name}" }
                                    td { class: "text-right", "{r.summary.created}" }
                                    td { class: "text-right opacity-70", "{r.summary.skipped_duplicate}" }
                                    td {
                                        class: if r.summary.errored > 0 { "text-right text-error" } else { "text-right opacity-70" },
                                        "{r.summary.errored}"
                                    }
                                    td {
                                        if is_applied {
                                            span { class: "badge badge-success badge-sm", "applied" }
                                        } else {
                                            span { class: "badge badge-ghost badge-sm", "reverted" }
                                        }
                                    }
                                    td { class: "text-right whitespace-nowrap",
                                        if is_applied {
                                            button {
                                                class: "btn btn-ghost btn-xs text-error",
                                                disabled: busy,
                                                onclick: move |_| {
                                                    let id = run_id.clone();
                                                    reverting.set(Some(id.clone()));
                                                    spawn(async move {
                                                        match use_finance::revert_import_run(&id).await {
                                                            Ok(_) => refresh += 1,
                                                            Err(e) => error.set(Some(e)),
                                                        }
                                                        reverting.set(None);
                                                    });
                                                },
                                                if busy { "Reverting…" } else { "Revert" }
                                            }
                                        }
                                    }
                                }
                            }
                        })}
                    }
                }
            }
        }
        if show_import() {
            ImportCsvModal {
                accounts: accounts(),
                on_close: move |_| show_import.set(false),
                on_imported: move |_| { show_import.set(false); refresh += 1; },
            }
        }
    }
}

#[component]
fn ReconcileModal(
    account: AccountResponse,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let today = today_iso();
    let mut on_date = use_signal(|| today.clone());
    let mut actual = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut preview: Signal<Option<ReconcilePreviewResponse>> = use_signal(|| None);
    let mut snapshots: Signal<Vec<BalanceSnapshotResponse>> = use_signal(Vec::new);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);

    let acc_id = account.id.clone();
    let currency = account.currency.clone();

    let acc_id_for_snaps = acc_id.clone();
    use_effect(move || {
        let _ = refresh();
        let id = acc_id_for_snaps.clone();
        spawn(async move {
            if let Ok(list) = use_finance::list_account_snapshots(&id).await {
                snapshots.set(list);
            }
        });
    });

    let acc_id_preview = acc_id.clone();
    let do_preview = move |_| {
        let id = acc_id_preview.clone();
        if busy() { return; }
        let actual_minor = match parse_money(&actual()) {
            Ok(v) => v,
            Err(e) => { error.set(Some(e)); return; }
        };
        let req = ReconcileRequest {
            on_date: on_date(),
            actual_balance_minor: actual_minor,
            note: None,
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            match use_finance::reconcile_preview(&id, &req).await {
                Ok(p) => preview.set(Some(p)),
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let acc_id_apply = acc_id.clone();
    let do_apply = move |_| {
        let id = acc_id_apply.clone();
        if busy() { return; }
        let actual_minor = match parse_money(&actual()) {
            Ok(v) => v,
            Err(e) => { error.set(Some(e)); return; }
        };
        let note_str = note().trim().to_string();
        let req = ReconcileRequest {
            on_date: on_date(),
            actual_balance_minor: actual_minor,
            note: if note_str.is_empty() { None } else { Some(note_str) },
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            match use_finance::reconcile_apply(&id, &req).await {
                Ok(_) => on_done.call(()),
                Err(e) => { error.set(Some(e)); busy.set(false); }
            }
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-lg",
                h3 { class: "font-bold text-lg mb-1", "Reconcile {account.name}" }
                p { class: "text-sm opacity-70 mb-3",
                    "Enter the balance your bank statement shows on a given date. Uncloud creates an adjustment transaction to bridge any difference."
                }

                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3", "{e}" }
                }

                div { class: "grid grid-cols-2 gap-3 mb-3",
                    div { class: "form-control",
                        label { class: "label", span { class: "label-text", "Statement date" } }
                        input {
                            r#type: "date",
                            class: "input input-bordered",
                            value: "{on_date}",
                            oninput: move |e| on_date.set(e.value()),
                        }
                    }
                    div { class: "form-control",
                        label { class: "label",
                            span { class: "label-text", "Statement balance" }
                            span { class: "label-text-alt opacity-60", "{currency}" }
                        }
                        input {
                            r#type: "text",
                            class: "input input-bordered text-right font-mono",
                            placeholder: "0.00",
                            value: "{actual}",
                            oninput: move |e| actual.set(e.value()),
                        }
                    }
                }
                div { class: "form-control mb-3",
                    label { class: "label", span { class: "label-text", "Note (optional)" } }
                    input {
                        class: "input input-bordered",
                        placeholder: "e.g. fee not yet booked",
                        value: "{note}",
                        oninput: move |e| note.set(e.value()),
                    }
                }

                if let Some(p) = preview() {
                    div { class: "stats shadow w-full mb-3 text-sm",
                        div { class: "stat py-2",
                            div { class: "stat-title text-xs", "Computed" }
                            div { class: "stat-value text-base font-mono", "{format_money(p.computed_minor, &currency)}" }
                        }
                        div { class: "stat py-2",
                            div { class: "stat-title text-xs", "Statement" }
                            div { class: "stat-value text-base font-mono", "{format_money(p.actual_minor, &currency)}" }
                        }
                        div { class: "stat py-2",
                            div { class: "stat-title text-xs", "Adjustment" }
                            div {
                                class: if p.delta_minor == 0 {
                                    "stat-value text-base font-mono"
                                } else if p.delta_minor > 0 {
                                    "stat-value text-base font-mono text-success"
                                } else {
                                    "stat-value text-base font-mono text-error"
                                },
                                "{format_money(p.delta_minor, &currency)}"
                            }
                        }
                    }
                }

                if !snapshots().is_empty() {
                    div { class: "mb-3",
                        div { class: "font-medium text-sm mb-1", "Past reconciliations" }
                        ul { class: "text-xs",
                            {snapshots().iter().map(|s| {
                                let s_id = s.id.clone();
                                let s_id_del = s.id.clone();
                                let drift = s.drift_minor;
                                let date = s.on_date.clone();
                                rsx! {
                                    li { key: "{s.id}", class: "flex items-center justify-between py-1 border-b border-base-200",
                                        span {
                                            "{date} → {format_money(s.actual_balance_minor, &currency)}"
                                            if drift != 0 {
                                                span { class: "badge badge-warning badge-xs ml-2",
                                                    "drift {format_money(drift, &currency)}"
                                                }
                                            }
                                        }
                                        span {
                                            if drift != 0 {
                                                button {
                                                    class: "btn btn-ghost btn-xs",
                                                    onclick: move |_| {
                                                        let id = s_id.clone();
                                                        spawn(async move {
                                                            match use_finance::recompute_snapshot(&id).await {
                                                                Ok(_) => refresh += 1,
                                                                Err(e) => error.set(Some(e)),
                                                            }
                                                        });
                                                    },
                                                    "Recompute"
                                                }
                                            }
                                            button {
                                                class: "btn btn-ghost btn-xs text-error",
                                                onclick: move |_| {
                                                    let id = s_id_del.clone();
                                                    spawn(async move {
                                                        match use_finance::delete_snapshot(&id).await {
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

                div { class: "modal-action",
                    button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Close" }
                    button {
                        class: "btn btn-outline",
                        disabled: busy(),
                        onclick: do_preview,
                        "Preview"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: busy() || preview().is_none(),
                        onclick: do_apply,
                        if busy() { "Applying…" } else { "Apply" }
                    }
                }
            }
        }
    }
}

#[component]
fn RulesTab() -> Element {
    let mut rules: Signal<Vec<FinanceRuleResponse>> = use_signal(Vec::new);
    let mut categories: Signal<HashMap<String, String>> = use_signal(HashMap::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0u32);
    let mut show_create = use_signal(|| false);
    let mut edit_target: Signal<Option<FinanceRuleResponse>> = use_signal(|| None);
    let mut applying = use_signal(|| false);
    let mut apply_summary: Signal<Option<(u32, u32)>> = use_signal(|| None);
    let mut reordering = use_signal(|| false);
    let mut drag_rule_idx: Signal<Option<usize>> = use_signal(|| None);
    let mut drop_rule_idx: Signal<Option<usize>> = use_signal(|| None);

    use_effect(move || {
        let _ = refresh();
        spawn(async move {
            loading.set(true);
            match use_finance::list_rules().await {
                Ok(list) => { rules.set(list); error.set(None); }
                Err(e) => error.set(Some(e)),
            }
            if let Ok(cats) = use_finance::list_categories().await {
                categories.set(cats.into_iter().map(|c| (c.id, c.name)).collect());
            }
            loading.set(false);
        });
    });

    let run_apply = move |_| {
        if applying() { return; }
        applying.set(true);
        apply_summary.set(None);
        error.set(None);
        spawn(async move {
            match use_finance::apply_rules().await {
                Ok(s) => apply_summary.set(Some((s.updated, s.still_unmatched))),
                Err(e) => error.set(Some(e)),
            }
            applying.set(false);
        });
    };

    let mut finish_reorder = move || {
        let from = *drag_rule_idx.peek();
        let to = *drop_rule_idx.peek();
        drag_rule_idx.set(None);
        drop_rule_idx.set(None);
        let (Some(from), Some(to)) = (from, to) else { return; };
        if from == to || reordering() {
            return;
        }

        let previous = rules.peek().clone();
        if from >= previous.len() {
            return;
        }
        let mut next = previous.clone();
        let item = next.remove(from);
        let to_clamped = to.min(next.len());
        next.insert(to_clamped, item);
        for (idx, rule) in next.iter_mut().enumerate() {
            rule.priority = (idx as i32).saturating_mul(100);
        }
        let ordered_ids: Vec<String> = next.iter().map(|r| r.id.clone()).collect();
        rules.set(next);
        reordering.set(true);
        error.set(None);
        spawn(async move {
            if let Err(e) = use_finance::reorder_rules(&ordered_ids).await {
                rules.set(previous);
                error.set(Some(e));
            }
            reordering.set(false);
        });
    };

    rsx! {
        div { class: "flex justify-between items-center mb-4",
            h2 { class: "text-xl font-semibold", "Categorization rules" }
            div { class: "flex gap-2",
                button {
                    class: "btn btn-ghost btn-sm",
                    disabled: applying() || rules().is_empty(),
                    onclick: run_apply,
                    if applying() { "Applying…" } else { "Apply to existing" }
                }
                button {
                    class: "btn btn-primary btn-sm",
                    onclick: move |_| show_create.set(true),
                    "New rule"
                }
            }
        }
        if let Some((updated, unmatched)) = apply_summary() {
            div { class: "alert alert-success mb-3",
                "Updated {updated} transaction(s). Still uncategorized: {unmatched}."
            }
        }
        if let Some(e) = error() {
            div { class: "alert alert-error mb-3", "{e}" }
        }
        if loading() && rules().is_empty() {
            div { class: "flex justify-center py-8",
                span { class: "loading loading-spinner loading-lg" }
            }
        } else if rules().is_empty() {
            div { class: "text-center py-10 opacity-60",
                p { "No rules yet." }
                p { class: "text-sm mt-2", "Rules auto-categorize transactions whose description matches a pattern. They apply during CSV import and via the \"Apply to existing\" button." }
            }
        } else {
            div { class: "overflow-x-auto",
                table { class: "table table-zebra",
                    thead { tr {
                        th { class: "w-10", "" }
                        th { "Name" }
                        th { "Pattern" }
                        th { "Match" }
                        th { "Category" }
                        th { "" }
                    } }
                    tbody {
                        onpointerup: move |_| finish_reorder(),
                        onpointercancel: move |_| {
                            drag_rule_idx.set(None);
                            drop_rule_idx.set(None);
                        },
                        {rules().iter().enumerate().map(|(idx, r)| {
                            let r_edit = r.clone();
                            let r_id = r.id.clone();
                            let category_name = categories().get(&r.category_id).cloned().unwrap_or_else(|| "—".into());
                            let kind = r.pattern_kind.clone();
                            let enabled = r.enabled;
                            let drag_source = *drag_rule_idx.read() == Some(idx);
                            let drag_from = *drag_rule_idx.read();
                            let drop_target = drag_rule_idx.read().is_some()
                                && *drop_rule_idx.read() == Some(idx)
                                && !drag_source;
                            let row_class = if drag_source {
                                "opacity-30"
                            } else if drop_target {
                                if drag_from.unwrap_or(0) > idx {
                                    "border-t-2 border-t-primary bg-primary/5"
                                } else {
                                    "border-b-2 border-b-primary bg-primary/5"
                                }
                            } else if enabled {
                                "hover:bg-base-200"
                            } else {
                                "opacity-50 hover:bg-base-200"
                            };
                            rsx! {
                                tr {
                                    key: "{r.id}",
                                    class: "{row_class} group transition-colors",
                                    onpointerenter: move |_| {
                                        if drag_rule_idx.peek().is_some() {
                                            drop_rule_idx.set(Some(idx));
                                        }
                                    },
                                    td {
                                        class: "w-10 px-1 cursor-grab active:cursor-grabbing",
                                        style: "touch-action: none;",
                                        onpointerdown: move |e: Event<PointerData>| {
                                            e.stop_propagation();
                                            e.prevent_default();
                                            drag_rule_idx.set(Some(idx));
                                            drop_rule_idx.set(Some(idx));
                                        },
                                        span {
                                            class: "inline-flex",
                                            title: "Drag to reorder",
                                            IconGripVertical {
                                                class: "w-4 h-4 text-base-content/30 group-hover:text-base-content/60".to_string()
                                            }
                                        }
                                    }
                                    td {
                                        "{r.name}"
                                        if !enabled { span { class: "badge badge-ghost ml-2 text-xs", "disabled" } }
                                    }
                                    td { class: "font-mono text-xs", "{r.pattern}" }
                                    td { class: "opacity-70 text-xs", "{kind}" }
                                    td { "{category_name}" }
                                    td { class: "text-right whitespace-nowrap",
                                        button {
                                            class: "btn btn-ghost btn-xs",
                                            onclick: move |_| edit_target.set(Some(r_edit.clone())),
                                            "Edit"
                                        }
                                        button {
                                            class: "btn btn-ghost btn-xs text-error",
                                            onclick: move |_| {
                                                let id = r_id.clone();
                                                spawn(async move {
                                                    match use_finance::delete_rule(&id).await {
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
            RuleFormModal {
                initial: None,
                categories: categories(),
                on_close: move |_| show_create.set(false),
                on_saved: move |_| { show_create.set(false); refresh += 1; },
            }
        }
        if let Some(r) = edit_target() {
            RuleFormModal {
                key: "{r.id}",
                initial: Some(r.clone()),
                categories: categories(),
                on_close: move |_| edit_target.set(None),
                on_saved: move |_| { edit_target.set(None); refresh += 1; },
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct RulePrefill {
    pub pattern: String,
    /// Pre-selected category, falls back to "first category" when None.
    pub category_id: Option<String>,
    /// Used as a starting name (typically derived from the transaction
    /// description), but the user can change it before saving.
    pub name: String,
}

#[component]
fn RuleFormModal(
    initial: Option<FinanceRuleResponse>,
    #[props(default)] prefill: Option<RulePrefill>,
    categories: HashMap<String, String>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let is_edit = initial.is_some();
    let editing_id = initial.as_ref().map(|r| r.id.clone());
    let initial_priority = initial.as_ref().map(|r| r.priority).unwrap_or(0);

    let mut name = use_signal(|| {
        initial.as_ref().map(|r| r.name.clone())
            .or_else(|| prefill.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default()
    });
    let mut pattern = use_signal(|| {
        initial.as_ref().map(|r| r.pattern.clone())
            .or_else(|| prefill.as_ref().map(|p| p.pattern.clone()))
            .unwrap_or_default()
    });
    let mut pattern_kind = use_signal(|| {
        initial.as_ref().map(|r| r.pattern_kind.clone()).unwrap_or_else(|| "substring".into())
    });
    let mut case_insensitive = use_signal(|| {
        initial.as_ref().map(|r| r.case_insensitive).unwrap_or(true)
    });
    let mut category_id = use_signal(|| {
        initial.as_ref().map(|r| r.category_id.clone())
            .or_else(|| prefill.as_ref().and_then(|p| p.category_id.clone()))
            .or_else(|| categories.iter().next().map(|(k, _)| k.clone()))
            .unwrap_or_default()
    });
    let mut enabled = use_signal(|| initial.as_ref().map(|r| r.enabled).unwrap_or(true));

    let mut local_cats: Signal<HashMap<String, String>> = use_signal(|| categories);
    let mut show_new_cat = use_signal(|| false);
    let mut new_cat_name = use_signal(String::new);
    let mut new_cat_busy = use_signal(|| false);

    let mut test_results: Signal<Option<TestRuleResponse>> = use_signal(|| None);
    let mut submitting = use_signal(|| false);
    let mut applying = use_signal(|| false);
    let mut apply_result: Signal<Option<(u32, u32)>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let cat_options = use_memo(move || {
        let mut v: Vec<(String, String)> = local_cats().into_iter().collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    });

    let run_test = move |_| {
        let req = TestRuleRequest {
            pattern: pattern(),
            pattern_kind: pattern_kind(),
            case_insensitive: case_insensitive(),
        };
        spawn(async move {
            match use_finance::test_rule(&req).await {
                Ok(r) => test_results.set(Some(r)),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let build_req = move || -> std::result::Result<FinanceRuleRequest, String> {
        Ok(FinanceRuleRequest {
            name: name(),
            pattern: pattern(),
            pattern_kind: pattern_kind(),
            case_insensitive: case_insensitive(),
            category_id: category_id(),
            priority: initial_priority,
            enabled: enabled(),
        })
    };

    let editing_id_a = editing_id.clone();
    let submit = move |_| {
        if submitting() || applying() { return; }
        let req = match build_req() {
            Ok(r) => r,
            Err(e) => { error.set(Some(e)); return; }
        };
        submitting.set(true);
        error.set(None);
        let editing_id = editing_id_a.clone();
        spawn(async move {
            let result = match editing_id {
                Some(id) => use_finance::update_rule(&id, &req).await,
                None => use_finance::create_rule(&req).await,
            };
            match result {
                Ok(_) => on_saved.call(()),
                Err(e) => { error.set(Some(e)); submitting.set(false); }
            }
        });
    };

    let editing_id_b = editing_id.clone();
    let submit_and_apply = move |_| {
        if submitting() || applying() { return; }
        let req = match build_req() {
            Ok(r) => r,
            Err(e) => { error.set(Some(e)); return; }
        };
        submitting.set(true);
        error.set(None);
        apply_result.set(None);
        let editing_id = editing_id_b.clone();
        spawn(async move {
            let saved = match editing_id {
                Some(id) => use_finance::update_rule(&id, &req).await,
                None => use_finance::create_rule(&req).await,
            };
            match saved {
                Ok(_) => {
                    submitting.set(false);
                    applying.set(true);
                    match use_finance::apply_rules().await {
                        Ok(s) => apply_result.set(Some((s.updated, s.still_unmatched))),
                        Err(e) => error.set(Some(e)),
                    }
                    applying.set(false);
                }
                Err(e) => { error.set(Some(e)); submitting.set(false); }
            }
        });
    };

    rsx! {
        div { class: "modal modal-open",
            div { class: "modal-box max-w-2xl",
                h3 { class: "font-bold text-lg mb-3",
                    if is_edit { "Edit rule" } else { "New rule" }
                }
                if let Some(e) = error() {
                    div { class: "alert alert-error mb-3", "{e}" }
                }
                div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                    div { class: "form-control md:col-span-2",
                        label { class: "label", span { class: "label-text", "Name" } }
                        input {
                            class: "input input-bordered",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    div { class: "form-control md:col-span-2",
                        label { class: "label",
                            span { class: "label-text", "Pattern" }
                        }
                        input {
                            class: "input input-bordered font-mono",
                            value: "{pattern}",
                            placeholder: "e.g. spotify, ^Miete, Uber Eats",
                            oninput: move |e| pattern.set(e.value()),
                        }
                    }
                    div { class: "form-control min-w-0",
                        span { class: "block text-sm pb-2", "Match" }
                        select {
                            class: "select select-bordered w-full",
                            value: "{pattern_kind}",
                            onchange: move |e| pattern_kind.set(e.value()),
                            option { value: "substring", "Contains" }
                            option { value: "starts_with", "Starts with" }
                            option { value: "regex", "Regex" }
                        }
                    }
                    div { class: "form-control min-w-0",
                        span { class: "block text-sm pb-2", "Options" }
                        label { class: "flex min-h-12 items-center gap-3 rounded-lg border border-base-300 px-3 cursor-pointer",
                            input {
                                r#type: "checkbox",
                                class: "checkbox checkbox-sm",
                                checked: case_insensitive(),
                                oninput: move |e| case_insensitive.set(e.checked()),
                            }
                            span { class: "label-text", "Case-insensitive" }
                        }
                    }
                    div { class: "form-control md:col-span-2 min-w-0",
                        label { class: "label",
                            span { class: "label-text", "Category" }
                            if !show_new_cat() {
                                button {
                                    r#type: "button",
                                    class: "label-text-alt link link-primary",
                                    onclick: move |_| { new_cat_name.set(String::new()); show_new_cat.set(true); },
                                    "+ New"
                                }
                            }
                        }
                        if show_new_cat() {
                            div { class: "grid grid-cols-1 sm:grid-cols-[minmax(0,1fr)_auto_auto] gap-2",
                                input {
                                    class: "input input-bordered input-sm w-full min-w-0",
                                    placeholder: "Category name",
                                    value: "{new_cat_name}",
                                    oninput: move |e| new_cat_name.set(e.value()),
                                }
                                button {
                                    r#type: "button",
                                    class: "btn btn-sm",
                                    disabled: new_cat_busy() || new_cat_name().trim().is_empty(),
                                    onclick: move |_| {
                                        new_cat_busy.set(true);
                                        let name = new_cat_name().trim().to_string();
                                        spawn(async move {
                                            let req = CreateFinanceCategoryRequest {
                                                name: name.clone(),
                                                parent_id: None,
                                                colour: None,
                                            };
                                            match use_finance::create_category(&req).await {
                                                Ok(c) => {
                                                    let new_id = c.id.clone();
                                                    let new_name = c.name.clone();
                                                    local_cats.with_mut(|m| { m.insert(new_id.clone(), new_name); });
                                                    category_id.set(new_id);
                                                    show_new_cat.set(false);
                                                }
                                                Err(e) => error.set(Some(e)),
                                            }
                                            new_cat_busy.set(false);
                                        });
                                    },
                                    if new_cat_busy() { "…" } else { "Add" }
                                }
                                button {
                                    r#type: "button",
                                    class: "btn btn-ghost btn-sm",
                                    onclick: move |_| show_new_cat.set(false),
                                    "Cancel"
                                }
                            }
                        } else {
                            select {
                                class: "select select-bordered w-full",
                                value: "{category_id}",
                                onchange: move |e| category_id.set(e.value()),
                                {cat_options().into_iter().map(|(id, n)| rsx! {
                                    option {
                                        key: "{id}",
                                        value: "{id}",
                                        selected: category_id() == id,
                                        "{n}"
                                    }
                                })}
                            }
                        }
                    }
                    div { class: "form-control md:col-span-2 min-w-0",
                        span { class: "block text-sm pb-2", "Status" }
                        label { class: "flex min-h-12 items-center gap-3 rounded-lg border border-base-300 px-3 cursor-pointer",
                            input {
                                r#type: "checkbox",
                                class: "checkbox checkbox-sm",
                                checked: enabled(),
                                oninput: move |e| enabled.set(e.checked()),
                            }
                            span { class: "label-text", "Enabled" }
                        }
                    }
                }
                div { class: "divider my-2" }
                div { class: "flex items-center justify-between mb-2",
                    div { class: "font-medium text-sm", "Test against recent transactions" }
                    button {
                        class: "btn btn-ghost btn-sm",
                        onclick: run_test,
                        "Run test"
                    }
                }
                if let Some(r) = test_results() {
                    if r.matches.is_empty() {
                        div { class: "opacity-60 text-sm", "No matches in the last {r.sampled} transactions." }
                    } else {
                        div { class: "text-sm",
                            div { class: "mb-1 opacity-70", "{r.matches.len()} match(es) in the last {r.sampled}:" }
                            ul { class: "max-h-40 overflow-y-auto text-xs",
                                {r.matches.iter().map(|m: &TestRuleMatch| {
                                    let date = m.date.get(..10).unwrap_or(&m.date).to_string();
                                    rsx! {
                                        li { key: "{m.transaction_id}", class: "py-0.5",
                                            span { class: "opacity-60 mr-2", "{date}" }
                                            "{m.description}"
                                        }
                                    }
                                })}
                            }
                        }
                    }
                }

                if let Some((updated, unmatched)) = apply_result() {
                    div { class: "alert alert-success mb-2",
                        "Saved & applied. Updated {updated} transaction(s); still uncategorized: {unmatched}."
                    }
                }

                div { class: "modal-action",
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| {
                            if apply_result().is_some() {
                                // An apply already wrote new categories;
                                // the parent must refresh — route through
                                // on_saved (which both refreshes and closes).
                                on_saved.call(());
                            } else {
                                on_close.call(());
                            }
                        },
                        if apply_result().is_some() { "Close" } else { "Cancel" }
                    }
                    if apply_result().is_none() {
                        button {
                            class: "btn",
                            disabled: submitting() || applying(),
                            onclick: submit_and_apply,
                            if applying() { "Applying…" } else { "Save & apply" }
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: submitting() || applying(),
                            onclick: submit,
                            if submitting() { "Saving…" } else if is_edit { "Save" } else { "Create" }
                        }
                    }
                }
            }
        }
    }
}
