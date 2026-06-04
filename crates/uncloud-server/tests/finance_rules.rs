//! Integration tests for the categorization-rules surface and its
//! interaction with the CSV importer.

mod common;

use axum_test::multipart::{MultipartForm, Part};
use common::TestApp;
use serde_json::Value;

fn fixture_csv() -> &'static [u8] {
    b"Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;\
Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);Sammlerreferenz;\
Lastschrift Ursprungsbetrag;Auslagenersatz Ruecklastschrift;\
Beguenstigter/Zahlungspflichtiger;Kontonummer/IBAN;BIC;Betrag;Waehrung;Info\n\
DE12345;05.03.26;05.03.26;LASTSCHRIFT;Spotify monthly;;;;;;;Spotify AB;SE12345;SPKSDEFF;-9,99;EUR;Umsatz gebucht\n\
DE12345;06.03.26;06.03.26;GUTSCHRIFT;Salary March;;;;;;;Acme GmbH;DE98765;ACMEDEFF;3.500,00;EUR;Umsatz gebucht\n"
}

async fn create_account(app: &TestApp) -> String {
    let r: Value = app
        .server
        .post("/api/finance/accounts")
        .json(&serde_json::json!({
            "name": "Sparkasse",
            "account_type": "checking",
            "currency": "EUR",
        }))
        .await
        .json();
    r["id"].as_str().unwrap().to_string()
}

async fn create_category(app: &TestApp, name: &str) -> String {
    let r: Value = app
        .server
        .post("/api/finance/categories")
        .json(&serde_json::json!({ "name": name }))
        .await
        .json();
    r["id"].as_str().unwrap().to_string()
}

async fn sparkasse_schema_id(app: &TestApp) -> String {
    let r: Value = app.server.get("/api/finance/import-schemas").await.json();
    let arr = r.as_array().unwrap();
    arr.iter()
        .find(|s| s["name"] == "Sparkasse CAMT V8")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn import_form(account_id: &str, schema_id: &str, csv: &[u8]) -> MultipartForm {
    MultipartForm::new()
        .add_part("account_id", Part::text(account_id.to_string()))
        .add_part("schema_id", Part::text(schema_id.to_string()))
        .add_part(
            "csv",
            Part::bytes(csv.to_vec())
                .file_name("t.csv")
                .mime_type("text/csv"),
        )
}

#[tokio::test]
async fn import_tags_rows_with_matching_rules() {
    let app = TestApp::new().await;
    app.register_and_login("alice").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;
    let music = create_category(&app, "Music").await;

    let rule: Value = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Spotify",
            "pattern": "spotify",
            "pattern_kind": "substring",
            "case_insensitive": true,
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await
        .json();
    let rule_id = rule["id"].as_str().unwrap().to_string();

    let _: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    let listing: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let items = listing["items"].as_array().unwrap();
    let spotify = items
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Spotify"))
        .expect("Spotify transaction not found");
    assert_eq!(spotify["category_id"], music);
    let salary = items
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Acme"))
        .expect("Salary transaction not found");
    assert!(salary["category_id"].is_null());

    // Sanity: rule list shows our entry.
    let rules: Value = app.server.get("/api/finance/rules").await.json();
    let arr = rules.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], rule_id);

    app.cleanup().await;
}

#[tokio::test]
async fn wildcard_rule_matches_imported_rows() {
    let app = TestApp::new().await;
    app.register_and_login("alice-wildcard").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;
    let music = create_category(&app, "Music").await;

    let _: Value = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Spotify wildcard",
            "pattern": "Spotify*monthly",
            "pattern_kind": "wildcard",
            "case_insensitive": true,
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await
        .json();

    let _: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    let listing: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let items = listing["items"].as_array().unwrap();
    let spotify = items
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Spotify"))
        .expect("Spotify transaction not found");
    assert_eq!(spotify["category_id"], music);

    app.cleanup().await;
}

#[tokio::test]
async fn apply_rules_categorizes_existing_rows_but_not_user_set_ones() {
    let app = TestApp::new().await;
    app.register_and_login("bob").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;
    let music = create_category(&app, "Music").await;
    let income = create_category(&app, "Income").await;

    // Import first — no rules yet, so everything lands unset.
    let _: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    // Manually categorize the salary row so we can prove rules don't
    // touch user-set categories.
    let pre: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let salary_id = pre["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Acme"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    app.server
        .put(&format!("/api/finance/transactions/{salary_id}"))
        .json(&serde_json::json!({ "category_id": income }))
        .await;

    // Now add a Spotify rule + a "Salary" rule that would also match the
    // user-categorized row if rules ignored category_source = user.
    app.server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Spotify",
            "pattern": "spotify",
            "pattern_kind": "substring",
            "category_id": music,
            "priority": 10,
            "enabled": true,
        }))
        .await;
    app.server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Salary",
            "pattern": "salary",
            "pattern_kind": "substring",
            "category_id": music, // intentionally "wrong" — user choice wins
            "priority": 5,
            "enabled": true,
        }))
        .await;

    let apply: Value = app.server.post("/api/finance/rules/apply").await.json();
    assert_eq!(apply["updated"], 1, "only Spotify should retag");
    let apply_again: Value = app.server.post("/api/finance/rules/apply").await.json();
    assert_eq!(
        apply_again["updated"], 0,
        "reapplying unchanged rules should not count already-current rows",
    );

    let after: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let items = after["items"].as_array().unwrap();
    let salary = items.iter().find(|t| t["id"] == salary_id).unwrap();
    assert_eq!(salary["category_id"], income, "user category preserved");
    let spotify = items
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Spotify"))
        .unwrap();
    assert_eq!(spotify["category_id"], music);
    let spotify_updated_at = spotify["updated_at"].clone();

    // A higher-priority rule that maps the same transaction to the same
    // category may update internal rule ownership, but it must not count
    // as a visible transaction update or bump updated_at.
    app.server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Spotify duplicate",
            "pattern": "spotify",
            "pattern_kind": "substring",
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await;
    let apply_same_category: Value = app.server.post("/api/finance/rules/apply").await.json();
    assert_eq!(
        apply_same_category["updated"], 0,
        "same-category rule metadata changes should not count as transaction updates",
    );
    let after_same_category: Value = app
        .server
        .get("/api/finance/transactions")
        .add_query_param("account_id", &account_id)
        .await
        .json();
    let spotify_after_same_category = after_same_category["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains("Spotify"))
        .unwrap();
    assert_eq!(spotify_after_same_category["category_id"], music);
    assert_eq!(
        spotify_after_same_category["updated_at"],
        spotify_updated_at
    );

    app.cleanup().await;
}

#[tokio::test]
async fn rules_can_be_reordered_without_numeric_priorities() {
    let app = TestApp::new().await;
    app.register_and_login("erin").await;
    let music = create_category(&app, "Music").await;

    let first: Value = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "First",
            "pattern": "first",
            "pattern_kind": "substring",
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await
        .json();
    let second: Value = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Second",
            "pattern": "second",
            "pattern_kind": "substring",
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await
        .json();
    let third: Value = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "Third",
            "pattern": "third",
            "pattern_kind": "substring",
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await
        .json();

    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    let third_id = third["id"].as_str().unwrap();
    app.server
        .put("/api/finance/rules/reorder")
        .json(&serde_json::json!({
            "rule_ids": [third_id, first_id, second_id],
        }))
        .await;

    let rules: Value = app.server.get("/api/finance/rules").await.json();
    let names: Vec<&str> = rules
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Third", "First", "Second"]);

    app.cleanup().await;
}

#[tokio::test]
async fn test_endpoint_returns_matches_without_writing() {
    let app = TestApp::new().await;
    app.register_and_login("carol").await;
    let account_id = create_account(&app).await;
    let schema_id = sparkasse_schema_id(&app).await;
    let _: Value = app
        .server
        .post("/api/finance/import")
        .multipart(import_form(&account_id, &schema_id, fixture_csv()))
        .await
        .json();

    let preview: Value = app
        .server
        .post("/api/finance/rules/test")
        .json(&serde_json::json!({
            "pattern": "spotify",
            "pattern_kind": "substring",
            "case_insensitive": true,
        }))
        .await
        .json();
    assert_eq!(preview["sampled"], 2);
    let matches = preview["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0]["description"]
        .as_str()
        .unwrap()
        .contains("Spotify"));

    app.cleanup().await;
}

#[tokio::test]
async fn bad_regex_returns_400() {
    let app = TestApp::new().await;
    app.register_and_login("dave").await;
    let music = create_category(&app, "Music").await;
    let resp = app
        .server
        .post("/api/finance/rules")
        .json(&serde_json::json!({
            "name": "broken",
            "pattern": "(unterminated",
            "pattern_kind": "regex",
            "category_id": music,
            "priority": 0,
            "enabled": true,
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
    app.cleanup().await;
}
