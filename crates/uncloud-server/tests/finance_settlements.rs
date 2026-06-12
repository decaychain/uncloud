//! Integration tests for Finance settlements / IOUs.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::Value;

async fn create_settlement(app: &TestApp, counterparty: &str, amount_minor: i64) -> Value {
    let res = app
        .server
        .post("/api/finance/settlements")
        .json(&serde_json::json!({
            "counterparty": counterparty,
            "direction": "owed_to_me",
            "amount_minor": amount_minor,
            "currency": "EUR",
            "description": "Dinner",
            "opened_at": "2026-06-01",
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    res.json()
}

async fn add_entry(
    app: &TestApp,
    settlement_id: &str,
    kind: &str,
    counterparty: Option<&str>,
    amount_minor: i64,
) -> (StatusCode, Value) {
    let mut body = serde_json::json!({
        "kind": kind,
        "amount_minor": amount_minor,
        "date": "2026-06-02",
    });
    if let Some(counterparty) = counterparty {
        body["counterparty"] = serde_json::json!(counterparty);
    }
    let res = app
        .server
        .post(&format!("/api/finance/settlements/{settlement_id}/entries"))
        .json(&body)
        .await;
    let status = res.status_code();
    let body = if status == StatusCode::CREATED || status == StatusCode::BAD_REQUEST {
        res.json()
    } else {
        Value::Null
    };
    (status, body)
}

#[tokio::test]
async fn group_settlement_closes_after_partial_payments() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;

    let settlement = create_settlement(&app, "Friends", 50_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    assert_eq!(settlement["status"], "open");
    assert_eq!(settlement["outstanding_minor"], 50_00);

    let (status, first) = add_entry(&app, &settlement_id, "payment", Some("Bob"), 20_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(first["status"], "open");
    assert_eq!(first["paid_minor"], 20_00);
    assert_eq!(first["outstanding_minor"], 30_00);
    assert_eq!(first["entries"][0]["counterparty"], "Bob");

    let (status, closed) = add_entry(&app, &settlement_id, "payment", Some("Gary"), 30_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(closed["status"], "settled");
    assert_eq!(closed["paid_minor"], 50_00);
    assert_eq!(closed["outstanding_minor"], 0);
    assert_eq!(closed["entries"].as_array().unwrap().len(), 2);

    let open: Value = app
        .server
        .get("/api/finance/settlements")
        .add_query_param("status", "open")
        .await
        .json();
    assert_eq!(open["total"], 0);

    let settled: Value = app
        .server
        .get("/api/finance/settlements")
        .add_query_param("status", "settled")
        .await
        .json();
    assert_eq!(settled["total"], 1);

    app.cleanup().await;
}

#[tokio::test]
async fn forgiveness_can_close_and_entry_delete_reopens() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;

    let settlement = create_settlement(&app, "Carol", 12_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (status, forgiven) = add_entry(&app, &settlement_id, "forgiveness", None, 12_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(forgiven["status"], "forgiven");
    assert_eq!(forgiven["forgiven_minor"], 12_00);
    assert_eq!(forgiven["outstanding_minor"], 0);
    let entry_id = forgiven["entries"][0]["id"].as_str().unwrap();

    let reopened: Value = app
        .server
        .delete(&format!(
            "/api/finance/settlements/{settlement_id}/entries/{entry_id}"
        ))
        .await
        .json();
    assert_eq!(reopened["status"], "open");
    assert_eq!(reopened["forgiven_minor"], 0);
    assert_eq!(reopened["outstanding_minor"], 12_00);
    assert!(reopened["closed_at"].is_null());

    app.cleanup().await;
}

#[tokio::test]
async fn entry_cannot_exceed_outstanding_amount() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;

    let settlement = create_settlement(&app, "Alice", 10_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (status, _) = add_entry(&app, &settlement_id, "payment", None, 11_00).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    app.cleanup().await;
}

#[tokio::test]
async fn charge_increases_outstanding_and_reopens_settled() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;

    // Bob owes 100 for the window, pays 20, asks for the door (+50) → owes 130.
    let settlement = create_settlement(&app, "Bob", 100_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();

    let (status, _) = add_entry(&app, &settlement_id, "payment", None, 20_00).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, charged) = add_entry(&app, &settlement_id, "charge", None, 50_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(charged["status"], "open");
    assert_eq!(charged["charged_minor"], 50_00);
    assert_eq!(charged["outstanding_minor"], 130_00);

    let (status, settled) = add_entry(&app, &settlement_id, "payment", None, 130_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(settled["status"], "settled");

    // Payments are rejected on a closed settlement, but a charge reopens it.
    let (status, _) = add_entry(&app, &settlement_id, "payment", None, 1_00).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, reopened) = add_entry(&app, &settlement_id, "charge", None, 25_00).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reopened["status"], "open");
    assert_eq!(reopened["outstanding_minor"], 25_00);
    assert!(reopened["closed_at"].is_null());

    app.cleanup().await;
}

#[tokio::test]
async fn charge_entry_cannot_be_deleted_below_payments() {
    let app = TestApp::new().await;
    app.register_and_login("erin").await;

    let settlement = create_settlement(&app, "Bob", 100_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (_, charged) = add_entry(&app, &settlement_id, "charge", None, 50_00).await;
    let charge_id = charged["entries"][0]["id"].as_str().unwrap().to_string();
    let (_, _) = add_entry(&app, &settlement_id, "payment", None, 120_00).await;

    // Removing the charge would leave 100 owed with 120 paid.
    let res = app
        .server
        .delete(&format!(
            "/api/finance/settlements/{settlement_id}/entries/{charge_id}"
        ))
        .await;
    res.assert_status(StatusCode::BAD_REQUEST);

    app.cleanup().await;
}

#[tokio::test]
async fn notes_and_next_payment_roundtrip() {
    let app = TestApp::new().await;
    app.register_and_login("frank").await;

    let res = app
        .server
        .post("/api/finance/settlements")
        .json(&serde_json::json!({
            "counterparty": "Bob",
            "direction": "owed_to_me",
            "amount_minor": 100_00,
            "currency": "EUR",
            "description": "Window repair",
            "notes": "Materials included",
            "opened_at": "2026-06-01",
            "next_payment_at": "2026-06-19",
        }))
        .await;
    res.assert_status(StatusCode::CREATED);
    let settlement: Value = res.json();
    assert_eq!(settlement["notes"], "Materials included");
    assert!(
        settlement["next_payment_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-19")
    );
    let settlement_id = settlement["id"].as_str().unwrap().to_string();

    // Empty strings clear the optional fields (JSON null cannot express
    // "clear" through the two-level Option).
    let updated: Value = app
        .server
        .put(&format!("/api/finance/settlements/{settlement_id}"))
        .json(&serde_json::json!({ "notes": "", "next_payment_at": "" }))
        .await
        .json();
    assert!(updated["notes"].is_null());
    assert!(updated["next_payment_at"].is_null());

    app.cleanup().await;
}

#[tokio::test]
async fn detail_endpoint_returns_entries_and_list_omits_them() {
    let app = TestApp::new().await;
    app.register_and_login("grace").await;

    let settlement = create_settlement(&app, "Friends", 50_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (_, _) = add_entry(&app, &settlement_id, "payment", Some("Bob"), 20_00).await;

    let detail: Value = app
        .server
        .get(&format!("/api/finance/settlements/{settlement_id}"))
        .await
        .json();
    assert_eq!(detail["outstanding_minor"], 30_00);
    assert_eq!(detail["entries"].as_array().unwrap().len(), 1);
    assert_eq!(detail["entries"][0]["counterparty"], "Bob");

    let list: Value = app.server.get("/api/finance/settlements").await.json();
    assert_eq!(list["items"][0]["outstanding_minor"], 30_00);
    assert!(list["items"][0].get("entries").is_none());

    app.cleanup().await;
}

#[tokio::test]
async fn deleting_settlement_cascades_entries() {
    let app = TestApp::new().await;
    app.register_and_login("heidi").await;

    let settlement = create_settlement(&app, "Bob", 50_00).await;
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (_, _) = add_entry(&app, &settlement_id, "payment", None, 10_00).await;

    app.server
        .delete(&format!("/api/finance/settlements/{settlement_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let orphans = app
        .db
        .collection::<mongodb::bson::Document>("finance_settlement_entries")
        .count_documents(mongodb::bson::doc! {})
        .await
        .unwrap();
    assert_eq!(orphans, 0);

    app.cleanup().await;
}

#[tokio::test]
async fn list_filters_by_category_including_descendants() {
    let app = TestApp::new().await;
    app.register_and_login("ivan").await;

    let parent: Value = app
        .server
        .post("/api/finance/categories")
        .json(&serde_json::json!({ "name": "Repairs" }))
        .await
        .json();
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child: Value = app
        .server
        .post("/api/finance/categories")
        .json(&serde_json::json!({ "name": "Windows", "parent_id": parent_id }))
        .await
        .json();
    let child_id = child["id"].as_str().unwrap().to_string();

    for (desc, cat) in [
        ("Window job", Some(child_id.as_str())),
        ("Roof job", Some(parent_id.as_str())),
        ("Beer", None),
    ] {
        let mut body = serde_json::json!({
            "counterparty": "Bob",
            "direction": "owed_to_me",
            "amount_minor": 10_00,
            "currency": "EUR",
            "description": desc,
            "opened_at": "2026-06-01",
        });
        if let Some(cat) = cat {
            body["category_id"] = serde_json::json!(cat);
        }
        app.server
            .post("/api/finance/settlements")
            .json(&body)
            .await
            .assert_status(StatusCode::CREATED);
    }

    let by_parent: Value = app
        .server
        .get("/api/finance/settlements")
        .add_query_param("category_id", &parent_id)
        .await
        .json();
    assert_eq!(by_parent["total"], 2);

    let by_child: Value = app
        .server
        .get("/api/finance/settlements")
        .add_query_param("category_id", &child_id)
        .await
        .json();
    assert_eq!(by_child["total"], 1);
    assert_eq!(by_child["items"][0]["description"], "Window job");

    let uncategorized: Value = app
        .server
        .get("/api/finance/settlements")
        .add_query_param("uncategorized", "true")
        .await
        .json();
    assert_eq!(uncategorized["total"], 1);
    assert_eq!(uncategorized["items"][0]["description"], "Beer");

    app.cleanup().await;
}
