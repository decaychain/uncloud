use serde::{Deserialize, Serialize};

// ── Accounts ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountResponse {
    pub id: String,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub opening_balance_minor: i64,
    pub iban: Option<String>,
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
    #[serde(default)]
    pub iban: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateAccountRequest {
    pub name: Option<String>,
    pub account_type: Option<String>,
    pub opening_balance_minor: Option<i64>,
    pub archived: Option<bool>,
    /// Two-level Option so PATCH can either leave IBAN alone (`None`),
    /// clear it (`Some(None)`), or set a new value (`Some(Some(_))`).
    pub iban: Option<Option<String>>,
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
    /// Set on auto-generated reconciliation adjustments; the UI can
    /// flag them with a badge and filter them out by default.
    pub source_snapshot_id: Option<String>,
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
    /// When false (default), reconciliation-adjustment rows are
    /// filtered out — they're not real spending and clutter the list.
    pub include_reconciliations: Option<bool>,
    pub limit: Option<u32>,
    pub skip: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategorySummaryItem {
    /// `None` for legs without a category (treat as "Uncategorized").
    pub category_id: Option<String>,
    pub income_minor: i64,
    /// Always negative or zero (sum of all negative leg amounts in the
    /// matching window).
    pub expense_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategorySummaryResponse {
    pub items: Vec<CategorySummaryItem>,
    pub income_total_minor: i64,
    /// Sum of all negative leg amounts (i.e. zero or negative).
    pub expense_total_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionListResponse {
    pub items: Vec<TransactionResponse>,
    pub total: u64,
}

// ── Settlements / IOUs ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettlementEntryResponse {
    pub id: String,
    /// "payment", "forgiveness", or "charge".
    pub kind: String,
    /// Optional per-entry override; useful for group settlements.
    pub counterparty: Option<String>,
    pub amount_minor: i64,
    pub date: String,
    pub linked_transaction_id: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceSettlementResponse {
    pub id: String,
    pub counterparty: String,
    /// "owed_to_me" or "owed_by_me".
    pub direction: String,
    /// The opening amount; `charge` entries add on top of it.
    pub amount_minor: i64,
    pub currency: String,
    pub category_id: Option<String>,
    pub description: String,
    pub notes: Option<String>,
    pub opened_at: String,
    pub next_payment_at: Option<String>,
    pub source_transaction_id: Option<String>,
    /// "open", "settled", or "forgiven".
    pub status: String,
    pub paid_minor: i64,
    pub forgiven_minor: i64,
    pub charged_minor: i64,
    /// `amount_minor + charged_minor - paid_minor - forgiven_minor`.
    pub outstanding_minor: i64,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

/// Returned by the single-settlement endpoint and by entry mutations.
/// Entries live in their own collection, so the list endpoint omits them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceSettlementDetailResponse {
    #[serde(flatten)]
    pub settlement: FinanceSettlementResponse,
    pub entries: Vec<SettlementEntryResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreateFinanceSettlementRequest {
    pub counterparty: String,
    pub direction: String,
    pub amount_minor: i64,
    pub currency: String,
    #[serde(default)]
    pub category_id: Option<String>,
    pub description: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub opened_at: String,
    #[serde(default)]
    pub next_payment_at: Option<String>,
    #[serde(default)]
    pub source_transaction_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateFinanceSettlementRequest {
    pub counterparty: Option<String>,
    pub direction: Option<String>,
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
    /// Two-level Option so PATCH can leave the field alone (omit it) or
    /// replace it. Clients clear a field by sending an empty string —
    /// plain JSON `null` deserializes to the outer `None` ("leave alone"),
    /// so it cannot express "clear".
    pub category_id: Option<Option<String>>,
    pub description: Option<String>,
    /// Same two-level shape; empty string clears.
    pub notes: Option<Option<String>>,
    pub opened_at: Option<String>,
    /// Same two-level shape; empty string clears.
    pub next_payment_at: Option<Option<String>>,
    /// Same two-level shape; empty string clears.
    pub source_transaction_id: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceSettlementListResponse {
    pub items: Vec<FinanceSettlementResponse>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreateSettlementEntryRequest {
    pub kind: String,
    #[serde(default)]
    pub counterparty: Option<String>,
    pub amount_minor: i64,
    pub date: String,
    #[serde(default)]
    pub linked_transaction_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
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
    pub account_id: String,
    /// Populated when the import auto-created its target account from
    /// the CSV's IBAN column.
    pub auto_created_account: Option<AccountResponse>,
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

// ── Categorization rules ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceRuleResponse {
    pub id: String,
    pub name: String,
    pub pattern: String,
    /// "substring", "starts_with", "wildcard", "regex".
    pub pattern_kind: String,
    pub case_insensitive: bool,
    pub category_id: String,
    pub priority: i32,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FinanceRuleRequest {
    pub name: String,
    pub pattern: String,
    pub pattern_kind: String,
    #[serde(default = "default_true_serde")]
    pub case_insensitive: bool,
    pub category_id: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true_serde")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReorderRulesRequest {
    pub rule_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyRulesResponse {
    pub updated: u32,
    pub still_unmatched: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRuleRequest {
    pub pattern: String,
    pub pattern_kind: String,
    #[serde(default = "default_true_serde")]
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRuleMatch {
    pub transaction_id: String,
    pub date: String,
    pub description: String,
    pub amount_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRuleResponse {
    pub sampled: u32,
    pub matches: Vec<TestRuleMatch>,
}

// ── Reconciliation ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconcileRequest {
    /// ISO date (YYYY-MM-DD).
    pub on_date: String,
    pub actual_balance_minor: i64,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconcilePreviewResponse {
    pub on_date: String,
    pub computed_minor: i64,
    pub actual_minor: i64,
    pub delta_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceSnapshotResponse {
    pub id: String,
    pub account_id: String,
    pub on_date: String,
    pub actual_balance_minor: i64,
    pub note: Option<String>,
    pub adjustment_transaction_id: String,
    pub created_at: String,
    /// Re-derived at read time: actual_balance - (balance recomputed
    /// from history excluding this adjustment). `0` means in-sync;
    /// non-zero means the adjustment no longer matches reality.
    pub drift_minor: i64,
}
