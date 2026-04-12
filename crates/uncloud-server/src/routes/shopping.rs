use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use bson::doc;
use chrono::Utc;
use mongodb::bson::oid::ObjectId;

use crate::error::{AppError, Result};
use crate::middleware::AuthUser;
use crate::models::{Shop, ShoppingCategory, ShoppingItem, ShoppingList, ShoppingListItem, User};
use crate::AppState;
use uncloud_common::{
    AddShoppingListItemRequest, CategoryResponse, CreateCategoryRequest, CreateShopRequest,
    CreateShoppingItemRequest, CreateShoppingListRequest, PatchShoppingListItemRequest,
    RenameShoppingListRequest, ShareListRequest, ShopResponse, ShoppingItemResponse,
    ShoppingListItemResponse, ShoppingListResponse, ShoppingListSummary, UpdateCategoryRequest,
    UpdatePositionRequest, UpdateShopRequest, UpdateShoppingItemRequest,
};

fn require_shopping(state: &AppState, user: &crate::models::User) -> Result<()> {
    if !state.config.features.shopping {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    if user.disabled_features.contains(&"shopping".to_string()) {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    Ok(())
}

fn list_access_filter(user_id: ObjectId) -> bson::Document {
    doc! {
        "$or": [
            { "owner_id": user_id },
            { "shared_with": user_id },
        ]
    }
}

fn item_to_response(item: &ShoppingItem) -> ShoppingItemResponse {
    ShoppingItemResponse {
        id: item.id.to_hex(),
        name: item.name.clone(),
        categories: item.categories.clone(),
        shop_ids: item.shop_ids.iter().map(|id| id.to_hex()).collect(),
        notes: item.notes.clone(),
        created_at: item.created_at.to_rfc3339(),
    }
}

const DEFAULT_CATEGORIES: &[&str] = &[
    "Grocery",
    "Dairy",
    "Produce",
    "Meat & Fish",
    "Bakery",
    "Frozen",
    "Beverages",
    "Cleaning",
    "Personal Care",
    "Home Appliances",
    "Other",
];

// ── Categories ──────────────────────────────────────────────────────────

/// `GET /api/shopping/categories`
pub async fn list_categories(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<CategoryResponse>>> {
    require_shopping(&state, &user)?;

    let coll = state.db.collection::<ShoppingCategory>("shopping_categories");

    // Check if user has any categories
    let count = coll
        .count_documents(doc! { "owner_id": user.id })
        .await?;

    // Seed defaults if empty
    if count == 0 {
        let now = Utc::now();
        for (i, name) in DEFAULT_CATEGORIES.iter().enumerate() {
            let cat = ShoppingCategory {
                id: ObjectId::new(),
                owner_id: user.id,
                name: name.to_string(),
                position: (i + 1) as f64,
                created_at: now,
            };
            // Ignore duplicate key errors in case of concurrent seeding
            let _ = coll.insert_one(&cat).await;
        }
    }

    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "position": 1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut categories = Vec::new();
    while cursor.advance().await? {
        let cat: ShoppingCategory = cursor.deserialize_current()?;
        categories.push(CategoryResponse {
            id: cat.id.to_hex(),
            name: cat.name,
            position: cat.position,
        });
    }

    Ok(Json(categories))
}

/// `POST /api/shopping/categories`
pub async fn create_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateCategoryRequest>,
) -> Result<(StatusCode, Json<CategoryResponse>)> {
    require_shopping(&state, &user)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Category name cannot be empty".to_string(),
        ));
    }

    let coll = state.db.collection::<ShoppingCategory>("shopping_categories");

    // Determine position: max position + 1.0
    let max_position = {
        let opts = mongodb::options::FindOptions::builder()
            .sort(doc! { "position": -1 })
            .limit(1)
            .build();
        let mut cursor = coll
            .find(doc! { "owner_id": user.id })
            .with_options(opts)
            .await?;
        if cursor.advance().await? {
            let c: ShoppingCategory = cursor.deserialize_current()?;
            c.position
        } else {
            0.0
        }
    };

    let cat = ShoppingCategory {
        id: ObjectId::new(),
        owner_id: user.id,
        name: name.clone(),
        position: max_position + 1.0,
        created_at: Utc::now(),
    };

    match coll.insert_one(&cat).await {
        Ok(_) => Ok((
            StatusCode::CREATED,
            Json(CategoryResponse {
                id: cat.id.to_hex(),
                name: cat.name,
                position: cat.position,
            }),
        )),
        Err(e) => {
            if is_duplicate_key(&e) {
                Err(AppError::BadRequest(
                    "A category with this name already exists".to_string(),
                ))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `DELETE /api/shopping/categories/{id}`
pub async fn delete_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let cat_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid category ID".to_string()))?;

    let coll = state.db.collection::<ShoppingCategory>("shopping_categories");

    // Fetch the category name before deleting so we can cascade
    let cat = coll
        .find_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Category".to_string()))?;

    coll.delete_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?;

    // Remove this category name from all shops belonging to this user
    let shops_coll = state.db.collection::<Shop>("shopping_shops");
    let _ = shops_coll
        .update_many(
            doc! { "owner_id": user.id },
            doc! { "$pull": { "categories": &cat.name } },
        )
        .await;

    // Remove this category name from all catalogue items belonging to this user
    let items_coll = state.db.collection::<ShoppingItem>("shopping_items");
    let _ = items_coll
        .update_many(
            doc! { "owner_id": user.id },
            doc! { "$pull": { "categories": &cat.name } },
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/shopping/categories/{id}`
pub async fn update_category(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateCategoryRequest>,
) -> Result<Json<CategoryResponse>> {
    require_shopping(&state, &user)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Category name cannot be empty".to_string()));
    }

    let cat_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid category ID".to_string()))?;

    let coll = state.db.collection::<ShoppingCategory>("shopping_categories");

    // Fetch old name before updating so we can cascade the rename
    let old_cat = coll
        .find_one(doc! { "_id": cat_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Category".to_string()))?;
    let old_name = old_cat.name.clone();

    let result = coll
        .update_one(
            doc! { "_id": cat_id, "owner_id": user.id },
            doc! { "$set": { "name": &name } },
        )
        .await;

    match result {
        Ok(r) if r.matched_count == 0 => Err(AppError::NotFound("Category".to_string())),
        Ok(_) => {
            // Cascade: rename in all items and shops that reference the old name
            if old_name != name {
                let items_coll = state.db.collection::<ShoppingItem>("shopping_items");
                let _ = items_coll
                    .update_many(
                        doc! { "owner_id": user.id, "categories": &old_name },
                        doc! { "$set": { "categories.$": &name } },
                    )
                    .await;

                let shops_coll = state.db.collection::<Shop>("shopping_shops");
                let _ = shops_coll
                    .update_many(
                        doc! { "owner_id": user.id, "categories": &old_name },
                        doc! { "$set": { "categories.$": &name } },
                    )
                    .await;
            }

            Ok(Json(CategoryResponse {
                id: cat_id.to_hex(),
                name,
                position: old_cat.position,
            }))
        }
        Err(e) => {
            if e.to_string().contains("E11000") {
                Err(AppError::Conflict(
                    "A category with this name already exists".to_string(),
                ))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `PUT /api/shopping/categories/{id}/position`
pub async fn update_category_position(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdatePositionRequest>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let cat_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid category ID".to_string()))?;

    let coll = state.db.collection::<ShoppingCategory>("shopping_categories");
    let result = coll
        .update_one(
            doc! { "_id": cat_id, "owner_id": user.id },
            doc! { "$set": { "position": body.position } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Category".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Shops ───────────────────────────────────────────────────────────────

/// `GET /api/shopping/shops`
pub async fn list_shops(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ShopResponse>>> {
    require_shopping(&state, &user)?;

    let coll = state.db.collection::<Shop>("shops");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "name": 1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut shops = Vec::new();
    while cursor.advance().await? {
        let shop: Shop = cursor.deserialize_current()?;
        shops.push(ShopResponse {
            id: shop.id.to_hex(),
            name: shop.name,
            categories: shop.categories,
        });
    }

    Ok(Json(shops))
}

/// `POST /api/shopping/shops`
pub async fn create_shop(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateShopRequest>,
) -> Result<(StatusCode, Json<ShopResponse>)> {
    require_shopping(&state, &user)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Shop name cannot be empty".to_string(),
        ));
    }

    let coll = state.db.collection::<Shop>("shops");
    let shop = Shop {
        id: ObjectId::new(),
        owner_id: user.id,
        name: name.clone(),
        categories: body.categories,
        created_at: Utc::now(),
    };

    match coll.insert_one(&shop).await {
        Ok(_) => Ok((
            StatusCode::CREATED,
            Json(ShopResponse {
                id: shop.id.to_hex(),
                name: shop.name,
                categories: shop.categories,
            }),
        )),
        Err(e) => {
            if is_duplicate_key(&e) {
                Err(AppError::BadRequest(
                    "A shop with this name already exists".to_string(),
                ))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `PUT /api/shopping/shops/{id}`
pub async fn update_shop(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateShopRequest>,
) -> Result<Json<ShopResponse>> {
    require_shopping(&state, &user)?;

    let shop_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid shop ID".to_string()))?;

    let coll = state.db.collection::<Shop>("shops");

    let mut update = doc! {};
    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Shop name cannot be empty".to_string(),
            ));
        }
        update.insert("name", name);
    }
    if let Some(ref categories) = body.categories {
        update.insert(
            "categories",
            bson::to_bson(categories).map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }

    if update.is_empty() {
        // Nothing to update — return current
        let shop = coll
            .find_one(doc! { "_id": shop_id, "owner_id": user.id })
            .await?
            .ok_or_else(|| AppError::NotFound("Shop".to_string()))?;
        return Ok(Json(ShopResponse {
            id: shop.id.to_hex(),
            name: shop.name,
            categories: shop.categories,
        }));
    }

    let result = coll
        .update_one(
            doc! { "_id": shop_id, "owner_id": user.id },
            doc! { "$set": update },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Shop".to_string()));
    }

    let updated = coll
        .find_one(doc! { "_id": shop_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Shop".to_string()))?;

    Ok(Json(ShopResponse {
        id: updated.id.to_hex(),
        name: updated.name,
        categories: updated.categories,
    }))
}

/// `DELETE /api/shopping/shops/{id}`
pub async fn delete_shop(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let shop_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid shop ID".to_string()))?;

    let coll = state.db.collection::<Shop>("shops");
    let result = coll
        .delete_one(doc! { "_id": shop_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Shop".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── Catalogue items ──────────────────────────────────────────────────────

/// `GET /api/shopping/items`
pub async fn list_items(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ShoppingItemResponse>>> {
    require_shopping(&state, &user)?;

    let coll = state.db.collection::<ShoppingItem>("shopping_items");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "name": 1 })
        .build();
    let mut cursor = coll
        .find(doc! { "owner_id": user.id })
        .with_options(options)
        .await?;

    let mut items = Vec::new();
    while cursor.advance().await? {
        items.push(item_to_response(&cursor.deserialize_current()?));
    }

    Ok(Json(items))
}

/// `POST /api/shopping/items`
pub async fn create_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateShoppingItemRequest>,
) -> Result<(StatusCode, Json<ShoppingItemResponse>)> {
    require_shopping(&state, &user)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "Item name cannot be empty".to_string(),
        ));
    }

    // Parse shop_ids
    let shop_ids: Vec<ObjectId> = body
        .shop_ids
        .iter()
        .map(|s| {
            ObjectId::parse_str(s)
                .map_err(|_| AppError::BadRequest(format!("Invalid shop ID: {}", s)))
        })
        .collect::<Result<Vec<_>>>()?;

    let coll = state.db.collection::<ShoppingItem>("shopping_items");

    let item = ShoppingItem {
        id: ObjectId::new(),
        owner_id: user.id,
        name,
        categories: body.categories,
        shop_ids,
        notes: body.notes,
        created_at: Utc::now(),
    };

    match coll.insert_one(&item).await {
        Ok(_) => Ok((StatusCode::CREATED, Json(item_to_response(&item)))),
        Err(e) => {
            if is_duplicate_key(&e) {
                Err(AppError::BadRequest(
                    "An item with this name already exists".to_string(),
                ))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `DELETE /api/shopping/items/{id}`
pub async fn delete_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let item_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid item ID".to_string()))?;

    let coll = state.db.collection::<ShoppingItem>("shopping_items");
    let result = coll
        .delete_one(doc! { "_id": item_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Item".to_string()));
    }

    // Cascade: remove from all lists
    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    list_items_coll
        .delete_many(doc! { "item_id": item_id, "owner_id": user.id })
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/shopping/items/{id}`
pub async fn update_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateShoppingItemRequest>,
) -> Result<Json<ShoppingItemResponse>> {
    require_shopping(&state, &user)?;

    let item_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid item ID".to_string()))?;

    let coll = state.db.collection::<ShoppingItem>("shopping_items");

    let mut update = doc! {};
    if let Some(ref name) = body.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Item name cannot be empty".to_string(),
            ));
        }
        update.insert("name", name);
    }
    if let Some(ref categories) = body.categories {
        // Validate: strip out category names that no longer exist
        let cat_coll = state.db.collection::<ShoppingCategory>("shopping_categories");
        let mut valid_cats = Vec::new();
        for cat_name in categories {
            let exists = cat_coll
                .count_documents(doc! { "owner_id": user.id, "name": cat_name })
                .await
                .unwrap_or(0);
            if exists > 0 {
                valid_cats.push(cat_name.clone());
            }
        }
        update.insert(
            "categories",
            bson::to_bson(&valid_cats).map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }
    if let Some(ref shop_ids) = body.shop_ids {
        let oids: Vec<ObjectId> = shop_ids
            .iter()
            .map(|s| {
                ObjectId::parse_str(s)
                    .map_err(|_| AppError::BadRequest(format!("Invalid shop ID: {}", s)))
            })
            .collect::<Result<Vec<_>>>()?;
        update.insert(
            "shop_ids",
            bson::to_bson(&oids).map_err(|e| AppError::Internal(e.to_string()))?,
        );
    }
    if let Some(ref notes) = body.notes {
        update.insert("notes", notes.as_str());
    }

    if update.is_empty() {
        let item = coll
            .find_one(doc! { "_id": item_id, "owner_id": user.id })
            .await?
            .ok_or_else(|| AppError::NotFound("Item".to_string()))?;
        return Ok(Json(item_to_response(&item)));
    }

    let result = coll
        .update_one(
            doc! { "_id": item_id, "owner_id": user.id },
            doc! { "$set": update },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Item".to_string()));
    }

    let updated = coll
        .find_one(doc! { "_id": item_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Item".to_string()))?;

    Ok(Json(item_to_response(&updated)))
}

// ── Shopping lists ───────────────────────────────────────────────────────

/// `GET /api/shopping/lists`
pub async fn list_lists(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ShoppingListSummary>>> {
    require_shopping(&state, &user)?;

    let coll = state.db.collection::<ShoppingList>("shopping_lists");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "updated_at": -1 })
        .build();
    let mut cursor = coll
        .find(list_access_filter(user.id))
        .with_options(options)
        .await?;

    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");

    // Look up usernames for shared_with
    let users_coll = state.db.collection::<User>("users");

    let mut summaries = Vec::new();
    while cursor.advance().await? {
        let list: ShoppingList = cursor.deserialize_current()?;
        let item_count = list_items_coll
            .count_documents(doc! { "list_id": list.id })
            .await? as usize;
        let checked_count = list_items_coll
            .count_documents(doc! { "list_id": list.id, "checked": true })
            .await? as usize;

        // Resolve shared_with user IDs to usernames
        let shared_with = resolve_usernames(&users_coll, &list.shared_with).await;

        summaries.push(ShoppingListSummary {
            id: list.id.to_hex(),
            name: list.name,
            item_count,
            checked_count,
            shared_with,
            created_at: list.created_at.to_rfc3339(),
        });
    }

    Ok(Json(summaries))
}

/// `POST /api/shopping/lists`
pub async fn create_list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateShoppingListRequest>,
) -> Result<(StatusCode, Json<ShoppingListSummary>)> {
    require_shopping(&state, &user)?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "List name cannot be empty".to_string(),
        ));
    }

    let coll = state.db.collection::<ShoppingList>("shopping_lists");
    let now = Utc::now();
    let list = ShoppingList {
        id: ObjectId::new(),
        owner_id: user.id,
        name: name.clone(),
        shared_with: Vec::new(),
        created_at: now,
        updated_at: now,
    };

    match coll.insert_one(&list).await {
        Ok(_) => Ok((
            StatusCode::CREATED,
            Json(ShoppingListSummary {
                id: list.id.to_hex(),
                name: list.name,
                item_count: 0,
                checked_count: 0,
                shared_with: Vec::new(),
                created_at: list.created_at.to_rfc3339(),
            }),
        )),
        Err(e) => {
            if is_duplicate_key(&e) {
                Err(AppError::Conflict(format!(
                    "A list named \"{}\" already exists",
                    name
                )))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `PUT /api/shopping/lists/{id}`
pub async fn update_list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<RenameShoppingListRequest>,
) -> Result<Json<ShoppingListSummary>> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            "List name cannot be empty".to_string(),
        ));
    }

    let coll = state.db.collection::<ShoppingList>("shopping_lists");

    // Must be owner to rename
    coll.update_one(
        doc! { "_id": list_id, "owner_id": user.id },
        doc! {
            "$set": {
                "name": &name,
                "updated_at": bson::DateTime::from_chrono(Utc::now()),
            }
        },
    )
    .await?;

    let updated = coll
        .find_one(doc! { "_id": list_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    let item_count = list_items_coll
        .count_documents(doc! { "list_id": list_id })
        .await? as usize;
    let checked_count = list_items_coll
        .count_documents(doc! { "list_id": list_id, "checked": true })
        .await? as usize;

    let users_coll = state.db.collection::<User>("users");
    let shared_with = resolve_usernames(&users_coll, &updated.shared_with).await;

    Ok(Json(ShoppingListSummary {
        id: updated.id.to_hex(),
        name: updated.name,
        item_count,
        checked_count,
        shared_with,
        created_at: updated.created_at.to_rfc3339(),
    }))
}

/// `DELETE /api/shopping/lists/{id}`
pub async fn delete_list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    // Only owner can delete
    let coll = state.db.collection::<ShoppingList>("shopping_lists");
    let result = coll
        .delete_one(doc! { "_id": list_id, "owner_id": user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("List".to_string()));
    }

    // Cascade: remove all list items
    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    list_items_coll
        .delete_many(doc! { "list_id": list_id })
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── List sharing ────────────────────────────────────────────────────────

/// `POST /api/shopping/lists/{id}/share`
pub async fn share_list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ShareListRequest>,
) -> Result<Json<ShoppingListSummary>> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    let username = body.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::BadRequest("Username cannot be empty".to_string()));
    }

    // Look up target user
    let users_coll = state.db.collection::<User>("users");
    let target_user = users_coll
        .find_one(doc! { "username": &username })
        .await?
        .ok_or_else(|| AppError::NotFound("User".to_string()))?;

    if target_user.id == user.id {
        return Err(AppError::BadRequest(
            "Cannot share a list with yourself".to_string(),
        ));
    }

    // Only owner can share
    let coll = state.db.collection::<ShoppingList>("shopping_lists");
    let result = coll
        .update_one(
            doc! { "_id": list_id, "owner_id": user.id },
            doc! { "$addToSet": { "shared_with": target_user.id } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("List".to_string()));
    }

    // Return updated summary
    let updated = coll
        .find_one(doc! { "_id": list_id, "owner_id": user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    let item_count = list_items_coll
        .count_documents(doc! { "list_id": list_id })
        .await? as usize;
    let checked_count = list_items_coll
        .count_documents(doc! { "list_id": list_id, "checked": true })
        .await? as usize;

    let shared_with = resolve_usernames(&users_coll, &updated.shared_with).await;

    Ok(Json(ShoppingListSummary {
        id: updated.id.to_hex(),
        name: updated.name,
        item_count,
        checked_count,
        shared_with,
        created_at: updated.created_at.to_rfc3339(),
    }))
}

/// `DELETE /api/shopping/lists/{id}/share/{user_id}`
pub async fn unshare_list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, target_user_ref)): Path<(String, String)>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    // Accept either an ObjectId or a username
    let target_id = if let Ok(oid) = ObjectId::parse_str(&target_user_ref) {
        oid
    } else {
        // Look up by username
        let users_coll = state.db.collection::<User>("users");
        let target_user = users_coll
            .find_one(doc! { "username": &target_user_ref })
            .await?
            .ok_or_else(|| AppError::NotFound("User".to_string()))?;
        target_user.id
    };

    // Only owner can unshare
    let coll = state.db.collection::<ShoppingList>("shopping_lists");
    let result = coll
        .update_one(
            doc! { "_id": list_id, "owner_id": user.id },
            doc! { "$pull": { "shared_with": target_id } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("List".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── List items ───────────────────────────────────────────────────────────

/// `GET /api/shopping/lists/{id}/items`
pub async fn get_list_items(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ShoppingListResponse>> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    // Check access (owner or shared_with)
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    let list = lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    let options = mongodb::options::FindOptions::builder()
        .sort(doc! { "position": 1 })
        .build();
    let mut cursor = list_items_coll
        .find(doc! { "list_id": list_id })
        .with_options(options)
        .await?;

    let mut list_items: Vec<ShoppingListItem> = Vec::new();
    while cursor.advance().await? {
        list_items.push(cursor.deserialize_current()?);
    }

    // Batch-fetch catalogue items
    let item_ids: Vec<bson::Bson> = list_items
        .iter()
        .map(|li| bson::Bson::ObjectId(li.item_id))
        .collect();

    let items_by_id = if item_ids.is_empty() {
        HashMap::new()
    } else {
        let items_coll = state.db.collection::<ShoppingItem>("shopping_items");
        let mut item_cursor = items_coll
            .find(doc! { "_id": { "$in": &item_ids } })
            .await?;
        let mut map = HashMap::new();
        while item_cursor.advance().await? {
            let item: ShoppingItem = item_cursor.deserialize_current()?;
            map.insert(item.id, item);
        }
        map
    };

    let response_items: Vec<ShoppingListItemResponse> = list_items
        .iter()
        .filter_map(|li| {
            let item = items_by_id.get(&li.item_id)?;
            Some(ShoppingListItemResponse {
                id: li.id.to_hex(),
                item_id: li.item_id.to_hex(),
                name: item.name.clone(),
                categories: item.categories.clone(),
                shop_ids: item.shop_ids.iter().map(|id| id.to_hex()).collect(),
                checked: li.checked,
                recurring: li.recurring,
                quantity: li.quantity.clone(),
                position: li.position,
                added_at: li.added_at.to_rfc3339(),
            })
        })
        .collect();

    let users_coll = state.db.collection::<User>("users");
    let shared_with = resolve_usernames(&users_coll, &list.shared_with).await;

    Ok(Json(ShoppingListResponse {
        id: list.id.to_hex(),
        name: list.name,
        items: response_items,
        shared_with,
        created_at: list.created_at.to_rfc3339(),
    }))
}

/// `POST /api/shopping/lists/{id}/items`
pub async fn add_list_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddShoppingListItemRequest>,
) -> Result<(StatusCode, Json<ShoppingListItemResponse>)> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    // Verify list access (owner or shared_with)
    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let items_coll = state.db.collection::<ShoppingItem>("shopping_items");

    // Parse shop_ids for new item creation
    let shop_oids: Vec<ObjectId> = body
        .shop_ids
        .iter()
        .filter_map(|s| ObjectId::parse_str(s).ok())
        .collect();

    // Resolve or create the catalogue item
    let catalogue_item = if let Some(ref item_id_str) = body.item_id {
        let item_id = ObjectId::parse_str(item_id_str)
            .map_err(|_| AppError::BadRequest("Invalid item ID".to_string()))?;
        items_coll
            .find_one(doc! { "_id": item_id, "owner_id": user.id })
            .await?
            .ok_or_else(|| AppError::NotFound("Item".to_string()))?
    } else if let Some(ref name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "Item name cannot be empty".to_string(),
            ));
        }
        // Find or create
        match items_coll
            .find_one(doc! { "owner_id": user.id, "name": &name })
            .await?
        {
            Some(existing) => existing,
            None => {
                let item = ShoppingItem {
                    id: ObjectId::new(),
                    owner_id: user.id,
                    name,
                    categories: body.categories.clone(),
                    shop_ids: shop_oids,
                    notes: None,
                    created_at: Utc::now(),
                };
                // Ignore duplicate key if a concurrent request creates it
                let _ = items_coll.insert_one(&item).await;
                items_coll
                    .find_one(doc! { "owner_id": user.id, "name": &item.name })
                    .await?
                    .unwrap_or(item)
            }
        }
    } else {
        return Err(AppError::BadRequest(
            "Either item_id or name must be provided".to_string(),
        ));
    };

    let list_items_coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");

    // Determine position: max position + 1.0
    let max_position = {
        let options = mongodb::options::FindOptions::builder()
            .sort(doc! { "position": -1 })
            .limit(1)
            .build();
        let mut cursor = list_items_coll
            .find(doc! { "list_id": list_id })
            .with_options(options)
            .await?;
        if cursor.advance().await? {
            let li: ShoppingListItem = cursor.deserialize_current()?;
            li.position
        } else {
            0.0
        }
    };

    let li = ShoppingListItem {
        id: ObjectId::new(),
        list_id,
        item_id: catalogue_item.id,
        owner_id: user.id,
        checked: false,
        recurring: body.recurring,
        quantity: body.quantity,
        position: max_position + 1.0,
        added_at: Utc::now(),
    };

    match list_items_coll.insert_one(&li).await {
        Ok(_) => {
            // Touch the list's updated_at
            lists_coll
                .update_one(
                    doc! { "_id": list_id },
                    doc! { "$set": { "updated_at": bson::DateTime::from_chrono(Utc::now()) } },
                )
                .await?;

            Ok((
                StatusCode::CREATED,
                Json(ShoppingListItemResponse {
                    id: li.id.to_hex(),
                    item_id: catalogue_item.id.to_hex(),
                    name: catalogue_item.name,
                    categories: catalogue_item.categories,
                    shop_ids: catalogue_item
                        .shop_ids
                        .iter()
                        .map(|id| id.to_hex())
                        .collect(),
                    checked: false,
                    recurring: li.recurring,
                    quantity: li.quantity,
                    position: li.position,
                    added_at: li.added_at.to_rfc3339(),
                }),
            ))
        }
        Err(e) => {
            if is_duplicate_key(&e) {
                Err(AppError::BadRequest(
                    "Item is already in this list".to_string(),
                ))
            } else {
                Err(AppError::Database(e))
            }
        }
    }
}

/// `PATCH /api/shopping/lists/{id}/items/{item_id}`
pub async fn patch_list_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, item_id)): Path<(String, String)>,
    Json(body): Json<PatchShoppingListItemRequest>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;
    let li_id = ObjectId::parse_str(&item_id)
        .map_err(|_| AppError::BadRequest("Invalid list item ID".to_string()))?;

    // Verify list access
    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");

    let mut update = doc! {};
    if let Some(checked) = body.checked {
        update.insert("checked", checked);
    }
    if let Some(ref quantity) = body.quantity {
        update.insert("quantity", quantity);
    }
    if let Some(recurring) = body.recurring {
        update.insert("recurring", recurring);
    }

    if update.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let result = coll
        .update_one(
            doc! { "_id": li_id, "list_id": list_id },
            doc! { "$set": update },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("List item".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/shopping/lists/{id}/items/{item_id}`
pub async fn remove_list_item(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, item_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;
    let li_id = ObjectId::parse_str(&item_id)
        .map_err(|_| AppError::BadRequest("Invalid list item ID".to_string()))?;

    // Verify list access
    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    let result = coll
        .delete_one(doc! { "_id": li_id, "list_id": list_id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("List item".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/shopping/lists/{id}/items/{item_id}/position`
pub async fn update_item_position(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, item_id)): Path<(String, String)>,
    Json(body): Json<UpdatePositionRequest>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;
    let li_id = ObjectId::parse_str(&item_id)
        .map_err(|_| AppError::BadRequest("Invalid list item ID".to_string()))?;

    // Verify list access
    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    let result = coll
        .update_one(
            doc! { "_id": li_id, "list_id": list_id },
            doc! { "$set": { "position": body.position } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("List item".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/shopping/lists/{id}/remove-purchased`
pub async fn remove_purchased(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    require_shopping(&state, &user)?;

    let list_id = ObjectId::parse_str(&id)
        .map_err(|_| AppError::BadRequest("Invalid list ID".to_string()))?;

    // Verify list access
    let lists_coll = state.db.collection::<ShoppingList>("shopping_lists");
    let mut filter = list_access_filter(user.id);
    filter.insert("_id", list_id);
    lists_coll
        .find_one(filter)
        .await?
        .ok_or_else(|| AppError::NotFound("List".to_string()))?;

    let coll = state
        .db
        .collection::<ShoppingListItem>("shopping_list_items");
    coll.delete_many(doc! { "list_id": list_id, "checked": true, "recurring": false })
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn is_duplicate_key(e: &mongodb::error::Error) -> bool {
    if let mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(we)) =
        e.kind.as_ref()
    {
        we.code == 11000
    } else {
        false
    }
}

async fn resolve_usernames(
    users_coll: &mongodb::Collection<User>,
    user_ids: &[ObjectId],
) -> Vec<String> {
    if user_ids.is_empty() {
        return Vec::new();
    }
    let bson_ids: Vec<bson::Bson> = user_ids
        .iter()
        .map(|id| bson::Bson::ObjectId(*id))
        .collect();
    let mut usernames = Vec::new();
    if let Ok(mut cursor) = users_coll
        .find(doc! { "_id": { "$in": &bson_ids } })
        .await
    {
        while cursor.advance().await.unwrap_or(false) {
            if let Ok(user) = cursor.deserialize_current() {
                usernames.push(user.username.clone());
            }
        }
    }
    usernames
}
