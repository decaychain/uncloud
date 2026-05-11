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
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}
