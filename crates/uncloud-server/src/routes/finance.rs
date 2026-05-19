//! Finance tracker — accounts, categories, transactions (manual entry).
//!
//! Foundation slice. CSV import lands in a follow-up, expected to reshape
//! the transaction model around `source_ref` upserts; existing fields are
//! preserved through that work.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::{DateTime, TimeZone, Utc};
use futures::TryStreamExt;
use mongodb::bson::oid::ObjectId;
use mongodb::bson::Bson;
use mongodb::options::FindOptions;

use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};
use crate::finance_import::{self, sparkasse_camt_v8, ParseError, ParsedRow};
use crate::middleware::AuthUser;
use crate::finance_rules::{self, RuleEngine};
use crate::models::{
    AmountSignConvention, BalanceSnapshot, CategorySource, CurrencySource, DecimalSeparator,
    FinanceAccount, FinanceCategory, FinanceRule, FinanceTransaction, ImportRun,
    ImportRunError as ModelImportRunError, ImportRunStatus, ImportRunSummary, ImportSchema,
    ImportSource, ImportSourceKind, RulePatternKind, TransactionLeg,
};
use crate::AppState;
use uncloud_common::{
    AccountBalanceResponse, AccountResponse, ApplyRulesResponse, BalanceSnapshotResponse,
    CategorySummaryItem, CategorySummaryResponse, CreateAccountRequest,
    CreateFinanceCategoryRequest, CreateTransactionRequest, FinanceCategoryResponse,
    FinanceRuleRequest, FinanceRuleResponse, ImportCsvResponse, ImportRowError, ImportRunResponse,
    ImportRunSourceDto, ImportRunSummaryDto, ImportSchemaRequest, ImportSchemaResponse,
    ListTransactionsQuery, ReconcilePreviewResponse, ReconcileRequest, TestRuleMatch,
    TestRuleRequest, TestRuleResponse, TransactionListResponse, TransactionResponse,
    UpdateAccountRequest, UpdateFinanceCategoryRequest, UpdateTransactionRequest,
};

const ACCOUNTS: &str = "finance_accounts";
const CATEGORIES: &str = "finance_categories";
const TRANSACTIONS: &str = "finance_transactions";
const IMPORT_SCHEMAS: &str = "finance_import_schemas";
const IMPORT_RUNS: &str = "finance_import_runs";
const BALANCE_SNAPSHOTS: &str = "finance_balance_snapshots";
const RULES: &str = "finance_rules";
const RECONCILIATION_CATEGORY: &str = "Reconciliation";

const ALLOWED_CURRENCY_LEN: usize = 3;

fn require_finance(state: &AppState) -> Result<()> {
    if !state.config.features.finance {
        return Err(AppError::Forbidden("Finance feature disabled".into()));
    }
    Ok(())
}

fn parse_oid(s: &str, name: &str) -> Result<ObjectId> {
    ObjectId::parse_str(s).map_err(|_| AppError::BadRequest(format!("Invalid {}", name)))
}

fn parse_date(input: &str) -> Result<DateTime<Utc>> {
    // Accept "YYYY-MM-DD" (treat as midnight UTC) or RFC 3339.
    if let Ok(rfc) = DateTime::parse_from_rfc3339(input) {
        return Ok(rfc.with_timezone(&Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0).unwrap();
        return Ok(Utc.from_utc_datetime(&dt));
    }
    Err(AppError::BadRequest(format!(
        "date must be YYYY-MM-DD or RFC 3339, got `{}`",
        input
    )))
}

fn validate_currency(c: &str) -> Result<String> {
    let trimmed = c.trim();
    if trimmed.len() != ALLOWED_CURRENCY_LEN
        || !trimmed.chars().all(|c| c.is_ascii_alphabetic())
    {
        return Err(AppError::BadRequest(format!(
            "currency must be a 3-letter ISO 4217 code, got `{}`",
            c
        )));
    }
    Ok(trimmed.to_ascii_uppercase())
}

fn account_to_response(a: &FinanceAccount) -> AccountResponse {
    AccountResponse {
        id: a.id.to_hex(),
        name: a.name.clone(),
        account_type: a.account_type.clone(),
        currency: a.currency.clone(),
        opening_balance_minor: a.opening_balance_minor,
        iban: a.iban.clone(),
        created_at: a.created_at.to_rfc3339(),
        updated_at: a.updated_at.to_rfc3339(),
        archived_at: a.archived_at.map(|d| d.to_rfc3339()),
    }
}

fn normalize_iban(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_ascii_uppercase())
    }
}

fn category_to_response(c: &FinanceCategory) -> FinanceCategoryResponse {
    FinanceCategoryResponse {
        id: c.id.to_hex(),
        parent_id: c.parent_id.map(|p| p.to_hex()),
        name: c.name.clone(),
        colour: c.colour.clone(),
        created_at: c.created_at.to_rfc3339(),
    }
}

fn transaction_to_response(t: &FinanceTransaction) -> TransactionResponse {
    let single_category = if t.legs.len() == 1 {
        t.legs[0].category_id.map(|c| c.to_hex())
    } else {
        None
    };
    TransactionResponse {
        id: t.id.to_hex(),
        account_id: t.account_id.to_hex(),
        date: t.date.to_rfc3339(),
        amount_minor: t.amount_minor,
        currency: t.currency.clone(),
        description: t.description.clone(),
        category_id: single_category,
        notes: t.notes.clone(),
        source_ref: t.source_ref.clone(),
        raw_bank_category: t.raw_bank_category.clone(),
        is_split: t.legs.len() > 1,
        source_snapshot_id: t.source_snapshot_id.map(|s| s.to_hex()),
        created_at: t.created_at.to_rfc3339(),
        updated_at: t.updated_at.to_rfc3339(),
    }
}

async fn find_account(
    state: &AppState,
    owner_id: ObjectId,
    id: ObjectId,
) -> Result<FinanceAccount> {
    state
        .db
        .collection::<FinanceAccount>(ACCOUNTS)
        .find_one(doc! { "_id": id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Account not found".into()))
}

// ── Accounts ─────────────────────────────────────────────────────────────

pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<AccountResponse>>> {
    require_finance(&state)?;
    let coll = state.db.collection::<FinanceAccount>(ACCOUNTS);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "name": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(a) = cursor.try_next().await? {
        out.push(account_to_response(&a));
    }
    Ok(Json(out))
}

pub async fn create_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<AccountResponse>)> {
    require_finance(&state)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Account name is required".into()));
    }
    let currency = validate_currency(&req.currency)?;
    let iban = req.iban.as_deref().and_then(normalize_iban);
    let now = Utc::now();
    let account = FinanceAccount {
        id: ObjectId::new(),
        owner_id: user.id,
        name,
        account_type: req.account_type.trim().to_string(),
        currency,
        opening_balance_minor: req.opening_balance_minor,
        iban,
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    state
        .db
        .collection::<FinanceAccount>(ACCOUNTS)
        .insert_one(&account)
        .await
        .map_err(|e| {
            if is_duplicate_key_error(&e) {
                AppError::BadRequest(
                    "Another account already uses this IBAN".into(),
                )
            } else {
                AppError::from(e)
            }
        })?;
    Ok((StatusCode::CREATED, Json(account_to_response(&account))))
}

pub async fn update_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateAccountRequest>,
) -> Result<Json<AccountResponse>> {
    require_finance(&state)?;
    let id = parse_oid(&id, "account id")?;
    let existing = find_account(&state, user.id, id).await?;

    let mut set = doc! { "updated_at": bson::DateTime::from_chrono(Utc::now()) };
    if let Some(name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppError::BadRequest("Name cannot be empty".into()));
        }
        set.insert("name", trimmed);
    }
    if let Some(t) = req.account_type {
        set.insert("account_type", t.trim());
    }
    if let Some(b) = req.opening_balance_minor {
        set.insert("opening_balance_minor", b);
    }
    let mut unset = doc! {};
    if let Some(iban_opt) = req.iban {
        match iban_opt.as_deref().and_then(normalize_iban) {
            Some(norm) => { set.insert("iban", norm); }
            None => { unset.insert("iban", ""); }
        }
    }
    match req.archived {
        Some(true) if existing.archived_at.is_none() => {
            set.insert("archived_at", bson::DateTime::from_chrono(Utc::now()));
        }
        Some(false) if existing.archived_at.is_some() => {
            unset.insert("archived_at", "");
        }
        _ => {}
    }
    let mut update = doc! { "$set": set };
    if !unset.is_empty() {
        update.insert("$unset", unset);
    }

    state
        .db
        .collection::<FinanceAccount>(ACCOUNTS)
        .update_one(doc! { "_id": id, "owner_id": user.id }, update)
        .await?;

    let updated = find_account(&state, user.id, id).await?;
    Ok(Json(account_to_response(&updated)))
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let id = parse_oid(&id, "account id")?;
    find_account(&state, user.id, id).await?;
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let in_use = txns
        .count_documents(doc! { "owner_id": user.id, "account_id": id })
        .await?;
    if in_use > 0 {
        return Err(AppError::BadRequest(
            "Account has transactions; archive instead or delete its transactions first".into(),
        ));
    }
    state
        .db
        .collection::<FinanceAccount>(ACCOUNTS)
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_account_balance(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<AccountBalanceResponse>> {
    require_finance(&state)?;
    let id = parse_oid(&id, "account id")?;
    let account = find_account(&state, user.id, id).await?;
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);

    let pipeline = vec![
        doc! { "$match": { "owner_id": user.id, "account_id": id } },
        doc! { "$group": {
            "_id": null,
            "sum": { "$sum": "$amount_minor" },
            "count": { "$sum": 1 },
        } },
    ];
    let mut cursor = txns.aggregate(pipeline).await?;
    let (sum, count) = if let Some(doc) = cursor.try_next().await? {
        let sum = doc.get_i64("sum").unwrap_or(0);
        let count = doc.get_i32("count").unwrap_or(0) as u64;
        (sum, count)
    } else {
        (0, 0)
    };

    Ok(Json(AccountBalanceResponse {
        account_id: id.to_hex(),
        currency: account.currency,
        opening_balance_minor: account.opening_balance_minor,
        transaction_total_minor: sum,
        balance_minor: account.opening_balance_minor + sum,
        transaction_count: count,
    }))
}

// ── Categories ───────────────────────────────────────────────────────────

pub async fn list_categories(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<FinanceCategoryResponse>>> {
    require_finance(&state)?;
    let coll = state.db.collection::<FinanceCategory>(CATEGORIES);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "name": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(c) = cursor.try_next().await? {
        out.push(category_to_response(&c));
    }
    Ok(Json(out))
}

pub async fn create_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateFinanceCategoryRequest>,
) -> Result<(StatusCode, Json<FinanceCategoryResponse>)> {
    require_finance(&state)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Category name is required".into()));
    }
    let parent_id = match req.parent_id.as_deref() {
        Some(s) if !s.is_empty() => {
            let oid = parse_oid(s, "parent_id")?;
            // ensure parent is one of user's categories and not itself nested
            let coll = state.db.collection::<FinanceCategory>(CATEGORIES);
            let parent = coll
                .find_one(doc! { "_id": oid, "owner_id": user.id })
                .await?
                .ok_or_else(|| AppError::BadRequest("Parent category not found".into()))?;
            if parent.parent_id.is_some() {
                return Err(AppError::BadRequest(
                    "Categories are limited to two levels".into(),
                ));
            }
            Some(oid)
        }
        _ => None,
    };
    let cat = FinanceCategory {
        id: ObjectId::new(),
        owner_id: user.id,
        parent_id,
        name,
        colour: req.colour.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        created_at: Utc::now(),
    };
    state
        .db
        .collection::<FinanceCategory>(CATEGORIES)
        .insert_one(&cat)
        .await?;
    Ok((StatusCode::CREATED, Json(category_to_response(&cat))))
}

pub async fn update_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateFinanceCategoryRequest>,
) -> Result<Json<FinanceCategoryResponse>> {
    require_finance(&state)?;
    let id = parse_oid(&id, "category id")?;
    let coll = state.db.collection::<FinanceCategory>(CATEGORIES);
    let _existing = coll
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Category not found".into()))?;
    let mut set = doc! {};
    let mut unset = doc! {};
    if let Some(name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppError::BadRequest("Name cannot be empty".into()));
        }
        set.insert("name", trimmed);
    }
    if let Some(p) = req.parent_id {
        if p.is_empty() {
            unset.insert("parent_id", "");
        } else {
            let oid = parse_oid(&p, "parent_id")?;
            if oid == id {
                return Err(AppError::BadRequest("Category cannot be its own parent".into()));
            }
            let parent = coll
                .find_one(doc! { "_id": oid, "owner_id": user.id })
                .await?
                .ok_or_else(|| AppError::BadRequest("Parent category not found".into()))?;
            if parent.parent_id.is_some() {
                return Err(AppError::BadRequest(
                    "Categories are limited to two levels".into(),
                ));
            }
            set.insert("parent_id", oid);
        }
    }
    if let Some(c) = req.colour {
        let trimmed = c.trim();
        if trimmed.is_empty() {
            unset.insert("colour", "");
        } else {
            set.insert("colour", trimmed);
        }
    }
    let mut update = doc! {};
    if !set.is_empty() {
        update.insert("$set", set);
    }
    if !unset.is_empty() {
        update.insert("$unset", unset);
    }
    if !update.is_empty() {
        coll.update_one(doc! { "_id": id, "owner_id": user.id }, update)
            .await?;
    }
    let updated = coll
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Category not found".into()))?;
    Ok(Json(category_to_response(&updated)))
}

pub async fn delete_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let id = parse_oid(&id, "category id")?;
    let coll = state.db.collection::<FinanceCategory>(CATEGORIES);
    let _existing = coll
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Category not found".into()))?;
    // unassign from children + transactions before delete
    coll.update_many(
        doc! { "parent_id": id, "owner_id": user.id },
        doc! { "$unset": { "parent_id": "" } },
    )
    .await?;
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    txns.update_many(
        doc! { "owner_id": user.id, "legs.category_id": id },
        doc! {
            "$unset": { "legs.$[el].category_id": "" },
            "$set": { "legs.$[el].category_source": "unset", "updated_at": bson::DateTime::from_chrono(Utc::now()) },
        },
    )
    .with_options(
        mongodb::options::UpdateOptions::builder()
            .array_filters(vec![doc! { "el.category_id": id }])
            .build(),
    )
    .await?;
    coll.delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Transactions ─────────────────────────────────────────────────────────

/// Builds the Mongo match filter for the user's transactions
/// according to a `ListTransactionsQuery`. Shared between the list
/// route and the category-summary aggregation so the two stay in sync.
fn build_tx_filter(user: &AuthUser, q: &ListTransactionsQuery) -> Result<mongodb::bson::Document> {
    let mut filter = doc! { "owner_id": user.id };
    if let Some(acc) = q.account_id.as_deref() {
        filter.insert("account_id", parse_oid(acc, "account_id")?);
    }
    if let Some(cat) = q.category_id.as_deref() {
        filter.insert("legs.category_id", parse_oid(cat, "category_id")?);
    }
    if q.uncategorized.unwrap_or(false) {
        filter.insert("legs.category_id", Bson::Null);
    }
    // Reconciliation adjustments are hidden by default — they're not
    // real spending and clutter the list. Caller can opt them back in.
    // `null` matches both absent (older docs) and explicitly-null fields
    // — Mongo treats them equivalently for equality.
    if !q.include_reconciliations.unwrap_or(false) {
        filter.insert("source_snapshot_id", Bson::Null);
    }
    let mut date_range = doc! {};
    if let Some(from) = q.from.as_deref() {
        date_range.insert("$gte", bson::DateTime::from_chrono(parse_date(from)?));
    }
    if let Some(to) = q.to.as_deref() {
        date_range.insert("$lte", bson::DateTime::from_chrono(parse_date(to)?));
    }
    if !date_range.is_empty() {
        filter.insert("date", date_range);
    }
    Ok(filter)
}

pub async fn list_transactions(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ListTransactionsQuery>,
) -> Result<Json<TransactionListResponse>> {
    require_finance(&state)?;
    let filter = build_tx_filter(&user, &q)?;

    let coll = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let total = coll.count_documents(filter.clone()).await?;

    let limit = q.limit.unwrap_or(100).clamp(1, 500) as i64;
    let skip = q.skip.unwrap_or(0) as u64;
    let opts = FindOptions::builder()
        .sort(doc! { "date": -1, "_id": -1 })
        .skip(skip)
        .limit(limit)
        .build();
    let mut cursor = coll.find(filter).with_options(opts).await?;
    let mut items = Vec::new();
    while let Some(t) = cursor.try_next().await? {
        items.push(transaction_to_response(&t));
    }
    Ok(Json(TransactionListResponse { items, total }))
}

/// Per-category income/expense aggregation for the same filter the
/// transactions list uses. Single leg-level pass — handles split
/// transactions correctly because each leg is attributed to its own
/// category at its own amount.
pub async fn transaction_category_summary(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ListTransactionsQuery>,
) -> Result<Json<CategorySummaryResponse>> {
    require_finance(&state)?;
    let filter = build_tx_filter(&user, &q)?;

    let coll = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let pipeline = vec![
        doc! { "$match": filter },
        doc! { "$unwind": "$legs" },
        doc! { "$group": {
            "_id": "$legs.category_id",
            "income": { "$sum": {
                "$cond": [ { "$gte": ["$legs.amount_minor", 0] }, "$legs.amount_minor", 0 ]
            } },
            "expense": { "$sum": {
                "$cond": [ { "$lt": ["$legs.amount_minor", 0] }, "$legs.amount_minor", 0 ]
            } },
        } },
    ];

    let mut cursor = coll.aggregate(pipeline).await?;
    let mut items = Vec::new();
    let mut income_total: i64 = 0;
    let mut expense_total: i64 = 0;
    while let Some(d) = cursor.try_next().await? {
        let category_id = match d.get("_id") {
            Some(Bson::ObjectId(oid)) => Some(oid.to_hex()),
            _ => None,
        };
        let income = d.get_i64("income").unwrap_or_else(|_| {
            d.get_i32("income").map(|n| n as i64).unwrap_or(0)
        });
        let expense = d.get_i64("expense").unwrap_or_else(|_| {
            d.get_i32("expense").map(|n| n as i64).unwrap_or(0)
        });
        income_total += income;
        expense_total += expense;
        items.push(CategorySummaryItem {
            category_id,
            income_minor: income,
            expense_minor: expense,
        });
    }

    Ok(Json(CategorySummaryResponse {
        items,
        income_total_minor: income_total,
        expense_total_minor: expense_total,
    }))
}

pub async fn create_transaction(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<CreateTransactionRequest>,
) -> Result<(StatusCode, Json<TransactionResponse>)> {
    require_finance(&state)?;
    let account_id = parse_oid(&req.account_id, "account_id")?;
    let account = find_account(&state, user.id, account_id).await?;
    let date = parse_date(&req.date)?;
    let description = req.description.trim().to_string();
    if description.is_empty() {
        return Err(AppError::BadRequest("Description is required".into()));
    }
    let category_id = match req.category_id.as_deref() {
        Some(s) if !s.is_empty() => {
            let oid = parse_oid(s, "category_id")?;
            let cat = state
                .db
                .collection::<FinanceCategory>(CATEGORIES)
                .find_one(doc! { "_id": oid, "owner_id": user.id })
                .await?;
            if cat.is_none() {
                return Err(AppError::BadRequest("Category not found".into()));
            }
            Some(oid)
        }
        _ => None,
    };
    let now = Utc::now();
    let leg = TransactionLeg {
        amount_minor: req.amount_minor,
        category_id,
        category_source: if category_id.is_some() {
            CategorySource::User
        } else {
            CategorySource::Unset
        },
        rule_id: None,
        note: None,
    };
    let txn = FinanceTransaction {
        id: ObjectId::new(),
        owner_id: user.id,
        account_id: account.id,
        currency: account.currency.clone(),
        amount_minor: req.amount_minor,
        description,
        date,
        source_ref: None,
        raw_bank_category: None,
        notes: req.notes.and_then(|n| {
            let t = n.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }),
        tags: vec![],
        legs: vec![leg],
        import_run_id: None,
        source_snapshot_id: None,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .collection::<FinanceTransaction>(TRANSACTIONS)
        .insert_one(&txn)
        .await?;
    Ok((StatusCode::CREATED, Json(transaction_to_response(&txn))))
}

pub async fn update_transaction(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateTransactionRequest>,
) -> Result<Json<TransactionResponse>> {
    require_finance(&state)?;
    let id = parse_oid(&id, "transaction id")?;
    let coll = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let existing = coll
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Transaction not found".into()))?;
    if existing.legs.len() > 1 {
        return Err(AppError::BadRequest(
            "Split transactions cannot be edited via this endpoint yet".into(),
        ));
    }

    let mut set = doc! { "updated_at": bson::DateTime::from_chrono(Utc::now()) };
    if let Some(d) = req.date {
        set.insert("date", bson::DateTime::from_chrono(parse_date(&d)?));
    }
    if let Some(desc) = req.description {
        let t = desc.trim();
        if t.is_empty() {
            return Err(AppError::BadRequest("Description cannot be empty".into()));
        }
        set.insert("description", t);
    }
    if let Some(amt) = req.amount_minor {
        set.insert("amount_minor", amt);
        // mirror onto the single leg's amount
        set.insert("legs.0.amount_minor", amt);
    }
    if let Some(cat_opt) = req.category_id {
        match cat_opt {
            Some(s) if !s.is_empty() => {
                let oid = parse_oid(&s, "category_id")?;
                let cat = state
                    .db
                    .collection::<FinanceCategory>(CATEGORIES)
                    .find_one(doc! { "_id": oid, "owner_id": user.id })
                    .await?;
                if cat.is_none() {
                    return Err(AppError::BadRequest("Category not found".into()));
                }
                set.insert("legs.0.category_id", oid);
                set.insert("legs.0.category_source", "user");
            }
            _ => {
                set.insert("legs.0.category_id", Bson::Null);
                set.insert("legs.0.category_source", "unset");
            }
        }
    }
    if let Some(notes_opt) = req.notes {
        match notes_opt {
            Some(n) => {
                let t = n.trim();
                if t.is_empty() {
                    set.insert("notes", Bson::Null);
                } else {
                    set.insert("notes", t);
                }
            }
            None => {
                set.insert("notes", Bson::Null);
            }
        }
    }

    coll.update_one(doc! { "_id": id, "owner_id": user.id }, doc! { "$set": set })
        .await?;
    let updated = coll
        .find_one(doc! { "_id": id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Transaction not found".into()))?;
    Ok(Json(transaction_to_response(&updated)))
}

pub async fn delete_transaction(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let id = parse_oid(&id, "transaction id")?;
    let coll = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let result = coll
        .delete_one(doc! { "_id": id, "owner_id": user.id })
        .await?;
    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Transaction not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── CSV import ───────────────────────────────────────────────────────────

/// Max upload size for a CSV file (8 MiB). A typical Sparkasse annual
/// export is well under 1 MiB; the cap is just to bound memory.
const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;

fn schema_to_response(s: &ImportSchema) -> ImportSchemaResponse {
    ImportSchemaResponse {
        id: s.id.to_hex(),
        name: s.name.clone(),
        delimiter: s.delimiter.clone(),
        encoding: s.encoding.clone(),
        decimal_separator: match s.decimal_separator {
            DecimalSeparator::Dot => "dot".into(),
            DecimalSeparator::Comma => "comma".into(),
        },
        skip_header_rows: s.skip_header_rows,
        has_headers: s.has_headers,
        date_column: s.date_column,
        date_format: s.date_format.clone(),
        amount_column: s.amount_column,
        amount_sign_convention: match s.amount_sign_convention {
            AmountSignConvention::PositiveCredit => "positive_credit".into(),
            AmountSignConvention::PositiveDebit => "positive_debit".into(),
        },
        description_columns: s.description_columns.clone(),
        currency_source: match s.currency_source {
            CurrencySource::Column => "column".into(),
            CurrencySource::Fixed => "fixed".into(),
        },
        currency_column: s.currency_column,
        fixed_currency: s.fixed_currency.clone(),
        bank_ref_column: s.bank_ref_column,
        iban_column: s.iban_column,
        raw_category_column: s.raw_category_column,
        is_builtin: s.is_builtin,
        created_at: s.created_at.to_rfc3339(),
        updated_at: s.updated_at.to_rfc3339(),
    }
}

fn parse_decimal_separator(s: &str) -> Result<DecimalSeparator> {
    match s {
        "dot" => Ok(DecimalSeparator::Dot),
        "comma" => Ok(DecimalSeparator::Comma),
        other => Err(AppError::BadRequest(format!(
            "Invalid decimal_separator `{other}` (expected `dot` or `comma`)"
        ))),
    }
}

fn parse_sign_convention(s: &str) -> Result<AmountSignConvention> {
    match s {
        "positive_credit" => Ok(AmountSignConvention::PositiveCredit),
        "positive_debit" => Ok(AmountSignConvention::PositiveDebit),
        other => Err(AppError::BadRequest(format!(
            "Invalid amount_sign_convention `{other}`"
        ))),
    }
}

fn parse_currency_source(s: &str) -> Result<CurrencySource> {
    match s {
        "column" => Ok(CurrencySource::Column),
        "fixed" => Ok(CurrencySource::Fixed),
        other => Err(AppError::BadRequest(format!(
            "Invalid currency_source `{other}` (expected `column` or `fixed`)"
        ))),
    }
}

fn validate_schema_request(req: &ImportSchemaRequest) -> Result<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("Schema name cannot be empty".into()));
    }
    if req.delimiter.len() != 1 {
        return Err(AppError::BadRequest(
            "Delimiter must be a single character".into(),
        ));
    }
    if req.description_columns.is_empty() {
        return Err(AppError::BadRequest(
            "At least one description column is required".into(),
        ));
    }
    let cs = parse_currency_source(&req.currency_source)?;
    match cs {
        CurrencySource::Column if req.currency_column.is_none() => Err(AppError::BadRequest(
            "currency_source=column requires currency_column".into(),
        )),
        CurrencySource::Fixed if req.fixed_currency.is_none() => Err(AppError::BadRequest(
            "currency_source=fixed requires fixed_currency".into(),
        )),
        _ => Ok(()),
    }
}

fn apply_schema_request(
    schema: &mut ImportSchema,
    req: ImportSchemaRequest,
) -> Result<()> {
    schema.name = req.name.trim().to_string();
    schema.delimiter = req.delimiter;
    schema.encoding = req.encoding;
    schema.decimal_separator = parse_decimal_separator(&req.decimal_separator)?;
    schema.skip_header_rows = req.skip_header_rows;
    schema.has_headers = req.has_headers;
    schema.date_column = req.date_column;
    schema.date_format = req.date_format;
    schema.amount_column = req.amount_column;
    schema.amount_sign_convention = parse_sign_convention(&req.amount_sign_convention)?;
    schema.description_columns = req.description_columns;
    schema.currency_source = parse_currency_source(&req.currency_source)?;
    schema.currency_column = req.currency_column;
    schema.fixed_currency = req
        .fixed_currency
        .map(|c| c.trim().to_ascii_uppercase());
    schema.bank_ref_column = req.bank_ref_column;
    schema.iban_column = req.iban_column;
    schema.raw_category_column = req.raw_category_column;
    schema.updated_at = Utc::now();
    Ok(())
}

/// Ensures the user has all builtin schemas seeded. Idempotent: skips
/// any builtin already present for the user.
async fn ensure_builtin_schemas(state: &AppState, owner_id: ObjectId) -> Result<()> {
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    for seed in [sparkasse_camt_v8::seed_for(owner_id)] {
        let builtin_id = seed
            .builtin_id
            .as_ref()
            .expect("builtin seed must set builtin_id");
        let existing = coll
            .find_one(doc! {
                "owner_id": owner_id,
                "builtin_id": builtin_id,
            })
            .await?;
        if existing.is_none() {
            coll.insert_one(&seed).await?;
        }
    }
    Ok(())
}

async fn find_schema(
    state: &AppState,
    owner_id: ObjectId,
    schema_id: ObjectId,
) -> Result<ImportSchema> {
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    coll.find_one(doc! { "_id": schema_id, "owner_id": owner_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Import schema not found".into()))
}

pub async fn list_import_schemas(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ImportSchemaResponse>>> {
    require_finance(&state)?;
    ensure_builtin_schemas(&state, user.id).await?;
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "is_builtin": -1, "name": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(s) = cursor.try_next().await? {
        out.push(schema_to_response(&s));
    }
    Ok(Json(out))
}

pub async fn create_import_schema(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<ImportSchemaRequest>,
) -> Result<(StatusCode, Json<ImportSchemaResponse>)> {
    require_finance(&state)?;
    validate_schema_request(&req)?;
    let now = Utc::now();
    let mut schema = ImportSchema {
        id: ObjectId::new(),
        owner_id: user.id,
        name: String::new(),
        delimiter: String::new(),
        encoding: String::new(),
        decimal_separator: DecimalSeparator::Dot,
        skip_header_rows: 0,
        has_headers: true,
        date_column: 0,
        date_format: String::new(),
        amount_column: 0,
        amount_sign_convention: AmountSignConvention::PositiveCredit,
        description_columns: Vec::new(),
        currency_source: CurrencySource::Fixed,
        currency_column: None,
        fixed_currency: None,
        bank_ref_column: None,
        iban_column: None,
        raw_category_column: None,
        is_builtin: false,
        builtin_id: None,
        created_at: now,
        updated_at: now,
    };
    apply_schema_request(&mut schema, req)?;
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    coll.insert_one(&schema).await?;
    Ok((StatusCode::CREATED, Json(schema_to_response(&schema))))
}

pub async fn update_import_schema(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ImportSchemaRequest>,
) -> Result<Json<ImportSchemaResponse>> {
    require_finance(&state)?;
    let schema_oid = parse_oid(&id, "schema id")?;
    let mut schema = find_schema(&state, user.id, schema_oid).await?;
    if schema.is_builtin {
        return Err(AppError::BadRequest(
            "Built-in schemas cannot be edited; clone first".into(),
        ));
    }
    validate_schema_request(&req)?;
    apply_schema_request(&mut schema, req)?;
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    coll.replace_one(doc! { "_id": schema.id }, &schema).await?;
    Ok(Json(schema_to_response(&schema)))
}

pub async fn delete_import_schema(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let schema_oid = parse_oid(&id, "schema id")?;
    let schema = find_schema(&state, user.id, schema_oid).await?;
    if schema.is_builtin {
        return Err(AppError::BadRequest(
            "Built-in schemas cannot be deleted".into(),
        ));
    }
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    coll.delete_one(doc! { "_id": schema.id }).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn clone_import_schema(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ImportSchemaResponse>)> {
    require_finance(&state)?;
    let schema_oid = parse_oid(&id, "schema id")?;
    let source = find_schema(&state, user.id, schema_oid).await?;
    let now = Utc::now();
    let clone = ImportSchema {
        id: ObjectId::new(),
        owner_id: user.id,
        name: format!("{} (copy)", source.name),
        delimiter: source.delimiter,
        encoding: source.encoding,
        decimal_separator: source.decimal_separator,
        skip_header_rows: source.skip_header_rows,
        has_headers: source.has_headers,
        date_column: source.date_column,
        date_format: source.date_format,
        amount_column: source.amount_column,
        amount_sign_convention: source.amount_sign_convention,
        description_columns: source.description_columns,
        currency_source: source.currency_source,
        currency_column: source.currency_column,
        fixed_currency: source.fixed_currency,
        bank_ref_column: source.bank_ref_column,
        iban_column: source.iban_column,
        raw_category_column: source.raw_category_column,
        is_builtin: false,
        builtin_id: None,
        created_at: now,
        updated_at: now,
    };
    let coll = state.db.collection::<ImportSchema>(IMPORT_SCHEMAS);
    coll.insert_one(&clone).await?;
    Ok((StatusCode::CREATED, Json(schema_to_response(&clone))))
}

pub async fn import_csv(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<ImportCsvResponse>> {
    require_finance(&state)?;

    let mut account_id_str: Option<String> = None;
    let mut schema_id_str: Option<String> = None;
    let mut csv_bytes: Option<Vec<u8>> = None;
    let mut csv_filename: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name().unwrap_or("") {
            "account_id" => {
                account_id_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Bad account_id: {e}")))?,
                );
            }
            "schema_id" => {
                schema_id_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Bad schema_id: {e}")))?,
                );
            }
            "csv" => {
                csv_filename = field.file_name().map(|s| s.to_string());
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read CSV: {e}")))?;
                if bytes.len() > MAX_IMPORT_BYTES {
                    return Err(AppError::BadRequest(format!(
                        "CSV exceeds {MAX_IMPORT_BYTES}-byte limit"
                    )));
                }
                csv_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    let schema_id_str = schema_id_str
        .ok_or_else(|| AppError::BadRequest("Missing schema_id field".into()))?;
    let csv_bytes = csv_bytes
        .ok_or_else(|| AppError::BadRequest("Missing csv field".into()))?;

    let schema_oid = parse_oid(&schema_id_str, "schema_id")?;
    let schema = find_schema(&state, user.id, schema_oid).await?;

    let parsed = finance_import::parse_csv(&csv_bytes, &schema).map_err(|e| match e {
        ParseError::Fatal(msg) => AppError::BadRequest(msg),
        ParseError::Row { line, message } => {
            // A row error at the top level shouldn't happen, but if it
            // does, surface it as 400.
            AppError::BadRequest(format!("line {line}: {message}"))
        }
    })?;

    // First successful parse — used both for IBAN-based account lookup
    // and for picking the auto-created account's currency. Done with a
    // shallow scan rather than collecting all rows into memory.
    let first_ok = parsed.iter().find_map(|r| r.as_ref().ok());

    let (account, auto_created) =
        resolve_import_account(&state, &user, account_id_str.as_deref(), &schema, first_ok).await?;

    let rules = load_user_rules(&state, user.id).await?;
    let (rule_engine, _rule_errs) = RuleEngine::build(&rules);

    let run_id = ObjectId::new();
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let mut imported = 0u32;
    let mut skipped = 0u32;
    let mut errors = 0u32;
    let mut error_details: Vec<ImportRowError> = Vec::new();

    for (idx, row) in parsed.into_iter().enumerate() {
        let row: ParsedRow = match row {
            Ok(r) => r,
            Err(ParseError::Row { line, message }) => {
                errors += 1;
                if error_details.len() < 50 {
                    error_details.push(ImportRowError { line, message });
                }
                continue;
            }
            Err(ParseError::Fatal(message)) => {
                errors += 1;
                if error_details.len() < 50 {
                    error_details.push(ImportRowError {
                        line: (idx as u32) + 2,
                        message,
                    });
                }
                continue;
            }
        };

        let rule_match = rule_engine
            .match_first(&row.description)
            .map(|idx| (rules[idx].id, rules[idx].category_id));
        match insert_imported_row(&txns, user.id, &account, row, run_id, rule_match).await {
            Ok(InsertOutcome::Inserted) => imported += 1,
            Ok(InsertOutcome::Skipped) => skipped += 1,
            Err(e) => {
                errors += 1;
                if error_details.len() < 50 {
                    error_details.push(ImportRowError {
                        line: (idx as u32) + 2,
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(&csv_bytes);
    let sha256 = hex::encode(hasher.finalize());

    let now = Utc::now();
    let run = ImportRun {
        id: run_id,
        owner_id: user.id,
        account_id: account.id,
        schema_id: schema.id,
        source: ImportSource {
            kind: ImportSourceKind::Upload,
            filename: csv_filename.unwrap_or_else(|| "import.csv".to_string()),
            size_bytes: csv_bytes.len() as u64,
            sha256,
            uncloud_file_id: None,
        },
        status: ImportRunStatus::Applied,
        summary: ImportRunSummary {
            created: imported,
            skipped_duplicate: skipped,
            errored: errors,
        },
        errors: error_details
            .iter()
            .map(|e| ModelImportRunError {
                line: e.line,
                message: e.message.clone(),
            })
            .collect(),
        created_at: now,
        reverted_at: None,
    };
    state
        .db
        .collection::<ImportRun>(IMPORT_RUNS)
        .insert_one(&run)
        .await?;

    Ok(Json(ImportCsvResponse {
        run_id: run_id.to_hex(),
        account_id: account.id.to_hex(),
        auto_created_account: if auto_created {
            Some(account_to_response(&account))
        } else {
            None
        },
        imported,
        skipped,
        errors,
        error_details,
    }))
}

enum InsertOutcome {
    Inserted,
    Skipped,
}

async fn insert_imported_row(
    txns: &mongodb::Collection<FinanceTransaction>,
    owner_id: ObjectId,
    account: &FinanceAccount,
    row: ParsedRow,
    run_id: ObjectId,
    rule_match: Option<(ObjectId, ObjectId)>,
) -> std::result::Result<InsertOutcome, AppError> {
    let now = Utc::now();
    let (cat_id, cat_source, rule_id) = match rule_match {
        Some((rid, cid)) => (Some(cid), CategorySource::Rule, Some(rid)),
        None => (None, CategorySource::Unset, None),
    };
    let leg = TransactionLeg {
        amount_minor: row.amount_minor,
        category_id: cat_id,
        category_source: cat_source,
        rule_id,
        note: None,
    };
    let txn = FinanceTransaction {
        id: ObjectId::new(),
        owner_id,
        account_id: account.id,
        currency: row.currency,
        amount_minor: row.amount_minor,
        description: row.description,
        date: row.date,
        source_ref: Some(row.source_ref),
        raw_bank_category: row.raw_bank_category,
        notes: None,
        tags: vec![],
        legs: vec![leg],
        import_run_id: Some(run_id),
        source_snapshot_id: None,
        created_at: now,
        updated_at: now,
    };

    match txns.insert_one(&txn).await {
        Ok(_) => Ok(InsertOutcome::Inserted),
        Err(e) => {
            if is_duplicate_key_error(&e) {
                Ok(InsertOutcome::Skipped)
            } else {
                Err(AppError::from(e))
            }
        }
    }
}

fn is_duplicate_key_error(err: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    match err.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(we)) => we.code == 11000,
        _ => false,
    }
}

/// Picks the target account for an import:
///   1. `account_id` field wins if provided (manual override).
///   2. Otherwise, if the schema has `iban_column` and the first row
///      carries an IBAN, look up an existing account by IBAN.
///   3. Otherwise create one. Currency comes from the first row.
///
/// Returns `(account, was_auto_created)`.
async fn resolve_import_account(
    state: &AppState,
    user: &AuthUser,
    account_id_str: Option<&str>,
    schema: &ImportSchema,
    first_row: Option<&ParsedRow>,
) -> Result<(FinanceAccount, bool)> {
    if let Some(id) = account_id_str.filter(|s| !s.is_empty()) {
        let oid = parse_oid(id, "account_id")?;
        return Ok((find_account(state, user.id, oid).await?, false));
    }

    if schema.iban_column.is_none() {
        return Err(AppError::BadRequest(
            "account_id is required when the schema does not declare an IBAN column".into(),
        ));
    }
    let Some(row) = first_row else {
        return Err(AppError::BadRequest(
            "CSV has no parseable rows — cannot auto-create an account".into(),
        ));
    };
    let Some(raw_iban) = row.iban.as_deref().and_then(normalize_iban) else {
        return Err(AppError::BadRequest(
            "First row has no IBAN; cannot auto-create an account. \
             Either fix the IBAN column in the schema or pick an account manually."
                .into(),
        ));
    };

    let coll = state.db.collection::<FinanceAccount>(ACCOUNTS);
    if let Some(existing) = coll
        .find_one(doc! { "owner_id": user.id, "iban": &raw_iban })
        .await?
    {
        return Ok((existing, false));
    }

    let display_iban = raw_iban
        .get(raw_iban.len().saturating_sub(4)..)
        .unwrap_or(raw_iban.as_str());
    let now = Utc::now();
    let account = FinanceAccount {
        id: ObjectId::new(),
        owner_id: user.id,
        name: format!("Account •••{display_iban}"),
        account_type: "checking".into(),
        currency: row.currency.clone(),
        opening_balance_minor: 0,
        iban: Some(raw_iban),
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    coll.insert_one(&account).await.map_err(|e| {
        if is_duplicate_key_error(&e) {
            // A concurrent import just won the race — best-effort: refetch.
            AppError::Internal("Account auto-create raced; please retry".into())
        } else {
            AppError::from(e)
        }
    })?;
    Ok((account, true))
}

fn run_to_response(r: &ImportRun) -> ImportRunResponse {
    ImportRunResponse {
        id: r.id.to_hex(),
        account_id: r.account_id.to_hex(),
        schema_id: r.schema_id.to_hex(),
        source: ImportRunSourceDto {
            kind: match r.source.kind {
                ImportSourceKind::Upload => "upload".into(),
                ImportSourceKind::UncloudFile => "uncloud_file".into(),
            },
            filename: r.source.filename.clone(),
            size_bytes: r.source.size_bytes,
            sha256: r.source.sha256.clone(),
            uncloud_file_id: r.source.uncloud_file_id.map(|id| id.to_hex()),
        },
        status: match r.status {
            ImportRunStatus::Applied => "applied".into(),
            ImportRunStatus::Reverted => "reverted".into(),
        },
        summary: ImportRunSummaryDto {
            created: r.summary.created,
            skipped_duplicate: r.summary.skipped_duplicate,
            errored: r.summary.errored,
        },
        errors: r
            .errors
            .iter()
            .map(|e| ImportRowError {
                line: e.line,
                message: e.message.clone(),
            })
            .collect(),
        created_at: r.created_at.to_rfc3339(),
        reverted_at: r.reverted_at.map(|d| d.to_rfc3339()),
    }
}

pub async fn list_import_runs(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ImportRunResponse>>> {
    require_finance(&state)?;
    let coll = state.db.collection::<ImportRun>(IMPORT_RUNS);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "created_at": -1 })
        .await?;
    let mut out = Vec::new();
    while let Some(r) = cursor.try_next().await? {
        out.push(run_to_response(&r));
    }
    Ok(Json(out))
}

pub async fn revert_import_run(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ImportRunResponse>> {
    require_finance(&state)?;
    let run_oid = parse_oid(&id, "run id")?;
    let runs = state.db.collection::<ImportRun>(IMPORT_RUNS);
    let mut run = runs
        .find_one(doc! { "_id": run_oid, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Import run not found".into()))?;

    if matches!(run.status, ImportRunStatus::Reverted) {
        return Err(AppError::BadRequest("Run already reverted".into()));
    }

    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    txns.delete_many(doc! {
        "owner_id": user.id,
        "import_run_id": run_oid,
    })
    .await?;

    let now = Utc::now();
    run.status = ImportRunStatus::Reverted;
    run.reverted_at = Some(now);
    runs.replace_one(doc! { "_id": run_oid }, &run).await?;

    Ok(Json(run_to_response(&run)))
}

// ── Reconciliation ───────────────────────────────────────────────────────

fn parse_iso_date(s: &str) -> Result<DateTime<Utc>> {
    let nd = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest(format!("Invalid date `{s}`, expected YYYY-MM-DD")))?;
    Ok(Utc.from_utc_datetime(&nd.and_hms_opt(0, 0, 0).unwrap()))
}

/// Sum of `amount_minor` for the account's transactions on or before
/// `on_date`, optionally excluding one transaction id (used when
/// recomputing a snapshot's own adjustment).
async fn sum_transactions_through(
    state: &AppState,
    owner_id: ObjectId,
    account_id: ObjectId,
    on_date: DateTime<Utc>,
    exclude_id: Option<ObjectId>,
) -> Result<i64> {
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let mut match_doc = doc! {
        "owner_id": owner_id,
        "account_id": account_id,
        // End-of-day is captured by the strict < next-day bound below;
        // we store dates at UTC midnight, so this catches everything
        // booked on `on_date` itself too.
        "date": { "$lte": bson::DateTime::from_chrono(on_date + chrono::Duration::days(1) - chrono::Duration::milliseconds(1)) },
    };
    if let Some(skip) = exclude_id {
        match_doc.insert("_id", doc! { "$ne": skip });
    }
    let pipeline = vec![
        doc! { "$match": match_doc },
        doc! { "$group": { "_id": null, "sum": { "$sum": "$amount_minor" } } },
    ];
    let mut cursor = txns.aggregate(pipeline).await?;
    let sum = if let Some(d) = cursor.try_next().await? {
        d.get_i64("sum").unwrap_or(0)
    } else {
        0
    };
    Ok(sum)
}

async fn ensure_reconciliation_category(
    state: &AppState,
    owner_id: ObjectId,
) -> Result<ObjectId> {
    let cats = state.db.collection::<FinanceCategory>(CATEGORIES);
    if let Some(existing) = cats
        .find_one(doc! { "owner_id": owner_id, "name": RECONCILIATION_CATEGORY })
        .await?
    {
        return Ok(existing.id);
    }
    let now = Utc::now();
    let cat = FinanceCategory {
        id: ObjectId::new(),
        owner_id,
        parent_id: None,
        name: RECONCILIATION_CATEGORY.into(),
        colour: Some("#888888".into()),
        created_at: now,
    };
    match cats.insert_one(&cat).await {
        Ok(_) => Ok(cat.id),
        Err(e) if is_duplicate_key_error(&e) => {
            // Lost the race with a concurrent reconcile; refetch.
            cats.find_one(doc! { "owner_id": owner_id, "name": RECONCILIATION_CATEGORY })
                .await?
                .map(|c| c.id)
                .ok_or_else(|| AppError::Internal("Reconciliation category vanished".into()))
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn reconcile_preview(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ReconcileRequest>,
) -> Result<Json<ReconcilePreviewResponse>> {
    require_finance(&state)?;
    let account_oid = parse_oid(&id, "account id")?;
    let account = find_account(&state, user.id, account_oid).await?;
    let on_date = parse_iso_date(&req.on_date)?;

    let txn_sum = sum_transactions_through(&state, user.id, account.id, on_date, None).await?;
    let computed = account.opening_balance_minor + txn_sum;

    Ok(Json(ReconcilePreviewResponse {
        on_date: req.on_date,
        computed_minor: computed,
        actual_minor: req.actual_balance_minor,
        delta_minor: req.actual_balance_minor - computed,
    }))
}

pub async fn reconcile_apply(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ReconcileRequest>,
) -> Result<(StatusCode, Json<BalanceSnapshotResponse>)> {
    require_finance(&state)?;
    let account_oid = parse_oid(&id, "account id")?;
    let account = find_account(&state, user.id, account_oid).await?;
    let on_date = parse_iso_date(&req.on_date)?;

    let txn_sum = sum_transactions_through(&state, user.id, account.id, on_date, None).await?;
    let computed = account.opening_balance_minor + txn_sum;
    let delta = req.actual_balance_minor - computed;

    let note = req.note.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let category_id = ensure_reconciliation_category(&state, user.id).await?;

    let now = Utc::now();
    let snapshot_id = ObjectId::new();
    let adj_id = ObjectId::new();
    let description = match note.as_deref() {
        Some(n) => format!("Reconciliation — {n}"),
        None => "Reconciliation".to_string(),
    };

    let leg = TransactionLeg {
        amount_minor: delta,
        category_id: Some(category_id),
        category_source: CategorySource::User,
        rule_id: None,
        note: None,
    };
    let adjustment = FinanceTransaction {
        id: adj_id,
        owner_id: user.id,
        account_id: account.id,
        currency: account.currency.clone(),
        amount_minor: delta,
        description,
        date: on_date,
        source_ref: None,
        raw_bank_category: None,
        notes: note.clone(),
        tags: vec![],
        legs: vec![leg],
        import_run_id: None,
        source_snapshot_id: Some(snapshot_id),
        created_at: now,
        updated_at: now,
    };

    let snapshot = BalanceSnapshot {
        id: snapshot_id,
        owner_id: user.id,
        account_id: account.id,
        on_date,
        actual_balance_minor: req.actual_balance_minor,
        note,
        adjustment_transaction_id: adj_id,
        created_at: now,
    };

    state
        .db
        .collection::<FinanceTransaction>(TRANSACTIONS)
        .insert_one(&adjustment)
        .await?;
    state
        .db
        .collection::<BalanceSnapshot>(BALANCE_SNAPSHOTS)
        .insert_one(&snapshot)
        .await?;

    Ok((StatusCode::CREATED, Json(snapshot_to_response(&snapshot, 0))))
}

fn snapshot_to_response(s: &BalanceSnapshot, drift_minor: i64) -> BalanceSnapshotResponse {
    BalanceSnapshotResponse {
        id: s.id.to_hex(),
        account_id: s.account_id.to_hex(),
        on_date: s.on_date.format("%Y-%m-%d").to_string(),
        actual_balance_minor: s.actual_balance_minor,
        note: s.note.clone(),
        adjustment_transaction_id: s.adjustment_transaction_id.to_hex(),
        created_at: s.created_at.to_rfc3339(),
        drift_minor,
    }
}

async fn snapshot_drift(state: &AppState, snapshot: &BalanceSnapshot) -> Result<i64> {
    let account = find_account(&state, snapshot.owner_id, snapshot.account_id).await?;
    let txn_sum = sum_transactions_through(
        &state,
        snapshot.owner_id,
        snapshot.account_id,
        snapshot.on_date,
        Some(snapshot.adjustment_transaction_id),
    )
    .await?;
    let computed_without_adj = account.opening_balance_minor + txn_sum;
    // If the linked adjustment had value `actual - computed_without_adj`,
    // then drift = actual - (computed_without_adj + current_adj). We
    // expect that to be 0; non-zero means late activity changed history.
    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let adj = txns
        .find_one(doc! { "_id": snapshot.adjustment_transaction_id })
        .await?
        .map(|t| t.amount_minor)
        .unwrap_or(0);
    Ok(snapshot.actual_balance_minor - (computed_without_adj + adj))
}

pub async fn list_account_snapshots(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<BalanceSnapshotResponse>>> {
    require_finance(&state)?;
    let account_oid = parse_oid(&id, "account id")?;
    let _ = find_account(&state, user.id, account_oid).await?;
    let coll = state.db.collection::<BalanceSnapshot>(BALANCE_SNAPSHOTS);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id, "account_id": account_oid })
        .sort(doc! { "on_date": -1 })
        .await?;
    let mut out = Vec::new();
    while let Some(s) = cursor.try_next().await? {
        let drift = snapshot_drift(&state, &s).await?;
        out.push(snapshot_to_response(&s, drift));
    }
    Ok(Json(out))
}

pub async fn recompute_snapshot(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<BalanceSnapshotResponse>> {
    require_finance(&state)?;
    let snap_oid = parse_oid(&id, "snapshot id")?;
    let snapshots = state.db.collection::<BalanceSnapshot>(BALANCE_SNAPSHOTS);
    let snapshot = snapshots
        .find_one(doc! { "_id": snap_oid, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Snapshot not found".into()))?;
    let account = find_account(&state, user.id, snapshot.account_id).await?;

    let txn_sum = sum_transactions_through(
        &state,
        user.id,
        snapshot.account_id,
        snapshot.on_date,
        Some(snapshot.adjustment_transaction_id),
    )
    .await?;
    let new_delta = snapshot.actual_balance_minor - (account.opening_balance_minor + txn_sum);
    let now = Utc::now();

    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    // Adjustment's first leg follows the new delta; user-set category stays.
    txns.update_one(
        doc! { "_id": snapshot.adjustment_transaction_id, "owner_id": user.id },
        doc! { "$set": {
            "amount_minor": new_delta,
            "legs.0.amount_minor": new_delta,
            "updated_at": bson::DateTime::from_chrono(now),
        } },
    )
    .await?;

    Ok(Json(snapshot_to_response(&snapshot, 0)))
}

pub async fn delete_snapshot(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let snap_oid = parse_oid(&id, "snapshot id")?;
    let snapshots = state.db.collection::<BalanceSnapshot>(BALANCE_SNAPSHOTS);
    let snapshot = snapshots
        .find_one(doc! { "_id": snap_oid, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Snapshot not found".into()))?;

    state
        .db
        .collection::<FinanceTransaction>(TRANSACTIONS)
        .delete_one(doc! {
            "_id": snapshot.adjustment_transaction_id,
            "owner_id": user.id,
        })
        .await?;
    snapshots.delete_one(doc! { "_id": snap_oid }).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Categorization rules ────────────────────────────────────────────────

fn parse_rule_pattern_kind(s: &str) -> Result<RulePatternKind> {
    match s {
        "substring" => Ok(RulePatternKind::Substring),
        "starts_with" => Ok(RulePatternKind::StartsWith),
        "regex" => Ok(RulePatternKind::Regex),
        other => Err(AppError::BadRequest(format!(
            "Invalid pattern_kind `{other}` (expected substring | starts_with | regex)"
        ))),
    }
}

fn rule_kind_str(k: RulePatternKind) -> &'static str {
    match k {
        RulePatternKind::Substring => "substring",
        RulePatternKind::StartsWith => "starts_with",
        RulePatternKind::Regex => "regex",
    }
}

fn rule_to_response(r: &FinanceRule) -> FinanceRuleResponse {
    FinanceRuleResponse {
        id: r.id.to_hex(),
        name: r.name.clone(),
        pattern: r.pattern.clone(),
        pattern_kind: rule_kind_str(r.pattern_kind).to_string(),
        case_insensitive: r.case_insensitive,
        category_id: r.category_id.to_hex(),
        priority: r.priority,
        enabled: r.enabled,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

fn validate_rule_request(req: &FinanceRuleRequest) -> Result<()> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("Rule name is required".into()));
    }
    if req.pattern.trim().is_empty() {
        return Err(AppError::BadRequest("Rule pattern is required".into()));
    }
    let kind = parse_rule_pattern_kind(&req.pattern_kind)?;
    // Compile-validate to reject bad regexes at write time.
    finance_rules::compile_pattern(&req.pattern, kind, req.case_insensitive)
        .map_err(|e| AppError::BadRequest(format!("Invalid pattern: {e}")))?;
    Ok(())
}

pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<FinanceRuleResponse>>> {
    require_finance(&state)?;
    let coll = state.db.collection::<FinanceRule>(RULES);
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .sort(doc! { "priority": 1, "_id": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(r) = cursor.try_next().await? {
        out.push(rule_to_response(&r));
    }
    Ok(Json(out))
}

async fn validate_rule_category(
    state: &AppState,
    owner_id: ObjectId,
    category_id_str: &str,
) -> Result<ObjectId> {
    let oid = parse_oid(category_id_str, "category_id")?;
    let exists = state
        .db
        .collection::<FinanceCategory>(CATEGORIES)
        .find_one(doc! { "_id": oid, "owner_id": owner_id })
        .await?
        .is_some();
    if !exists {
        return Err(AppError::BadRequest("Category not found".into()));
    }
    Ok(oid)
}

pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<FinanceRuleRequest>,
) -> Result<(StatusCode, Json<FinanceRuleResponse>)> {
    require_finance(&state)?;
    validate_rule_request(&req)?;
    let kind = parse_rule_pattern_kind(&req.pattern_kind)?;
    let category_id = validate_rule_category(&state, user.id, &req.category_id).await?;
    let now = Utc::now();
    let rule = FinanceRule {
        id: ObjectId::new(),
        owner_id: user.id,
        name: req.name.trim().to_string(),
        pattern: req.pattern,
        pattern_kind: kind,
        case_insensitive: req.case_insensitive,
        category_id,
        priority: req.priority,
        enabled: req.enabled,
        created_at: now,
        updated_at: now,
    };
    state.db.collection::<FinanceRule>(RULES).insert_one(&rule).await?;
    Ok((StatusCode::CREATED, Json(rule_to_response(&rule))))
}

pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<FinanceRuleRequest>,
) -> Result<Json<FinanceRuleResponse>> {
    require_finance(&state)?;
    validate_rule_request(&req)?;
    let kind = parse_rule_pattern_kind(&req.pattern_kind)?;
    let oid = parse_oid(&id, "rule id")?;
    let category_id = validate_rule_category(&state, user.id, &req.category_id).await?;

    let coll = state.db.collection::<FinanceRule>(RULES);
    let mut existing = coll
        .find_one(doc! { "_id": oid, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Rule not found".into()))?;

    existing.name = req.name.trim().to_string();
    existing.pattern = req.pattern;
    existing.pattern_kind = kind;
    existing.case_insensitive = req.case_insensitive;
    existing.category_id = category_id;
    existing.priority = req.priority;
    existing.enabled = req.enabled;
    existing.updated_at = Utc::now();

    coll.replace_one(doc! { "_id": oid }, &existing).await?;
    Ok(Json(rule_to_response(&existing)))
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_finance(&state)?;
    let oid = parse_oid(&id, "rule id")?;
    let r = state
        .db
        .collection::<FinanceRule>(RULES)
        .delete_one(doc! { "_id": oid, "owner_id": user.id })
        .await?;
    if r.deleted_count == 0 {
        return Err(AppError::NotFound("Rule not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn load_user_rules(state: &AppState, owner_id: ObjectId) -> Result<Vec<FinanceRule>> {
    let coll = state.db.collection::<FinanceRule>(RULES);
    let mut cursor = coll
        .find(doc! { "owner_id": owner_id })
        .sort(doc! { "priority": 1, "_id": 1 })
        .await?;
    let mut out = Vec::new();
    while let Some(r) = cursor.try_next().await? {
        out.push(r);
    }
    Ok(out)
}

pub async fn apply_rules(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<ApplyRulesResponse>> {
    require_finance(&state)?;
    let rules = load_user_rules(&state, user.id).await?;
    let (engine, _errs) = RuleEngine::build(&rules);

    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    // Only legs[0] is currently set by the importer; split transactions
    // are deferred. We touch legs[0] only when its category_source is
    // not 'user' (i.e. unset or previously rule-set).
    let mut cursor = txns
        .find(doc! {
            "owner_id": user.id,
            "$or": [
                { "legs.0.category_source": "unset" },
                { "legs.0.category_source": "rule" },
            ],
        })
        .await?;

    let mut updated = 0u32;
    let mut still_unmatched = 0u32;
    while let Some(t) = cursor.try_next().await? {
        match engine.match_first(&t.description) {
            Some(idx) => {
                let rule = &rules[idx];
                txns.update_one(
                    doc! { "_id": t.id },
                    doc! { "$set": {
                        "legs.0.category_id": rule.category_id,
                        "legs.0.category_source": "rule",
                        "legs.0.rule_id": rule.id,
                        "updated_at": bson::DateTime::from_chrono(Utc::now()),
                    } },
                )
                .await?;
                updated += 1;
            }
            None => {
                // No match — if we previously rule-tagged this leg,
                // clear the tag so it doesn't pretend a rule still owns it.
                if matches!(t.legs.first().map(|l| l.category_source), Some(CategorySource::Rule)) {
                    txns.update_one(
                        doc! { "_id": t.id },
                        doc! { "$set": {
                            "legs.0.category_id": Bson::Null,
                            "legs.0.category_source": "unset",
                            "legs.0.rule_id": Bson::Null,
                            "updated_at": bson::DateTime::from_chrono(Utc::now()),
                        } },
                    )
                    .await?;
                }
                still_unmatched += 1;
            }
        }
    }

    Ok(Json(ApplyRulesResponse {
        updated,
        still_unmatched,
    }))
}

pub async fn test_rule(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<TestRuleRequest>,
) -> Result<Json<TestRuleResponse>> {
    require_finance(&state)?;
    let kind = parse_rule_pattern_kind(&req.pattern_kind)?;
    let matcher = finance_rules::compile_pattern(&req.pattern, kind, req.case_insensitive)
        .map_err(|e| AppError::BadRequest(format!("Invalid pattern: {e}")))?;

    let txns = state.db.collection::<FinanceTransaction>(TRANSACTIONS);
    let opts = FindOptions::builder()
        .sort(doc! { "date": -1 })
        .limit(200i64)
        .build();
    let mut cursor = txns
        .find(doc! { "owner_id": user.id })
        .with_options(opts)
        .await?;

    let mut matches = Vec::new();
    let mut sampled = 0u32;
    while let Some(t) = cursor.try_next().await? {
        sampled += 1;
        if matcher.matches(&t.description) && matches.len() < 50 {
            matches.push(TestRuleMatch {
                transaction_id: t.id.to_hex(),
                date: t.date.to_rfc3339(),
                description: t.description.clone(),
                amount_minor: t.amount_minor,
            });
        }
    }

    Ok(Json(TestRuleResponse { sampled, matches }))
}
