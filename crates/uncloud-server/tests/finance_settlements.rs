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
