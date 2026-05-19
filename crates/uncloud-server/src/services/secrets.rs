use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use rand::RngCore;

use crate::config::SecretsConfig;
use crate::error::{AppError, Result};
use crate::models::EncryptedMailCredential;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const ALGORITHM: &str = "AES-256-GCM";

#[derive(Clone)]
pub struct SecretCipher {
    cipher: Aes256Gcm,
}

impl SecretCipher {
    pub fn from_config(config: &SecretsConfig) -> Result<Self> {
        let Some(raw) = config.master_key.as_deref() else {
            return Err(AppError::BadRequest(
                "mail credential storage is not configured".into(),
            ));
        };
        Self::from_master_key(raw)
    }

    pub fn from_master_key(raw: &str) -> Result<Self> {
        let key = decode_master_key(raw.trim())?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| AppError::Internal("invalid secret key length".into()))?;
        Ok(Self { cipher })
    }

    pub fn encrypt_mail_credential(&self, password: &str) -> Result<EncryptedMailCredential> {
        let mut nonce = [0_u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), password.as_bytes())
            .map_err(|_| AppError::Internal("failed to encrypt mail credential".into()))?;
        Ok(EncryptedMailCredential {
            version: 1,
            algorithm: ALGORITHM.to_string(),
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }

    pub fn decrypt_mail_credential(&self, credential: &EncryptedMailCredential) -> Result<String> {
        if credential.version != 1 || credential.algorithm != ALGORITHM {
            return Err(AppError::Internal(
                "unsupported encrypted mail credential format".into(),
            ));
        }
        let nonce = STANDARD
            .decode(&credential.nonce)
            .map_err(|_| AppError::Internal("invalid encrypted mail credential nonce".into()))?;
        if nonce.len() != NONCE_LEN {
            return Err(AppError::Internal(
                "invalid encrypted mail credential nonce length".into(),
            ));
        }
        let ciphertext = STANDARD.decode(&credential.ciphertext).map_err(|_| {
            AppError::Internal("invalid encrypted mail credential ciphertext".into())
        })?;
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| AppError::Internal("failed to decrypt mail credential".into()))?;
        String::from_utf8(plaintext)
            .map_err(|_| AppError::Internal("mail credential is not valid UTF-8".into()))
    }
}

fn decode_master_key(raw: &str) -> Result<[u8; KEY_LEN]> {
    let raw = raw.strip_prefix("base64:").unwrap_or(raw);
    let decoded = STANDARD
        .decode(raw)
        .or_else(|_| URL_SAFE_NO_PAD.decode(raw))
        .or_else(|_| hex::decode(raw))
        .map_err(|_| {
            AppError::BadRequest("secrets.master_key must be a 32-byte base64 or hex value".into())
        })?;
    decoded.try_into().map_err(|_| {
        AppError::BadRequest("secrets.master_key must decode to exactly 32 bytes".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

    #[test]
    fn mail_credential_round_trip() {
        let cipher = SecretCipher::from_master_key(TEST_KEY).unwrap();
        let encrypted = cipher.encrypt_mail_credential("app-password").unwrap();

        assert_ne!(encrypted.ciphertext, "app-password");
        assert_eq!(
            cipher.decrypt_mail_credential(&encrypted).unwrap(),
            "app-password"
        );
    }

    #[test]
    fn rejects_short_master_key() {
        let err = match SecretCipher::from_master_key("too-short") {
            Ok(_) => panic!("short master key should be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("secrets.master_key"));
    }
}
