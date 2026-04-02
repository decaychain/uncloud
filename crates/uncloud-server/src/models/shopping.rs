use bson::serde_helpers::chrono_datetime_as_bson_datetime;
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingCategory {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub position: f64,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shop {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingItem {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub shop_ids: Vec<ObjectId>,
    pub notes: Option<String>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingList {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub shared_with: Vec<ObjectId>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingListItem {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub list_id: ObjectId,
    pub item_id: ObjectId,
    pub owner_id: ObjectId,
    pub checked: bool,
    #[serde(default)]
    pub recurring: bool,
    pub quantity: Option<String>,
    #[serde(default)]
    pub position: f64,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub added_at: DateTime<Utc>,
}
