//! Encrypted secrets vault using AES-256-GCM
//!
//! Provides secure storage and retrieval of sensitive configuration:
//! - Webhook HMAC secrets
//! - Wallet private keys
//! - RPC API keys
//!
//! File format: Base64 encoded (nonce || ciphertext || tag)

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Secrets stored in the encrypted vault
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSecrets {
    /// Primary webhook HMAC secret
    pub webhook_secret: String,
    /// Previous webhook secret (for rotation grace period)
    #[serde(default)]
    pub webhook_secret_previous: Option<String>,
    /// Wallet private key (as byte array)
    #[serde(default)]
    pub wallet_private_key: Option<Vec<u8>>,
    /// RPC API key
    #[serde(default)]
    pub rpc_api_key: Option<String>,
    /// Fallback RPC API key
    #[serde(default)]
    pub fallback_rpc_api_key: Option<String>,
}

/// Vault for encrypted secrets
pub struct Vault {
    /// Encryption key (32 bytes for AES-256)
    key: [u8; 32],
}

/// Vault errors
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// Invalid encryption key
    #[error("Invalid vault key: {0}")]
    InvalidKey(String),

    /// Decryption failed
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),

    /// Encryption failed
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    /// File I/O error
    #[error("File error: {0}")]
    FileError(#[from] std::io::Error),

    /// JSON parsing error
    #[error("Parse error: {0}")]
    ParseError(#[from] serde_json::Error),

    /// Base64 decoding error
    #[error("Base64 decode error: {0}")]
    Base64Error(String),
}

impl Vault {
    /// Create a new vault with the given key
    ///
    /// # Arguments
    /// * `key_hex` - 64-character hex string (32 bytes)
    pub fn new(key_hex: &str) -> Result<Self, VaultError> {
        let key_bytes = hex::decode(key_hex).map_err(|e| {
            VaultError::InvalidKey(format!("Invalid hex key: {}", e))
        })?;

        if key_bytes.len() != 32 {
            return Err(VaultError::InvalidKey(format!(
                "Key must be 32 bytes (64 hex chars), got {} bytes",
                key_bytes.len()
            )));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);

        Ok(Self { key })
    }

    /// Create a vault from the CHIMERA_VAULT_KEY environment variable
    pub fn from_env() -> Result<Self, VaultError> {
        let key_hex = std::env::var("CHIMERA_VAULT_KEY").map_err(|_| {
            VaultError::InvalidKey("CHIMERA_VAULT_KEY environment variable not set".to_string())
        })?;

        Self::new(&key_hex)
    }

    /// Load and decrypt secrets from a file
    pub fn load_secrets(&self, path: impl AsRef<Path>) -> Result<VaultSecrets, VaultError> {
        let encrypted_data = std::fs::read_to_string(path)?;
        self.decrypt_secrets(&encrypted_data)
    }

    /// Decrypt secrets from a base64-encoded string
    pub fn decrypt_secrets(&self, encrypted_base64: &str) -> Result<VaultSecrets, VaultError> {
        // Decode base64
        let encrypted_bytes = BASE64.decode(encrypted_base64.trim()).map_err(|e| {
            VaultError::Base64Error(format!("Failed to decode base64: {}", e))
        })?;

        // Extract nonce (first 12 bytes) and ciphertext+tag (rest)
        if encrypted_bytes.len() < 12 + 16 {
            return Err(VaultError::DecryptionFailed(
                "Encrypted data too short".to_string(),
            ));
        }

        let nonce = Nonce::from_slice(&encrypted_bytes[..12]);
        let ciphertext = &encrypted_bytes[12..];

        // Create cipher and decrypt
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| VaultError::InvalidKey(format!("Failed to create cipher: {}", e)))?;

        let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| {
            VaultError::DecryptionFailed(format!("AES-GCM decryption failed: {}", e))
        })?;

        // Parse JSON
        let secrets: VaultSecrets = serde_json::from_slice(&plaintext)?;

        Ok(secrets)
    }

    /// Encrypt and save secrets to a file
    pub fn save_secrets(
        &self,
        secrets: &VaultSecrets,
        path: impl AsRef<Path>,
    ) -> Result<(), VaultError> {
        let encrypted = self.encrypt_secrets(secrets)?;
        std::fs::write(path, encrypted)?;
        Ok(())
    }

    /// Encrypt secrets to a base64-encoded string
    pub fn encrypt_secrets(&self, secrets: &VaultSecrets) -> Result<String, VaultError> {
        // Serialize to JSON
        let plaintext = serde_json::to_vec(secrets)?;

        // Generate random nonce
        let nonce_bytes: [u8; 12] = rand_nonce();
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create cipher and encrypt
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| VaultError::InvalidKey(format!("Failed to create cipher: {}", e)))?;

        let ciphertext = cipher.encrypt(nonce, plaintext.as_slice()).map_err(|e| {
            VaultError::EncryptionFailed(format!("AES-GCM encryption failed: {}", e))
        })?;

        // Combine nonce + ciphertext and encode as base64
        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        Ok(BASE64.encode(&combined))
    }

    /// Generate a new random vault key (for setup)
    pub fn generate_key() -> String {
        let key_bytes: [u8; 32] = rand_bytes();
        hex::encode(key_bytes)
    }
}

/// Generate random bytes using getrandom (cryptographically secure)
fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("Failed to generate random bytes");
    bytes
}

/// Generate random nonce for AES-GCM
fn rand_nonce() -> [u8; 12] {
    rand_bytes()
}

/// Try to load secrets from vault file, falling back to environment variables
pub fn load_secrets_with_fallback() -> Result<VaultSecrets, VaultError> {
    // Try vault file first
    if let Ok(vault) = Vault::from_env() {
        let vault_path = std::env::var("CHIMERA_VAULT_PATH")
            .unwrap_or_else(|_| "config/secrets.enc".to_string());

        if Path::new(&vault_path).exists() {
            match vault.load_secrets(&vault_path) {
                Ok(secrets) => {
                    tracing::info!("Loaded secrets from encrypted vault");
                    return Ok(secrets);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load vault, falling back to env vars");
                }
            }
        }
    }

    // Fall back to environment variables
    let webhook_secret = std::env::var("CHIMERA_SECURITY__WEBHOOK_SECRET").unwrap_or_default();
    let webhook_secret_previous =
        std::env::var("CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS").ok();

    Ok(VaultSecrets {
        webhook_secret,
        webhook_secret_previous,
        wallet_private_key: None,
        rpc_api_key: std::env::var("CHIMERA_RPC__API_KEY").ok(),
        fallback_rpc_api_key: std::env::var("CHIMERA_RPC__FALLBACK_API_KEY").ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key() {
        let key = Vault::generate_key();
        assert_eq!(key.len(), 64); // 32 bytes = 64 hex chars
    }

    #[test]
    fn test_vault_roundtrip() {
        let key = Vault::generate_key();
        let vault = Vault::new(&key).unwrap();

        let secrets = VaultSecrets {
            webhook_secret: "test-secret-123".to_string(),
            webhook_secret_previous: Some("old-secret".to_string()),
            wallet_private_key: Some(vec![1, 2, 3, 4, 5]),
            rpc_api_key: Some("rpc-key".to_string()),
            fallback_rpc_api_key: None,
        };

        let encrypted = vault.encrypt_secrets(&secrets).unwrap();
        let decrypted = vault.decrypt_secrets(&encrypted).unwrap();

        assert_eq!(decrypted.webhook_secret, secrets.webhook_secret);
        assert_eq!(
            decrypted.webhook_secret_previous,
            secrets.webhook_secret_previous
        );
        assert_eq!(decrypted.wallet_private_key, secrets.wallet_private_key);
        assert_eq!(decrypted.rpc_api_key, secrets.rpc_api_key);
    }

    #[test]
    fn test_invalid_key_length() {
        let result = Vault::new("tooshort");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_key_hex() {
        let result = Vault::new("not-valid-hex-string-definitely-not-64-chars-of-hex-here-nope!");
        assert!(result.is_err());
    }
}
