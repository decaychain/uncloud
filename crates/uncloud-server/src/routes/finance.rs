//! Finance tracker — accounts, categories, transactions (manual entry).
//!
//! Foundation slice. CSV import lands in a follow-up, expected to reshape
//! the transaction model around `source_ref` upserts; existing fields are
//! preserved through that work.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::{DateTime, TimeZone, Utc};
use futures::TryStreamExt;
use mongodb::bson::oid::ObjectId;
use mongodb::bson::Bson;
use mongodb::options::FindOptions;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{
    CategorySource, FinanceAccount, FinanceCategory, FinanceTransaction, TransactionLeg,
};
use crate::AppState;
use uncloud_common::{
    AccountBalanceResponse, AccountResponse, CreateAccountRequest, CreateFinanceCategoryRequest,
    CreateTransactionRequest, FinanceCategoryResponse, ListTransactionsQuery,
    TransactionListResponse, TransactionResponse, UpdateAccountRequest,
    UpdateFinanceCategoryRequest, UpdateTransactionRequest,
};

const ACCOUNTS: &str = "finance_accounts";
const CATEGORIES: &str = "finance_categories";
const TRANSACTIONS: &str = "finance_transactions";

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
        created_at: a.created_at.to_rfc3339(),
        updated_at: a.updated_at.to_rfc3339(),
        archived_at: a.archived_at.map(|d| d.to_rfc3339()),
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
    let now = Utc::now();
    let account = FinanceAccount {
        id: ObjectId::new(),
        owner_id: user.id,
        name,
        account_type: req.account_type.trim().to_string(),
        currency,
        opening_balance_minor: req.opening_balance_minor,
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    state
        .db
        .collection::<FinanceAccount>(ACCOUNTS)
        .insert_one(&account)
        .await?;
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

pub async fn list_transactions(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ListTransactionsQuery>,
) -> Result<Json<TransactionListResponse>> {
    require_finance(&state)?;
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
