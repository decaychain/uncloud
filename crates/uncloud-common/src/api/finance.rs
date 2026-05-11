use serde::{Deserialize, Serialize};

// ── Accounts ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountResponse {
    pub id: String,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub opening_balance_minor: i64,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountBalanceResponse {
    pub account_id: String,
    pub currency: String,
    pub opening_balance_minor: i64,
    pub transaction_total_minor: i64,
    pub balance_minor: i64,
    pub transaction_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateAccountRequest {
    pub name: String,
    pub account_type: String,
    pub currency: String,
    #[serde(default)]
    pub opening_balance_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAccountRequest {
    pub name: Option<String>,
    pub account_type: Option<String>,
    pub opening_balance_minor: Option<i64>,
    pub archived: Option<bool>,
}

// ── Categories ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceCategoryResponse {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub colour: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFinanceCategoryRequest {
    pub name: String,
    pub parent_id: Option<String>,
    pub colour: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateFinanceCategoryRequest {
    pub name: Option<String>,
    pub parent_id: Option<String>,
    pub colour: Option<String>,
}

// ── Transactions ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub id: String,
    pub account_id: String,
    pub date: String,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub category_id: Option<String>,
    pub notes: Option<String>,
    pub source_ref: Option<String>,
    pub raw_bank_category: Option<String>,
    pub is_split: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTransactionRequest {
    pub account_id: String,
    pub date: String,
    pub amount_minor: i64,
    pub description: String,
    pub category_id: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateTransactionRequest {
    pub date: Option<String>,
    pub amount_minor: Option<i64>,
    pub description: Option<String>,
    pub category_id: Option<Option<String>>,
    pub notes: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListTransactionsQuery {
    pub account_id: Option<String>,
    pub category_id: Option<String>,
    pub uncategorized: Option<bool>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub skip: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionListResponse {
    pub items: Vec<TransactionResponse>,
    pub total: u64,
}
