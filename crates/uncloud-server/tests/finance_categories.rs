//! Integration tests for finance category hierarchy behavior.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::Value;

async fn create_category(app: &TestApp, name: &str, parent_id: Option<&str>) -> Value {
    let body = match parent_id {
        Some(parent_id) => serde_json::json!({ "name": name, "parent_id": parent_id }),
        None => serde_json::json!({ "name": name }),
    };
    let res = app.server.post("/api/finance/categories").json(&body).await;
    res.assert_status(StatusCode::CREATED);
    res.json()
}

async fn create_account(app: &TestApp) -> String {
    let res = app
        .server
        .post("/api/finance/accounts")
        .json(&serde_json::json!({
            "name": "Checking",
            "account_type": "checking",
            "currency": "EUR",
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let account: Value = res.json();
    account["id"].as_str().unwrap().to_string()
}

async fn create_transaction(
    app: &TestApp,
    account_id: &str,
    description: &str,
    amount_minor: i64,
    category_id: &str,
) -> Value {
    let res = app
        .server
        .post("/api/finance/transactions")
        .json(&serde_json::json!({
            "account_id": account_id,
            "date": "2026-05-01",
            "amount_minor": amount_minor,
            "description": description,
            "category_id": category_id,
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    res.json()
}

#[tokio::test]
async fn categories_can_be_nested_and_promoted() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let parent = create_category(&app, "Housing", None).await;
    let child = create_category(&app, "Rent", None).await;
    let parent_id = parent["id"].as_str().unwrap();
    let child_id = child["id"].as_str().unwrap();

    let nested = app
        .server
        .put(&format!("/api/finance/categories/{child_id}"))
        .json(&serde_json::json!({ "parent_id": parent_id }))
        .await;
    nested.assert_status_ok();
    let nested: Value = nested.json();
    assert_eq!(nested["parent_id"], parent_id);

    let promoted = app
        .server
        .put(&format!("/api/finance/categories/{child_id}"))
        .json(&serde_json::json!({ "parent_id": "" }))
        .await;
    promoted.assert_status_ok();
    let promoted: Value = promoted.json();
    assert!(promoted["parent_id"].is_null());

    app.cleanup().await;
}

#[tokio::test]
async fn parent_category_filter_includes_subcategories() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;

    let account_id = create_account(&app).await;
    let parent = create_category(&app, "Housing", None).await;
    let child = create_category(&app, "Rent", Some(parent["id"].as_str().unwrap())).await;
    let other = create_category(&app, "Groceries", None).await;
    let parent_id = parent["id"].as_str().unwrap();
    let child_id = child["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();

    create_transaction(&app, &account_id, "Mortgage", -100_00, parent_id).await;
    create_transaction(&app, &account_id, "Rent", -50_00, child_id).await;
    create_transaction(&app, &account_id, "Supermarket", -25_00, other_id).await;

    let parent_list: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("category_id", parent_id)
        .await
        .json();
    assert_eq!(parent_list["total"], 2);
    let descriptions: Vec<&str> = parent_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["description"].as_str().unwrap())
        .collect();
    assert!(descriptions.contains(&"Mortgage"));
    assert!(descriptions.contains(&"Rent"));
    assert!(!descriptions.contains(&"Supermarket"));

    let child_list: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("category_id", child_id)
        .await
        .json();
    assert_eq!(child_list["total"], 1);
    assert_eq!(child_list["items"][0]["description"], "Rent");

    app.cleanup().await;
}

#[tokio::test]
async fn category_with_children_cannot_be_nested() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;

    let parent = create_category(&app, "Housing", None).await;
    let child = create_category(&app, "Rent", Some(parent["id"].as_str().unwrap())).await;
    let other = create_category(&app, "Living", None).await;
    assert_eq!(child["parent_id"], parent["id"]);

    let parent_id = parent["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();
    let rejected = app
        .server
        .put(&format!("/api/finance/categories/{parent_id}"))
        .json(&serde_json::json!({ "parent_id": other_id }))
        .await;
    rejected.assert_status(StatusCode::BAD_REQUEST);

    app.cleanup().await;
}
