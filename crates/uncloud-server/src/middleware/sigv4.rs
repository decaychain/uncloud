//! AWS Signature V4 verification middleware for the S3-compatible API.
//!
//! Parses the `Authorization: AWS4-HMAC-SHA256 Credential=…, SignedHeaders=…, Signature=…`
//! header, reconstructs the canonical request, derives the signing key, and compares
//! the computed signature against the one in the header.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use mongodb::bson::doc;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::models::{S3Credential, User};
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// The authenticated S3 user, analogous to `AuthUser`.
#[derive(Clone)]
pub struct S3User(pub User);

impl std::ops::Deref for S3User {
    type Target = User;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> axum::extract::FromRequestParts<S> for S3User
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<S3User>()
            .cloned()
            .ok_or((StatusCode::FORBIDDEN, "S3 authentication required"))
    }
}

/// Parsed components from the Authorization header.
struct AuthParsed {
    access_key: String,
    date: String, // YYYYMMDD
    region: String,
    service: String,
    signed_headers: Vec<String>,
    signature: String,
}

fn parse_auth_header(header: &str) -> Option<AuthParsed> {
    let header = header.strip_prefix("AWS4-HMAC-SHA256 ")?;

    let mut credential = None;
    let mut signed_headers_raw = None;
    let mut signature = None;

    for part in header.split(", ") {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("Credential=") {
            credential = Some(val.to_string());
        } else if let Some(val) = part.strip_prefix("SignedHeaders=") {
            signed_headers_raw = Some(val.to_string());
        } else if let Some(val) = part.strip_prefix("Signature=") {
            signature = Some(val.to_string());
        }
    }

    let credential = credential?;
    let signed_headers_raw = signed_headers_raw?;
    let signature = signature?;

    // Credential = access_key/date/region/service/aws4_request
    let cred_parts: Vec<&str> = credential.splitn(5, '/').collect();
    if cred_parts.len() != 5 || cred_parts[4] != "aws4_request" {
        return None;
    }

    let signed_headers: Vec<String> = signed_headers_raw
        .split(';')
        .map(|s| s.to_string())
        .collect();

    Some(AuthParsed {
        access_key: cred_parts[0].to_string(),
        date: cred_parts[1].to_string(),
        region: cred_parts[2].to_string(),
        service: cred_parts[3].to_string(),
        signed_headers,
        signature,
    })
}

/// Build the canonical query string: sort parameters by key then value.
fn canonical_query_string(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let mut params: Vec<(String, String)> = raw
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let mut it = pair.splitn(2, '=');
            let key = it.next().unwrap_or("").to_string();
            let val = it.next().unwrap_or("").to_string();
            (key, val)
        })
        .collect();
    params.sort();
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode a string per AWS SigV4 rules: encode everything except
/// unreserved characters (A-Z, a-z, 0-9, '-', '_', '.', '~').
fn encode_uri_component(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Build the canonical URI: URI-decode the raw path, then re-encode each
/// path segment individually per AWS SigV4 spec.
fn canonical_uri(raw_path: &str) -> String {
    if raw_path.is_empty() || raw_path == "/" {
        return "/".to_string();
    }

    // URI-decode the raw path first
    let decoded = urlencoding::decode(raw_path)
        .unwrap_or_else(|_| std::borrow::Cow::Borrowed(raw_path));

    // Re-encode each segment individually, preserving the '/' separators
    let segments: Vec<String> = decoded
        .split('/')
        .map(|seg| encode_uri_component(seg))
        .collect();

    let result = segments.join("/");
    if result.is_empty() {
        "/".to_string()
    } else {
        result
    }
}

/// Build canonical headers string from request headers and signed header list.
fn canonical_headers(
    headers: &axum::http::HeaderMap,
    signed_headers: &[String],
) -> (String, String) {
    let mut header_map: BTreeMap<String, String> = BTreeMap::new();

    for name in signed_headers {
        let lower = name.to_lowercase();
        if let Some(value) = headers.get(lower.as_str()) {
            let val = value.to_str().unwrap_or("").trim().to_string();
            header_map
                .entry(lower)
                .and_modify(|existing| {
                    existing.push(',');
                    existing.push_str(&val);
                })
                .or_insert(val);
        }
    }

    let canonical = header_map
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v))
        .collect::<String>();

    let signed = header_map
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(";");

    (canonical, signed)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Derive the signing key from the raw secret access key.
fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{}", secret).as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

pub async fn sigv4_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    // Extract Authorization header
    let auth_header = match request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
    {
        Some(h) if h.starts_with("AWS4-HMAC-SHA256 ") => h.to_string(),
        _ => {
            return s3_error_response(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "Missing or invalid Authorization header",
            );
        }
    };

    let parsed = match parse_auth_header(&auth_header) {
        Some(p) => p,
        None => {
            return s3_error_response(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "Malformed Authorization header",
            );
        }
    };

    // Look up the S3 credential by access_key_id
    let creds_coll = state.db.collection::<S3Credential>("s3_credentials");
    let credential = match creds_coll
        .find_one(doc! { "access_key_id": &parsed.access_key })
        .await
    {
        Ok(Some(c)) => c,
        _ => {
            return s3_error_response(
                StatusCode::FORBIDDEN,
                "InvalidAccessKeyId",
                "The AWS access key ID you provided does not exist in our records",
            );
        }
    };

    // Look up the user
    let user = match state.auth.get_user_by_id(credential.user_id).await {
        Ok(Some(u)) => u,
        _ => {
            return s3_error_response(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "User not found for this access key",
            );
        }
    };

    // Get payload hash from x-amz-content-sha256 (most clients send UNSIGNED-PAYLOAD)
    let payload_hash = request
        .headers()
        .get("x-amz-content-sha256")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("UNSIGNED-PAYLOAD")
        .to_string();

    // Build canonical request
    let method = request.method().as_str().to_string();
    let uri = request.uri().clone();
    let canon_path = canonical_uri(uri.path());
    let query = uri.query().unwrap_or("").to_string();

    let canon_qs = canonical_query_string(&query);
    let (canon_headers, signed_headers_str) =
        canonical_headers(request.headers(), &parsed.signed_headers);

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method, canon_path, canon_qs, canon_headers, signed_headers_str, payload_hash
    );

    let canonical_request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));

    // Get the x-amz-date header for the timestamp
    let amz_date = request
        .headers()
        .get("x-amz-date")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    // String to sign
    let credential_scope = format!(
        "{}/{}/{}/aws4_request",
        parsed.date, parsed.region, parsed.service
    );

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date, credential_scope, canonical_request_hash
    );

    // Derive signing key using raw secret and verify
    let signing_key = derive_signing_key(
        &credential.secret_access_key,
        &parsed.date,
        &parsed.region,
        &parsed.service,
    );
    let computed_signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    if computed_signature.as_bytes().ct_eq(parsed.signature.as_bytes()).unwrap_u8() == 0 {
        return s3_error_response(
            StatusCode::FORBIDDEN,
            "SignatureDoesNotMatch",
            "The request signature we calculated does not match the signature you provided",
        );
    }

    request.extensions_mut().insert(S3User(user));
    next.run(request).await
}

/// XML-escape a string for safe inclusion in XML output.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn s3_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Error>
  <Code>{}</Code>
  <Message>{}</Message>
</Error>"#,
        xml_escape(code),
        xml_escape(message),
    );

    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/xml".to_string(),
        )],
        body,
    )
        .into_response()
}
