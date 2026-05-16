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

// ── CSV import ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRowError {
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportCsvResponse {
    pub run_id: String,
    pub imported: u32,
    pub skipped: u32,
    pub errors: u32,
    pub error_details: Vec<ImportRowError>,
}

// ── Import runs (history) ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRunSummaryDto {
    pub created: u32,
    pub skipped_duplicate: u32,
    pub errored: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRunSourceDto {
    /// "upload" or "uncloud_file".
    pub kind: String,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub uncloud_file_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRunResponse {
    pub id: String,
    pub account_id: String,
    pub schema_id: String,
    pub source: ImportRunSourceDto,
    /// "applied" or "reverted".
    pub status: String,
    pub summary: ImportRunSummaryDto,
    pub errors: Vec<ImportRowError>,
    pub created_at: String,
    pub reverted_at: Option<String>,
}

// ── Import schemas ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportSchemaResponse {
    pub id: String,
    pub name: String,
    pub delimiter: String,
    pub encoding: String,
    pub decimal_separator: String,
    pub skip_header_rows: u32,
    pub has_headers: bool,
    pub date_column: u32,
    pub date_format: String,
    pub amount_column: u32,
    pub amount_sign_convention: String,
    pub description_columns: Vec<u32>,
    pub currency_source: String,
    pub currency_column: Option<u32>,
    pub fixed_currency: Option<String>,
    pub bank_ref_column: Option<u32>,
    pub iban_column: Option<u32>,
    pub raw_category_column: Option<u32>,
    pub is_builtin: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportSchemaRequest {
    pub name: String,
    pub delimiter: String,
    pub encoding: String,
    /// "dot" or "comma".
    pub decimal_separator: String,
    #[serde(default)]
    pub skip_header_rows: u32,
    #[serde(default = "default_true_serde")]
    pub has_headers: bool,
    pub date_column: u32,
    pub date_format: String,
    pub amount_column: u32,
    /// "positive_credit" or "positive_debit".
    pub amount_sign_convention: String,
    pub description_columns: Vec<u32>,
    /// "column" or "fixed".
    pub currency_source: String,
    #[serde(default)]
    pub currency_column: Option<u32>,
    #[serde(default)]
    pub fixed_currency: Option<String>,
    #[serde(default)]
    pub bank_ref_column: Option<u32>,
    #[serde(default)]
    pub iban_column: Option<u32>,
    #[serde(default)]
    pub raw_category_column: Option<u32>,
}

fn default_true_serde() -> bool {
    true
}
