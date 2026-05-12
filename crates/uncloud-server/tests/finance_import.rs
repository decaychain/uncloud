//! Integration tests for the finance CSV import endpoint.

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

fn import_form(account_id: &str, profile_id: &str, csv: &[u8]) -> MultipartForm {
    MultipartForm::new()
        .add_part("account_id", Part::text(account_id.to_string()))
        .add_part("profile_id", Part::text(profile_id.to_string()))
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

    let first: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, "sparkasse_camt_v8", fixture_csv()))
        .await
        .json();

    assert_eq!(first["imported"], 2);
    assert_eq!(first["skipped"], 0);
    assert_eq!(first["errors"], 0);

    // Same CSV re-uploaded: every row is a duplicate now.
    let second: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, "sparkasse_camt_v8", fixture_csv()))
        .await
        .json();

    assert_eq!(second["imported"], 0);
    assert_eq!(second["skipped"], 2);
    assert_eq!(second["errors"], 0);

    // Transaction list shows exactly the 2 rows from the first import.
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
async fn rejects_non_sparkasse_csv_with_400() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;
    let account_id = create_account(&app).await;

    let resp = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(
            &account_id,
            "sparkasse_camt_v8",
            b"a;b;c\n1;2;3\n",
        ))
        .await;

    assert_eq!(resp.status_code(), 400);
    app.cleanup().await;
}

#[tokio::test]
async fn unknown_profile_id_is_400() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;
    let account_id = create_account(&app).await;

    let resp = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, "nope_v1", fixture_csv()))
        .await;

    assert_eq!(resp.status_code(), 400);
    app.cleanup().await;
}

#[tokio::test]
async fn lists_available_profiles() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;

    let resp: Value = app.server.get("/api/finance/import/profiles").await.json();
    let arr = resp.as_array().expect("array");
    assert!(arr.iter().any(|p| p["id"] == "sparkasse_camt_v8"));

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
            "sparkasse_camt_v8",
            fixture_csv(),
        ))
        .await;

    assert_eq!(resp.status_code(), 401);
    app.cleanup().await;
}
