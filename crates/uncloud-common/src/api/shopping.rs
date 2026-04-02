use serde::{Deserialize, Serialize};

// Categories
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryResponse {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub position: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCategoryRequest {
    pub name: String,
}

// Shops
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopResponse {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShopRequest {
    pub name: String,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateShopRequest {
    pub name: Option<String>,
    pub categories: Option<Vec<String>>,
}

// Catalogue items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingItemResponse {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub shop_ids: Vec<String>,
    pub notes: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShoppingItemRequest {
    pub name: String,
    pub categories: Vec<String>,
    pub shop_ids: Vec<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateShoppingItemRequest {
    pub name: Option<String>,
    pub categories: Option<Vec<String>>,
    pub shop_ids: Option<Vec<String>>,
    pub notes: Option<String>,
}

// Lists
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingListSummary {
    pub id: String,
    pub name: String,
    pub item_count: usize,
    pub checked_count: usize,
    pub shared_with: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingListResponse {
    pub id: String,
    pub name: String,
    pub items: Vec<ShoppingListItemResponse>,
    pub shared_with: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShoppingListRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameShoppingListRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareListRequest {
    pub username: String,
}

// List items
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShoppingListItemResponse {
    pub id: String,
    pub item_id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub shop_ids: Vec<String>,
    pub checked: bool,
    pub recurring: bool,
    pub quantity: Option<String>,
    pub position: f64,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddShoppingListItemRequest {
    pub item_id: Option<String>,
    pub name: Option<String>,
    pub categories: Vec<String>,
    pub shop_ids: Vec<String>,
    pub quantity: Option<String>,
    #[serde(default)]
    pub recurring: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchShoppingListItemRequest {
    pub checked: Option<bool>,
    pub quantity: Option<String>,
    pub recurring: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePositionRequest {
    pub position: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFeaturesRequest {
    pub shopping: Option<bool>,
}
