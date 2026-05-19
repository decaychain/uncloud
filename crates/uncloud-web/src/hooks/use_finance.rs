//! HTTP wrappers for the finance tracker REST surface.

use uncloud_common::{
    AccountBalanceResponse, AccountResponse, ApplyRulesResponse, BalanceSnapshotResponse,
    CreateAccountRequest, CreateFinanceCategoryRequest, CreateTransactionRequest,
    FinanceCategoryResponse, FinanceRuleRequest, FinanceRuleResponse, ImportCsvResponse,
    ImportRunResponse, ImportSchemaRequest, ImportSchemaResponse, ReconcilePreviewResponse,
    ReconcileRequest, TestRuleRequest, TestRuleResponse, TransactionListResponse,
    TransactionResponse, UpdateAccountRequest, UpdateFinanceCategoryRequest,
    UpdateTransactionRequest,
};

use super::api;
use super::api::api_url;

// ── Accounts ────────────────────────────────────────────────────────────

pub async fn list_accounts() -> Result<Vec<AccountResponse>, String> {
    let r = api::get("/finance/accounts").send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<AccountResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load accounts".to_string())
    }
}

pub async fn create_account(req: &CreateAccountRequest) -> Result<AccountResponse, String> {
    let r = api::post("/finance/accounts")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<AccountResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_account(id: &str, req: &UpdateAccountRequest) -> Result<AccountResponse, String> {
    let r = api::put(&format!("/finance/accounts/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<AccountResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_account(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/accounts/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn account_balance(id: &str) -> Result<AccountBalanceResponse, String> {
    let r = api::get(&format!("/finance/accounts/{}/balance", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<AccountBalanceResponse>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load balance".to_string())
    }
}

// ── Categories ──────────────────────────────────────────────────────────

pub async fn list_categories() -> Result<Vec<FinanceCategoryResponse>, String> {
    let r = api::get("/finance/categories").send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<FinanceCategoryResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load categories".to_string())
    }
}

pub async fn create_category(req: &CreateFinanceCategoryRequest) -> Result<FinanceCategoryResponse, String> {
    let r = api::post("/finance/categories")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<FinanceCategoryResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_category(id: &str, req: &UpdateFinanceCategoryRequest) -> Result<FinanceCategoryResponse, String> {
    let r = api::put(&format!("/finance/categories/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<FinanceCategoryResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_category(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/categories/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

// ── Transactions ────────────────────────────────────────────────────────

pub async fn list_transactions(
    account_id: Option<&str>,
    uncategorized: bool,
    from: Option<&str>,
    to: Option<&str>,
    include_reconciliations: bool,
    limit: u32,
    skip: u32,
) -> Result<TransactionListResponse, String> {
    let mut url = format!("/finance/transactions?limit={}&skip={}", limit, skip);
    if let Some(a) = account_id {
        url.push_str(&format!("&account_id={}", a));
    }
    if uncategorized {
        url.push_str("&uncategorized=true");
    }
    if let Some(f) = from {
        url.push_str(&format!("&from={}", f));
    }
    if let Some(t) = to {
        url.push_str(&format!("&to={}", t));
    }
    if include_reconciliations {
        url.push_str("&include_reconciliations=true");
    }
    let r = api::get(&url).send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<TransactionListResponse>().await.map_err(|e| e.to_string())
    } else {
        Err("Failed to load transactions".to_string())
    }
}

pub async fn create_transaction(req: &CreateTransactionRequest) -> Result<TransactionResponse, String> {
    let r = api::post("/finance/transactions")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 201 {
        r.json::<TransactionResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_transaction(id: &str, req: &UpdateTransactionRequest) -> Result<TransactionResponse, String> {
    let r = api::put(&format!("/finance/transactions/{}", id))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<TransactionResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_transaction(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/transactions/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() || r.status() == 204 {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

async fn extract_error(r: gloo_net::http::Response) -> String {
    let status = r.status();
    match r.text().await {
        Ok(t) if !t.is_empty() => t,
        _ => format!("HTTP {}", status),
    }
}

// ── CSV import ──────────────────────────────────────────────────────────

pub async fn list_import_schemas() -> Result<Vec<ImportSchemaResponse>, String> {
    let r = api::get("/finance/import-schemas")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<ImportSchemaResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn create_import_schema(
    req: &ImportSchemaRequest,
) -> Result<ImportSchemaResponse, String> {
    let r = api::post("/finance/import-schemas")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ImportSchemaResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_import_schema(
    id: &str,
    req: &ImportSchemaRequest,
) -> Result<ImportSchemaResponse, String> {
    let r = api::put(&format!("/finance/import-schemas/{id}"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ImportSchemaResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_import_schema(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/import-schemas/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        Ok(())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn clone_import_schema(id: &str) -> Result<ImportSchemaResponse, String> {
    let r = api::post(&format!("/finance/import-schemas/{id}/clone"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ImportSchemaResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn import_csv(
    account_id: &str,
    schema_id: &str,
    file_name: &str,
    csv_bytes: Vec<u8>,
) -> Result<ImportCsvResponse, String> {
    let blob_parts = js_sys::Array::new();
    let bytes_array = js_sys::Uint8Array::from(csv_bytes.as_slice());
    blob_parts.push(&bytes_array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/csv");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &opts)
        .map_err(|_| "Failed to create Blob".to_string())?;

    let form = web_sys::FormData::new().map_err(|_| "Failed to create FormData".to_string())?;
    if !account_id.is_empty() {
        form.append_with_str("account_id", account_id)
            .map_err(|_| "Failed to append account_id".to_string())?;
    }
    form.append_with_str("schema_id", schema_id)
        .map_err(|_| "Failed to append schema_id".to_string())?;
    form.append_with_blob_and_filename("csv", &blob, file_name)
        .map_err(|_| "Failed to append csv".to_string())?;

    let url = api_url("/finance/import");
    let resp = api::post_raw(&url)
        .body(form)
        .map_err(|e| format!("Request error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if resp.ok() {
        resp.json::<ImportCsvResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(resp).await)
    }
}

pub async fn list_import_runs() -> Result<Vec<ImportRunResponse>, String> {
    let r = api::get("/finance/imports")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<ImportRunResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn revert_import_run(id: &str) -> Result<ImportRunResponse, String> {
    let r = api::post(&format!("/finance/imports/{id}/revert"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ImportRunResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn reconcile_preview(
    account_id: &str,
    req: &ReconcileRequest,
) -> Result<ReconcilePreviewResponse, String> {
    let r = api::post(&format!("/finance/accounts/{account_id}/reconcile/preview"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ReconcilePreviewResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn reconcile_apply(
    account_id: &str,
    req: &ReconcileRequest,
) -> Result<BalanceSnapshotResponse, String> {
    let r = api::post(&format!("/finance/accounts/{account_id}/reconcile/apply"))
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<BalanceSnapshotResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn list_account_snapshots(
    account_id: &str,
) -> Result<Vec<BalanceSnapshotResponse>, String> {
    let r = api::get(&format!("/finance/accounts/{account_id}/snapshots"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<BalanceSnapshotResponse>>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn recompute_snapshot(id: &str) -> Result<BalanceSnapshotResponse, String> {
    let r = api::post(&format!("/finance/snapshots/{id}/recompute"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<BalanceSnapshotResponse>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_snapshot(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/snapshots/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.ok() { Ok(()) } else { Err(extract_error(r).await) }
}

pub async fn list_rules() -> Result<Vec<FinanceRuleResponse>, String> {
    let r = api::get("/finance/rules").send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<Vec<FinanceRuleResponse>>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn create_rule(req: &FinanceRuleRequest) -> Result<FinanceRuleResponse, String> {
    let r = api::post("/finance/rules")
        .json(req).map_err(|e| e.to_string())?
        .send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<FinanceRuleResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn update_rule(id: &str, req: &FinanceRuleRequest) -> Result<FinanceRuleResponse, String> {
    let r = api::put(&format!("/finance/rules/{id}"))
        .json(req).map_err(|e| e.to_string())?
        .send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<FinanceRuleResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn delete_rule(id: &str) -> Result<(), String> {
    let r = api::delete(&format!("/finance/rules/{id}"))
        .send().await.map_err(|e| e.to_string())?;
    if r.ok() { Ok(()) } else { Err(extract_error(r).await) }
}

pub async fn apply_rules() -> Result<ApplyRulesResponse, String> {
    let r = api::post("/finance/rules/apply").send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<ApplyRulesResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}

pub async fn test_rule(req: &TestRuleRequest) -> Result<TestRuleResponse, String> {
    let r = api::post("/finance/rules/test")
        .json(req).map_err(|e| e.to_string())?
        .send().await.map_err(|e| e.to_string())?;
    if r.ok() {
        r.json::<TestRuleResponse>().await.map_err(|e| e.to_string())
    } else {
        Err(extract_error(r).await)
    }
}
