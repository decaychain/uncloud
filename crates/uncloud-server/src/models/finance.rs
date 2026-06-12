//! Finance tracker domain models — accounts, categories, transactions.
//!
//! v0 scope: manual entry only. CSV import lands in a later iteration but
//! the model already carries the fields it will need (`source_ref`,
//! `raw_bank_category`, embedded leg array) so the schema doesn't need a
//! migration when that work begins.
//!
//! Money is stored as `i64` minor units (cents) with a global assumption
//! of two decimal places. JPY-style zero-decimal currencies will need
//! revisiting before they're supported.

use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinanceAccount {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    #[serde(default)]
    pub opening_balance_minor: i64,
    /// Normalized IBAN (whitespace stripped, uppercased). Used by the
    /// CSV importer to find or auto-create the target account when the
    /// schema marks an `iban_column`.
    #[serde(default)]
    pub iban: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinanceCategory {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    #[serde(default)]
    pub parent_id: Option<ObjectId>,
    pub name: String,
    #[serde(default)]
    pub colour: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CategorySource {
    Unset,
    User,
    Rule,
}

impl Default for CategorySource {
    fn default() -> Self {
        Self::Unset
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionLeg {
    pub amount_minor: i64,
    #[serde(default)]
    pub category_id: Option<ObjectId>,
    #[serde(default)]
    pub category_source: CategorySource,
    #[serde(default)]
    pub rule_id: Option<ObjectId>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinanceTransaction {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub currency: String,
    pub amount_minor: i64,
    pub description: String,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub date: DateTime<Utc>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub raw_bank_category: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub legs: Vec<TransactionLeg>,
    /// Tags the run that created this row. `None` for manually-created
    /// transactions and for rows imported before runs existed. Reverting
    /// an `ImportRun` deletes every transaction with this id.
    #[serde(default)]
    pub import_run_id: Option<ObjectId>,
    /// Set on the auto-generated adjustment that backs a `BalanceSnapshot`.
    /// Such rows are not editable directly; they regenerate from the
    /// snapshot via `POST /finance/snapshots/{id}/recompute`.
    #[serde(default)]
    pub source_snapshot_id: Option<ObjectId>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportRunStatus {
    Applied,
    Reverted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceKind {
    Upload,
    UncloudFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSource {
    pub kind: ImportSourceKind,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub uncloud_file_id: Option<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRunSummary {
    pub created: u32,
    pub skipped_duplicate: u32,
    pub errored: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRunError {
    pub line: u32,
    pub message: String,
}

/// One CSV import attempt. Created at commit time (single-phase for v1
/// — preview/apply two-phase is deferred). Reverting deletes all
/// transactions whose `import_run_id` matches this run; the run row
/// stays as an audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRun {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    pub schema_id: ObjectId,
    pub source: ImportSource,
    pub status: ImportRunStatus,
    pub summary: ImportRunSummary,
    #[serde(default)]
    pub errors: Vec<ImportRunError>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub reverted_at: Option<DateTime<Utc>>,
}

/// A bank-statement reconciliation checkpoint. Storing the actual
/// balance separately from the adjustment transaction lets us detect
/// drift after late imports and regenerate the adjustment to match
/// the statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceSnapshot {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub account_id: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub on_date: DateTime<Utc>,
    pub actual_balance_minor: i64,
    #[serde(default)]
    pub note: Option<String>,
    /// The adjustment transaction generated to bridge computed→actual.
    pub adjustment_transaction_id: ObjectId,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettlementDirection {
    OwedToMe,
    OwedByMe,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettlementStatus {
    Open,
    Settled,
    Forgiven,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettlementEntryKind {
    Payment,
    Forgiveness,
    /// Adds to the outstanding amount instead of reducing it — a new
    /// obligation tracked under the same settlement ("you also owe me
    /// 50 for the door").
    Charge,
}

/// Stored in its own `finance_settlement_entries` collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementEntry {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub settlement_id: ObjectId,
    pub kind: SettlementEntryKind,
    #[serde(default)]
    pub counterparty: Option<String>,
    pub amount_minor: i64,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub date: DateTime<Utc>,
    #[serde(default)]
    pub linked_transaction_id: Option<ObjectId>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinanceSettlement {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub counterparty: String,
    pub direction: SettlementDirection,
    /// Opening amount; the live balance also counts `charge` entries.
    pub amount_minor: i64,
    pub currency: String,
    #[serde(default)]
    pub category_id: Option<ObjectId>,
    pub description: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub opened_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub next_payment_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source_transaction_id: Option<ObjectId>,
    pub status: SettlementStatus,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RulePatternKind {
    Substring,
    StartsWith,
    Wildcard,
    Regex,
}

impl Default for RulePatternKind {
    fn default() -> Self {
        Self::Substring
    }
}

/// A user-defined categorization rule. Applied on import (first match
/// wins, ordered by priority asc) and on demand via the "apply rules"
/// endpoint. User-set categories are never overwritten.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinanceRule {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    pub pattern: String,
    pub pattern_kind: RulePatternKind,
    #[serde(default = "default_case_insensitive")]
    pub case_insensitive: bool,
    pub category_id: ObjectId,
    /// Lower numbers run first. Ties broken by insertion order (mongo _id).
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

fn default_case_insensitive() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecimalSeparator {
    Dot,
    Comma,
}

impl Default for DecimalSeparator {
    fn default() -> Self {
        Self::Dot
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AmountSignConvention {
    /// Positive numbers are credits (money in), negatives are debits.
    PositiveCredit,
    /// Positive numbers are debits (money out); we negate to get the
    /// signed amount. Common in invoice-style exports.
    PositiveDebit,
}

impl Default for AmountSignConvention {
    fn default() -> Self {
        Self::PositiveCredit
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CurrencySource {
    /// Currency lives in a CSV column (column index in `currency_column`).
    Column,
    /// Same currency for every row (in `fixed_currency`).
    Fixed,
}

/// A user-editable CSV-import schema. One per bank format. Each user has
/// their own set of schemas plus seeded builtin templates that they can
/// clone but not edit in place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSchema {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,

    pub delimiter: String,
    pub encoding: String,
    pub decimal_separator: DecimalSeparator,
    #[serde(default)]
    pub skip_header_rows: u32,
    #[serde(default = "default_true")]
    pub has_headers: bool,

    pub date_column: u32,
    pub date_format: String,

    pub amount_column: u32,
    pub amount_sign_convention: AmountSignConvention,

    /// Joined with " / " when more than one column is selected. Empty
    /// fields are dropped from the join.
    pub description_columns: Vec<u32>,

    pub currency_source: CurrencySource,
    #[serde(default)]
    pub currency_column: Option<u32>,
    #[serde(default)]
    pub fixed_currency: Option<String>,

    /// Stable per-row identifier column (e.g. transaction reference).
    /// When set, `source_ref` is derived from it; when None, source_ref
    /// is a hash of the whole row.
    #[serde(default)]
    pub bank_ref_column: Option<u32>,
    #[serde(default)]
    pub iban_column: Option<u32>,
    #[serde(default)]
    pub raw_category_column: Option<u32>,

    /// True for the seeded "Sparkasse CAMT V8" template. Builtin schemas
    /// cannot be edited or deleted, only cloned.
    #[serde(default)]
    pub is_builtin: bool,
    /// Stable identifier for seeded templates so we don't re-seed.
    #[serde(default)]
    pub builtin_id: Option<String>,

    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}
