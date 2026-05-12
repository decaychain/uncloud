//! CSV-import pipeline for the finance tracker.
//!
//! `ImportProfile` is the bank-format plug point. Each profile takes raw
//! CSV bytes (as the bank delivers them — encoding-and-all) and produces a
//! `Vec<ParsedRow>`. The route handler then UPSERTs those into
//! `finance_transactions` keyed by `(account_id, source_ref)` — the partial
//! unique index on that pair makes re-imports of the same file idempotent.
//!
//! Today we ship one concrete profile (Sparkasse CAMT V8). Adding a new
//! bank means implementing `ImportProfile` and listing the instance in
//! `available_profiles()`.

use chrono::{DateTime, Utc};

pub mod sparkasse_camt_v8;

#[derive(Debug, Clone)]
pub struct ParsedRow {
    pub date: DateTime<Utc>,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub raw_bank_category: Option<String>,
    /// Unique-per-account dedup key. Profiles MUST produce a deterministic
    /// value here so the partial unique index makes re-imports idempotent.
    pub source_ref: String,
}

#[derive(Debug, Clone)]
pub enum ParseError {
    /// The whole input is malformed (wrong delimiter, can't decode, etc.)
    /// — abort the import.
    Fatal(String),
    /// One row was bad; skip it and keep going.
    Row { line: u32, message: String },
}

pub struct ProfileInfo {
    pub id: &'static str,
    pub name: &'static str,
}

pub trait ImportProfile: Send + Sync {
    fn info(&self) -> ProfileInfo;
    fn parse(&self, bytes: &[u8]) -> Result<Vec<Result<ParsedRow, ParseError>>, ParseError>;
}

pub fn available_profiles() -> Vec<Box<dyn ImportProfile>> {
    vec![Box::new(sparkasse_camt_v8::SparkasseCamtV8)]
}

pub fn profile_by_id(id: &str) -> Option<Box<dyn ImportProfile>> {
    available_profiles().into_iter().find(|p| p.info().id == id)
}
