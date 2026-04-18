//! Cross-platform credential storage for the desktop app.
//!
//! Tries the OS keyring first (Secret Service on Linux, Keychain on macOS,
//! Credential Manager on Windows). When the keyring is unavailable —
//! headless Linux, locked container, no Secret Service running — falls back
//! to an AES-GCM-encrypted file in the app data dir, using a key embedded
//! at build time (see `build.rs`). The fallback is weak security by design:
//! it stops casual file inspection but anyone who can run our binary can
//! extract the key. The longer-term plan is server-issued bearer tokens.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rand::RngCore;

include!(concat!(env!("OUT_DIR"), "/fallback_key.rs"));

/// Service name used in the OS keyring. Dev builds use a different name so
/// they never collide with a release install on the same machine.
fn service_name() -> &'static str {
    if cfg!(debug_assertions) {
        "uncloud-dev"
    } else {
        "uncloud"
    }
}

/// Store the password for `(server_url, username)`. Tries keyring first,
/// falls back to the encrypted file on error.
pub fn store_password(
    fallback_dir: &Path,
    server_url: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let account = account_key(server_url, username);
    if let Err(e) = keyring_set(&account, password) {
        tracing::warn!("Keyring unavailable, using encrypted file fallback: {e}");
        write_fallback_file(fallback_dir, &account, password)?;
    } else {
        // Best-effort: scrub any stale fallback file so the keyring is the
        // single source of truth from now on.
        let _ = std::fs::remove_file(fallback_path(fallback_dir, &account));
    }
    Ok(())
}

/// Load the password for `(server_url, username)`. Tries keyring first,
/// then the encrypted file fallback.
pub fn load_password(
    fallback_dir: &Path,
    server_url: &str,
    username: &str,
) -> Option<String> {
    let account = account_key(server_url, username);
    match keyring_get(&account) {
        Ok(Some(pw)) => Some(pw),
        Ok(None) | Err(_) => read_fallback_file(fallback_dir, &account).ok(),
    }
}

/// Remove the password from both keyring and fallback file. Idempotent —
/// errors are swallowed (this is called from `disconnect`).
pub fn delete_password(fallback_dir: &Path, server_url: &str, username: &str) {
    let account = account_key(server_url, username);
    let _ = keyring_delete(&account);
    let _ = std::fs::remove_file(fallback_path(fallback_dir, &account));
}

fn account_key(server_url: &str, username: &str) -> String {
    format!("{server_url}::{username}")
}

// ── Keyring ──────────────────────────────────────────────────────────────────

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn keyring_set(account: &str, password: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(service_name(), account).map_err(|e| e.to_string())?;
    entry.set_password(password).map_err(|e| e.to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn keyring_get(account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(service_name(), account).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn keyring_delete(account: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(service_name(), account).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn keyring_set(_account: &str, _password: &str) -> Result<(), String> {
    Err("keyring not supported on this platform".to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn keyring_get(_account: &str) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn keyring_delete(_account: &str) -> Result<(), String> {
    Ok(())
}

// ── Encrypted file fallback ──────────────────────────────────────────────────

fn fallback_path(dir: &Path, account: &str) -> PathBuf {
    // Hash the account into a short filename to avoid awkward characters
    // (`::`, `/`, `?`) on disk.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    account.hash(&mut hasher);
    dir.join(format!("cred-{:016x}.bin", hasher.finish()))
}

fn write_fallback_file(dir: &Path, account: &str, password: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;

    let cipher = Aes256Gcm::new_from_slice(&FALLBACK_KEY)
        .map_err(|e| format!("invalid key length: {e}"))?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, password.as_bytes())
        .map_err(|e| format!("encrypt: {e}"))?;

    // File layout: base64( 12-byte nonce || ciphertext+tag ).
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    let encoded = B64.encode(&blob);

    std::fs::write(fallback_path(dir, account), encoded)
        .map_err(|e| format!("write: {e}"))
}

fn read_fallback_file(dir: &Path, account: &str) -> Result<String, String> {
    let path = fallback_path(dir, account);
    let encoded = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
    let blob = B64.decode(encoded.trim()).map_err(|e| format!("base64: {e}"))?;
    if blob.len() < 12 + 16 {
        return Err("ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&FALLBACK_KEY)
        .map_err(|e| format!("invalid key length: {e}"))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| format!("decrypt (wrong key or tampered file): {e}"))?;
    String::from_utf8(plaintext).map_err(|e| format!("utf-8: {e}"))
}
