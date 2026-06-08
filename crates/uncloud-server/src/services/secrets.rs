use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::RngCore;

use crate::config::SecretsConfig;
use crate::error::{AppError, Result};
use crate::models::{EncryptedMailCredential, EncryptedSubsonicCredential};

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
                "server secret storage is not configured".into(),
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
        let encrypted = self.encrypt_secret(password, "mail credential")?;
        Ok(EncryptedMailCredential {
            version: encrypted.version,
            algorithm: encrypted.algorithm,
            nonce: encrypted.nonce,
            ciphertext: encrypted.ciphertext,
        })
    }

    pub fn decrypt_mail_credential(&self, credential: &EncryptedMailCredential) -> Result<String> {
        self.decrypt_secret(
            credential.version,
            &credential.algorithm,
            &credential.nonce,
            &credential.ciphertext,
            "mail credential",
        )
    }

    pub fn encrypt_subsonic_credential(
        &self,
        password: &str,
    ) -> Result<EncryptedSubsonicCredential> {
        let encrypted = self.encrypt_secret(password, "Subsonic app password")?;
        Ok(EncryptedSubsonicCredential {
            version: encrypted.version,
            algorithm: encrypted.algorithm,
            nonce: encrypted.nonce,
            ciphertext: encrypted.ciphertext,
        })
    }

    pub fn decrypt_subsonic_credential(
        &self,
        credential: &EncryptedSubsonicCredential,
    ) -> Result<String> {
        self.decrypt_secret(
            credential.version,
            &credential.algorithm,
            &credential.nonce,
            &credential.ciphertext,
            "Subsonic app password",
        )
    }

    fn encrypt_secret(&self, password: &str, label: &str) -> Result<EncryptedMailCredential> {
        let mut nonce = [0_u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), password.as_bytes())
            .map_err(|_| AppError::Internal(format!("failed to encrypt {label}")))?;
        Ok(EncryptedMailCredential {
            version: 1,
            algorithm: ALGORITHM.to_string(),
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }

    fn decrypt_secret(
        &self,
        version: u8,
        algorithm: &str,
        nonce: &str,
        ciphertext: &str,
        label: &str,
    ) -> Result<String> {
        if version != 1 || algorithm != ALGORITHM {
            return Err(AppError::Internal(format!(
                "unsupported encrypted {label} format"
            )));
        }
        let nonce = STANDARD
            .decode(nonce)
            .map_err(|_| AppError::Internal(format!("invalid encrypted {label} nonce")))?;
        if nonce.len() != NONCE_LEN {
            return Err(AppError::Internal(format!(
                "invalid encrypted {label} nonce length"
            )));
        }
        let ciphertext = STANDARD
            .decode(ciphertext)
            .map_err(|_| AppError::Internal(format!("invalid encrypted {label} ciphertext")))?;
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| AppError::Internal(format!("failed to decrypt {label}")))?;
        String::from_utf8(plaintext)
            .map_err(|_| AppError::Internal(format!("{label} is not valid UTF-8")))
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
    fn subsonic_credential_round_trip() {
        let cipher = SecretCipher::from_master_key(TEST_KEY).unwrap();
        let encrypted = cipher
            .encrypt_subsonic_credential("subsonic-app-password")
            .unwrap();

        assert_ne!(encrypted.ciphertext, "subsonic-app-password");
        assert_eq!(
            cipher.decrypt_subsonic_credential(&encrypted).unwrap(),
            "subsonic-app-password"
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
