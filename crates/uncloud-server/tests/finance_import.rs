//! Integration tests for the finance CSV import endpoint and import schemas.

mod common;

use axum_test::multipart::{MultipartForm, Part};
use common::TestApp;
use serde_json::Value;

fn fixture_csv() -> &'static [u8] {
    // Minimal Sparkasse CAMT V8 fixture — 17 columns + 2 rows.
    b"Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;\
Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);Sammlerreferenz;\
Lastschrift Ursprungsbetrag;Auslagenersatz Ruecklastschrift;\
Beguenstigter/Zahlungspflichtiger;Kontonummer/IBAN;BIC;Betrag;Waehrung;Info\n\
DE12345;05.03.26;05.03.26;LASTSCHRIFT;Spotify monthly;;;;;;;Spotify AB;SE12345;SPKSDEFF;-9,99;EUR;Umsatz gebucht\n\
DE12345;06.03.26;06.03.26;GUTSCHRIFT;Salary March;;;;;;;Acme GmbH;DE98765;ACMEDEFF;3.500,00;EUR;Umsatz gebucht\n"
}

async fn create_account(app: &TestApp) -> String {
    let resp: Value = app
        .server
        .post("/api/finance/accounts")
        .json(&serde_json::json!({
            "name": "Sparkasse Giro",
            "account_type": "checking",
            "currency": "EUR",
        }))
        .await
        .json();
    resp["id"].as_str().unwrap().to_string()
}

/// Fetch the seeded "Sparkasse CAMT V8" schema for the logged-in user.
/// The first call to `GET /finance/import-schemas` seeds the builtin
/// schema for that user.
async fn sparkasse_schema_id(app: &TestApp) -> String {
    let resp: Value = app.server.get("/api/finance/import-schemas").await.json();
    let arr = resp.as_array().expect("schemas array");
    let s = arr
        .iter()
        .find(|s| s["name"] == "Sparkasse CAMT V8")
        .expect("builtin Sparkasse schema not seeded");
    s["id"].as_str().unwrap().to_string()
}

fn import_form(account_id: &str, schema_id: &str, csv: &[u8]) -> MultipartForm {
    MultipartForm::new()
        .add_part("account_id", Part::text(account_id.to_string()))
        .add_part("schema_id", Part::text(schema_id.to_string()))
        .add_part(
            "csv",
            Part::bytes(csv.to_vec())
                .file_name("test.csv")
                .mime_type("text/csv"),
        )
}

#[tokio::test]
async fn imports_two_rows_then_dedups_on_reimport() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;

    let first: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    assert_eq!(first["imported"], 2);
    assert_eq!(first["skipped"], 0);
    assert_eq!(first["errors"], 0);
    assert!(first["run_id"].as_str().is_some(), "run_id should be returned");

    // Same CSV re-uploaded: every row is a duplicate now.
    let second: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    assert_eq!(second["imported"], 0);
    assert_eq!(second["skipped"], 2);
    assert_eq!(second["errors"], 0);

    let listing: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    assert_eq!(listing["total"], 2);

    app.cleanup().await;
}

#[tokio::test]
async fn unknown_schema_id_is_404() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;
    let account_id = create_account(&app).await;

    let resp = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(
            &account_id,
            "507f1f77bcf86cd799439099",
            fixture_csv(),
        ))
        .await;

    assert_eq!(resp.status_code(), 404);
    app.cleanup().await;
}

#[tokio::test]
async fn lists_and_seeds_builtin_schemas() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;

    let resp: Value = app.server.get("/api/finance/import-schemas").await.json();
    let arr = resp.as_array().expect("array");
    assert!(arr.iter().any(|s| s["name"] == "Sparkasse CAMT V8"
        && s["is_builtin"] == true));
    app.cleanup().await;
}

#[tokio::test]
async fn clone_makes_editable_copy() {
    let app = TestApp::new().await;
    app.register_and_login("eve").await;

    let id = sparkasse_schema_id(&app).await;
    let cloned: Value = app
        .server
        .post(&format!("/api/finance/import-schemas/{id}/clone"))
        .await
        .json();
    assert_eq!(cloned["is_builtin"], false);
    assert_eq!(cloned["name"], "Sparkasse CAMT V8 (copy)");

    // Editing the builtin should be rejected.
    let edit_builtin = app
        .server
        .put(&format!("/api/finance/import-schemas/{id}"))
        .json(&serde_json::json!({
            "name": "Mutated",
            "delimiter": ";",
            "encoding": "windows-1252",
            "decimal_separator": "comma",
            "skip_header_rows": 0,
            "has_headers": true,
            "date_column": 1,
            "date_format": "DD.MM.YY",
            "amount_column": 14,
            "amount_sign_convention": "positive_credit",
            "description_columns": [11, 4],
            "currency_source": "column",
            "currency_column": 15,
        }))
        .await;
    assert_eq!(edit_builtin.status_code(), 400);

    // Editing the clone should succeed.
    let clone_id = cloned["id"].as_str().unwrap();
    let edited: Value = app
        .server
        .put(&format!("/api/finance/import-schemas/{clone_id}"))
        .json(&serde_json::json!({
            "name": "Sparkasse, my edits",
            "delimiter": ";",
            "encoding": "windows-1252",
            "decimal_separator": "comma",
            "skip_header_rows": 0,
            "has_headers": true,
            "date_column": 1,
            "date_format": "DD.MM.YY",
            "amount_column": 14,
            "amount_sign_convention": "positive_credit",
            "description_columns": [11, 4],
            "currency_source": "column",
            "currency_column": 15,
        }))
        .await
        .json();
    assert_eq!(edited["name"], "Sparkasse, my edits");

    app.cleanup().await;
}

#[tokio::test]
async fn import_requires_auth() {
    let app = TestApp::new().await;

    let resp = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(
            "507f1f77bcf86cd799439011",
            "507f1f77bcf86cd799439012",
            fixture_csv(),
        ))
        .await;

    assert_eq!(resp.status_code(), 401);
    app.cleanup().await;
}

#[tokio::test]
async fn import_creates_run_and_revert_deletes_transactions() {
    let app = TestApp::new().await;
    app.register_and_login("frank").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;

    let import: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();
    let run_id = import["run_id"].as_str().expect("run_id").to_string();

    let runs: Value = app.server.get("/api/finance/imports").await.json();
    let arr = runs.as_array().expect("runs array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], run_id);
    assert_eq!(arr[0]["status"], "applied");
    assert_eq!(arr[0]["summary"]["created"], 2);
    assert_eq!(arr[0]["source"]["kind"], "upload");

    let listing_before: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    assert_eq!(listing_before["total"], 2);

    let reverted: Value = app
        .server
        .post(&format!("/api/finance/imports/{run_id}/revert"))
        .await
        .json();
    assert_eq!(reverted["status"], "reverted");
    assert!(reverted["reverted_at"].as_str().is_some());

    let listing_after: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    assert_eq!(listing_after["total"], 0);

    // Reverting a run twice is rejected.
    let twice = app
        .server
        .post(&format!("/api/finance/imports/{run_id}/revert"))
        .await;
    assert_eq!(twice.status_code(), 400);

    app.cleanup().await;
}

#[tokio::test]
async fn revert_does_not_touch_other_users_transactions() {
    let app = TestApp::new().await;
    app.register_and_login("grace").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;
    let import: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();
    let run_id = import["run_id"].as_str().unwrap().to_string();

    // Different user can't revert someone else's run — should 404.
    app.register_and_login("heidi").await;
    let resp = app
        .server
        .post(&format!("/api/finance/imports/{run_id}/revert"))
        .await;
    assert_eq!(resp.status_code(), 404);

    app.cleanup().await;
}
