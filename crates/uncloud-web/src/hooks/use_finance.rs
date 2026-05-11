//! HTTP wrappers for the finance tracker REST surface.

use uncloud_common::{
    AccountBalanceResponse, AccountResponse, CreateAccountRequest, CreateFinanceCategoryRequest,
    CreateTransactionRequest, FinanceCategoryResponse, TransactionListResponse, TransactionResponse,
    UpdateAccountRequest, UpdateFinanceCategoryRequest, UpdateTransactionRequest,
};

use super::api;

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
