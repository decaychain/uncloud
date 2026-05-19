//! Integration tests for the finance reconciliation flow.

mod common;

use common::TestApp;
use serde_json::Value;

async fn create_account(app: &TestApp, opening_minor: i64) -> String {
    let resp: Value = app
        .server
        .post("/api/finance/accounts")
        .json(&serde_json::json!({
            "name": "Checking",
            "account_type": "checking",
            "currency": "EUR",
            "opening_balance_minor": opening_minor,
        }))
        .await
        .json();
    resp["id"].as_str().unwrap().to_string()
}

async fn create_tx(app: &TestApp, account_id: &str, date: &str, amount_minor: i64, desc: &str) {
    app.server
        .post("/api/finance/transactions")
        .json(&serde_json::json!({
            "account_id": account_id,
            "date": date,
            "amount_minor": amount_minor,
            "description": desc,
        }))
        .await;
}

#[tokio::test]
async fn preview_computes_delta_against_history() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    let account_id = create_account(&app, 100_00).await; // 100.00 EUR opening
    create_tx(&app, &account_id, "2026-05-01", 50_00, "salary slice").await;
    create_tx(&app, &account_id, "2026-05-02", -10_00, "lunch").await;
    // Computed balance through 2026-05-02 = 100 + 50 - 10 = 140.00

    let preview: Value = app
        .server
        .post(&format!("/api/finance/accounts/{account_id}/reconcile/preview"))
        .json(&serde_json::json!({
            "on_date": "2026-05-02",
            "actual_balance_minor": 150_00,
        }))
        .await
        .json();
    assert_eq!(preview["computed_minor"], 140_00);
    assert_eq!(preview["actual_minor"], 150_00);
    assert_eq!(preview["delta_minor"], 10_00);

    app.cleanup().await;
}

#[tokio::test]
async fn apply_creates_snapshot_and_adjustment() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;
    let account_id = create_account(&app, 0).await;
    create_tx(&app, &account_id, "2026-05-01", 100_00, "deposit").await;

    let resp: Value = app
        .server
        .post(&format!("/api/finance/accounts/{account_id}/reconcile/apply"))
        .json(&serde_json::json!({
            "on_date": "2026-05-01",
            "actual_balance_minor": 105_00,
            "note": "bank fee not yet booked",
        }))
        .await
        .json();
    let snapshot_id = resp["id"].as_str().unwrap().to_string();
    assert_eq!(resp["actual_balance_minor"], 105_00);
    assert_eq!(resp["drift_minor"], 0);
    assert!(resp["adjustment_transaction_id"].as_str().is_some());

    // Balance after reconciliation matches the actual.
    let bal: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/balance"))
        .await
        .json();
    assert_eq!(bal["balance_minor"], 105_00);

    // Reconciliation adjustments are hidden from the default listing.
    let default_txns: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let default_items = default_txns["items"].as_array().unwrap();
    assert_eq!(default_items.len(), 1, "only the real deposit shows by default");

    // Opting in surfaces the adjustment alongside the deposit.
    let all_txns: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .add_query_param("include_reconciliations", "true")
        .await
        .json();
    let items = all_txns["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "deposit + adjustment");
    let adj = items
        .iter()
        .find(|t| t["description"].as_str().unwrap().starts_with("Reconciliation"))
        .expect("adjustment transaction not found");
    assert_eq!(adj["amount_minor"], 5_00);
    assert!(adj["source_snapshot_id"].as_str().is_some(), "snapshot link present");

    // Snapshot list shows the snapshot with zero drift.
    let snaps: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/snapshots"))
        .await
        .json();
    let arr = snaps.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], snapshot_id);
    assert_eq!(arr[0]["drift_minor"], 0);

    app.cleanup().await;
}

#[tokio::test]
async fn drift_detected_after_late_import_then_recompute_clears_it() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;
    let account_id = create_account(&app, 0).await;
    create_tx(&app, &account_id, "2026-05-01", 100_00, "deposit").await;

    let snapshot: Value = app
        .server
        .post(&format!("/api/finance/accounts/{account_id}/reconcile/apply"))
        .json(&serde_json::json!({
            "on_date": "2026-05-01",
            "actual_balance_minor": 105_00,
        }))
        .await
        .json();
    let snapshot_id = snapshot["id"].as_str().unwrap().to_string();

    // Late-arriving historical transaction increases the computed balance.
    create_tx(&app, &account_id, "2026-04-30", 7_00, "missed deposit").await;

    let snaps: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/snapshots"))
        .await
        .json();
    assert_eq!(snaps[0]["drift_minor"], -7_00, "actual stays; computed grew, so drift is negative");

    let recomputed: Value = app
        .server
        .post(&format!("/api/finance/snapshots/{snapshot_id}/recompute"))
        .await
        .json();
    assert_eq!(recomputed["drift_minor"], 0);

    // Balance matches the original actual snapshot value.
    let bal: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/balance"))
        .await
        .json();
    assert_eq!(bal["balance_minor"], 105_00);

    app.cleanup().await;
}

#[tokio::test]
async fn delete_snapshot_removes_its_adjustment() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;
    let account_id = create_account(&app, 0).await;
    create_tx(&app, &account_id, "2026-05-01", 100_00, "deposit").await;

    let snap: Value = app
        .server
        .post(&format!("/api/finance/accounts/{account_id}/reconcile/apply"))
        .json(&serde_json::json!({
            "on_date": "2026-05-01",
            "actual_balance_minor": 110_00,
        }))
        .await
        .json();
    let snapshot_id = snap["id"].as_str().unwrap().to_string();

    let del = app
        .server
        .delete(&format!("/api/finance/snapshots/{snapshot_id}"))
        .await;
    assert_eq!(del.status_code(), 204);

    // Adjustment is gone; balance reverts to pre-reconciliation.
    let bal: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/balance"))
        .await
        .json();
    assert_eq!(bal["balance_minor"], 100_00);
    let snaps: Value = app
        .server
        .get(&format!("/api/finance/accounts/{account_id}/snapshots"))
        .await
        .json();
    assert!(snaps.as_array().unwrap().is_empty());

    app.cleanup().await;
}
