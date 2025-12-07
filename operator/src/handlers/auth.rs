//! Wallet authentication handler
//!
//! Authenticates users via Solana wallet signature verification.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::error::AppError;

/// Auth state for wallet authentication
pub struct WalletAuthState {
    pub db: SqlitePool,
    /// JWT secret for signing tokens (in production, use proper secret management)
    pub jwt_secret: String,
}

#[derive(Debug, Deserialize)]
pub struct WalletAuthRequest {
    pub wallet_address: String,
    pub message: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct WalletAuthResponse {
    pub token: String,
    pub role: String,
    pub identifier: String,
}

/// Wallet authentication endpoint
///
/// POST /api/v1/auth/wallet
pub async fn wallet_auth(
    State(state): State<Arc<WalletAuthState>>,
    Json(req): Json<WalletAuthRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify the message contains expected format
    if !req.message.contains("Chimera Dashboard Authentication") {
        return Err(AppError::Auth(
            "Invalid authentication message".to_string(),
        ));
    }

    // Verify the wallet address in message matches the provided wallet address
    if !req.message.contains(&req.wallet_address) {
        return Err(AppError::Auth(
            "Wallet address mismatch".to_string(),
        ));
    }

    // Decode signature
    let signature_bytes = BASE64
        .decode(&req.signature)
        .map_err(|_| AppError::Auth("Invalid signature encoding".to_string()))?;

    // Verify signature using Solana SDK
    use solana_sdk::{
        pubkey::Pubkey,
        signature::Signature,
    };

    let pubkey = req.wallet_address
        .parse::<Pubkey>()
        .map_err(|_| AppError::Auth("Invalid wallet address format".to_string()))?;

    // Create signature from bytes
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|_| AppError::Auth("Invalid signature format".to_string()))?;

    // Verify signature
    if !signature.verify(pubkey.as_ref(), req.message.as_bytes()) {
        return Err(AppError::Auth("Invalid signature verification".to_string()));
    }

    // Check if wallet is in admin_wallets table
    let role = check_wallet_role(&state.db, &req.wallet_address).await?;

    // Generate JWT token
    let token = generate_jwt(&req.wallet_address, &role, &state.jwt_secret)?;

    Ok((
        StatusCode::OK,
        Json(WalletAuthResponse {
            token,
            role,
            identifier: req.wallet_address,
        }),
    ))
}

/// Check if wallet is registered as admin
async fn check_wallet_role(db: &SqlitePool, wallet_address: &str) -> Result<String, AppError> {
    // Check admin_wallets table
    let result = sqlx::query_scalar::<_, String>(
        "SELECT role FROM admin_wallets WHERE wallet_address = ?",
    )
    .bind(wallet_address)
    .fetch_optional(db)
    .await?;

    match result {
        Some(role) => Ok(role),
        None => {
            // Check if wallet is in wallets table (readonly access for tracked wallets)
            let in_roster = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM wallets WHERE address = ?",
            )
            .bind(wallet_address)
            .fetch_one(db)
            .await?;

            if in_roster > 0 {
                Ok("readonly".to_string())
            } else {
                // Unknown wallet - deny access
                Err(AppError::Auth(
                    "Wallet not authorized for dashboard access".to_string(),
                ))
            }
        }
    }
}

/// Generate a simple JWT token
/// In production, use a proper JWT library like jsonwebtoken
fn generate_jwt(wallet_address: &str, role: &str, secret: &str) -> Result<String, AppError> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // Simple JWT structure (header.payload.signature)
    let header = BASE64.encode(r#"{"alg":"HS256","typ":"JWT"}"#);

    let exp = Utc::now() + TimeDelta::hours(24);
    let payload = serde_json::json!({
        "sub": wallet_address,
        "role": role,
        "exp": exp.timestamp(),
        "iat": Utc::now().timestamp(),
    });
    let payload_b64 = BASE64.encode(payload.to_string());

    let message = format!("{}.{}", header, payload_b64);

    // Create signature
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| AppError::Internal("Failed to create HMAC".to_string()))?;
    mac.update(message.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());

    Ok(format!("{}.{}", message, signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_jwt() {
        let token = generate_jwt("7xKXtg...gAsU", "admin", "test-secret").unwrap();
        assert!(token.contains('.'));
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
    }
}
