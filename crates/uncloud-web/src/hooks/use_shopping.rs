use uncloud_common::{
    AddShoppingListItemRequest, CategoryResponse, CreateCategoryRequest, CreateShopRequest,
    CreateShoppingItemRequest, CreateShoppingListRequest, PatchShoppingListItemRequest,
    RenameShoppingListRequest, ShareListRequest, ShopResponse, ShoppingItemResponse,
    ShoppingListItemResponse, ShoppingListResponse, ShoppingListSummary, UpdateCategoryRequest,
    UpdateFeaturesRequest, UpdatePositionRequest, UpdateShopRequest, UpdateShoppingItemRequest,
    UserResponse,
};

use super::api;

// ── Categories ──────────────────────────────────────────────────────────

pub async fn list_categories() -> Result<Vec<CategoryResponse>, String> {
    let response = api::get("/shopping/categories")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<CategoryResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load categories".to_string())
    }
}

pub async fn create_category(name: &str) -> Result<CategoryResponse, String> {
    let body = CreateCategoryRequest {
        name: name.to_string(),
    };
    let response = api::post("/shopping/categories")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<CategoryResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create category".to_string())
    }
}

pub async fn delete_category(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/shopping/categories/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete category".to_string())
    }
}

pub async fn rename_category(id: &str, name: &str) -> Result<CategoryResponse, String> {
    let body = UpdateCategoryRequest { name: name.to_string() };
    let response = api::put(&format!("/shopping/categories/{}", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<CategoryResponse>().await.map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("A category with this name already exists".to_string())
    } else {
        Err("Failed to rename category".to_string())
    }
}

pub async fn update_category_position(id: &str, position: f64) -> Result<(), String> {
    let body = UpdatePositionRequest { position };
    let response = api::put(&format!("/shopping/categories/{}/position", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to update category position".to_string())
    }
}

// ── Shops ───────────────────────────────────────────────────────────────

pub async fn list_shops() -> Result<Vec<ShopResponse>, String> {
    let response = api::get("/shopping/shops")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ShopResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load shops".to_string())
    }
}

pub async fn create_shop(req: CreateShopRequest) -> Result<ShopResponse, String> {
    let response = api::post("/shopping/shops")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<ShopResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create shop".to_string())
    }
}

pub async fn update_shop(id: &str, req: UpdateShopRequest) -> Result<ShopResponse, String> {
    let response = api::put(&format!("/shopping/shops/{}", id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShopResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update shop".to_string())
    }
}

pub async fn delete_shop(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/shopping/shops/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete shop".to_string())
    }
}

// ── Catalogue items ─────────────────────────────────────────────────────

pub async fn list_items() -> Result<Vec<ShoppingItemResponse>, String> {
    let response = api::get("/shopping/items")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ShoppingItemResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load items".to_string())
    }
}

pub async fn create_item(
    req: CreateShoppingItemRequest,
) -> Result<ShoppingItemResponse, String> {
    let response = api::post("/shopping/items")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<ShoppingItemResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to create item".to_string())
    }
}

pub async fn delete_item(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/shopping/items/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete item".to_string())
    }
}

pub async fn update_item(
    id: &str,
    req: UpdateShoppingItemRequest,
) -> Result<ShoppingItemResponse, String> {
    let response = api::put(&format!("/shopping/items/{}", id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShoppingItemResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update item".to_string())
    }
}

// ── Shopping lists ──────────────────────────────────────────────────────

pub async fn list_lists() -> Result<Vec<ShoppingListSummary>, String> {
    let response = api::get("/shopping/lists")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<Vec<ShoppingListSummary>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to load lists".to_string())
    }
}

pub async fn create_list(name: &str) -> Result<ShoppingListSummary, String> {
    let body = CreateShoppingListRequest {
        name: name.to_string(),
    };
    let response = api::post("/shopping/lists")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<ShoppingListSummary>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 409 {
        Err("CONFLICT".to_string())
    } else {
        Err("Failed to create list".to_string())
    }
}

pub async fn rename_list(id: &str, name: &str) -> Result<ShoppingListSummary, String> {
    let body = RenameShoppingListRequest {
        name: name.to_string(),
    };
    let response = api::put(&format!("/shopping/lists/{}", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShoppingListSummary>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to rename list".to_string())
    }
}

pub async fn delete_list(id: &str) -> Result<(), String> {
    let response = api::delete(&format!("/shopping/lists/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to delete list".to_string())
    }
}

pub async fn get_list(list_id: &str) -> Result<ShoppingListResponse, String> {
    let response = api::get(&format!("/shopping/lists/{}/items", list_id))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShoppingListResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 404 {
        Err("List not found".to_string())
    } else {
        Err("Failed to load list".to_string())
    }
}

// ── List sharing ────────────────────────────────────────────────────────

pub async fn share_list(list_id: &str, username: &str) -> Result<ShoppingListSummary, String> {
    let body = ShareListRequest {
        username: username.to_string(),
    };
    let response = api::post(&format!("/shopping/lists/{}/share", list_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<ShoppingListSummary>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 404 {
        Err("User not found".to_string())
    } else {
        Err("Failed to share list".to_string())
    }
}

pub async fn unshare_list(list_id: &str, user_id: &str) -> Result<(), String> {
    let response = api::delete(&format!(
        "/shopping/lists/{}/share/{}",
        list_id, user_id
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to unshare list".to_string())
    }
}

// ── List items ──────────────────────────────────────────────────────────

pub async fn add_list_item(
    list_id: &str,
    req: AddShoppingListItemRequest,
) -> Result<ShoppingListItemResponse, String> {
    let response = api::post(&format!("/shopping/lists/{}/items", list_id))
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 201 {
        response
            .json::<ShoppingListItemResponse>()
            .await
            .map_err(|e| e.to_string())
    } else if response.status() == 400 {
        Err("Item is already in this list".to_string())
    } else {
        Err("Failed to add item".to_string())
    }
}

pub async fn patch_list_item(
    list_id: &str,
    item_id: &str,
    req: PatchShoppingListItemRequest,
) -> Result<(), String> {
    let response = api::patch(&format!(
        "/shopping/lists/{}/items/{}",
        list_id, item_id
    ))
    .json(&req)
    .map_err(|e| e.to_string())?
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to update item".to_string())
    }
}

pub async fn remove_list_item(list_id: &str, item_id: &str) -> Result<(), String> {
    let response = api::delete(&format!(
        "/shopping/lists/{}/items/{}",
        list_id, item_id
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to remove item".to_string())
    }
}

pub async fn update_item_position(
    list_id: &str,
    item_id: &str,
    position: f64,
) -> Result<(), String> {
    let body = UpdatePositionRequest { position };
    let response = api::put(&format!(
        "/shopping/lists/{}/items/{}/position",
        list_id, item_id
    ))
    .json(&body)
    .map_err(|e| e.to_string())?
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to update position".to_string())
    }
}

pub async fn remove_purchased(list_id: &str) -> Result<(), String> {
    let response = api::post(&format!(
        "/shopping/lists/{}/remove-purchased",
        list_id
    ))
    .send()
    .await
    .map_err(|e| e.to_string())?;

    if response.ok() || response.status() == 204 {
        Ok(())
    } else {
        Err("Failed to remove purchased items".to_string())
    }
}

pub async fn clear_checked(list_id: &str) -> Result<(), String> {
    remove_purchased(list_id).await
}

pub async fn list_usernames() -> Result<Vec<String>, String> {
    let response = api::get("/users/names")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response.json::<Vec<String>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load users".to_string())
    }
}

pub async fn update_my_features(req: UpdateFeaturesRequest) -> Result<UserResponse, String> {
    let response = api::put_v1("/auth/me/features")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if response.ok() {
        response
            .json::<UserResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Failed to update features".to_string())
    }
}
