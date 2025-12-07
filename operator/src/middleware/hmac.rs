//! HMAC verification middleware
//!
//! Verifies webhook signatures and prevents replay attacks.
//!
//! Security checks:
//! 1. HMAC-SHA256 signature verification (supports multiple secrets for rotation)
//! 2. Timestamp within acceptable drift window
//! 3. Request body integrity
//!
//! Secret Rotation:
//! - Supports both current and previous secret during grace period
//! - Logs which secret was used for audit purposes

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header::HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;

/// HMAC verification state with support for secret rotation
#[derive(Clone)]
pub struct HmacState {
    /// List of valid HMAC secrets (current + previous during rotation)
    secrets: Arc<Vec<Vec<u8>>>,
    /// Maximum timestamp drift in seconds
    max_drift_secs: i64,
}

impl HmacState {
    /// Create a new HMAC state with a single secret
    pub fn new(secret: String, max_drift_secs: i64) -> Self {
        Self {
            secrets: Arc::new(vec![secret.into_bytes()]),
            max_drift_secs,
        }
    }

    /// Create a new HMAC state with multiple secrets (for rotation grace period)
    ///
    /// The first secret is the current/primary secret.
    /// Additional secrets are previous secrets that are still valid during rotation.
    pub fn with_rotation(secrets: Vec<String>, max_drift_secs: i64) -> Self {
        let secret_bytes: Vec<Vec<u8>> = secrets
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.into_bytes())
            .collect();

        if secret_bytes.is_empty() {
            tracing::warn!("HmacState created with no valid secrets!");
        }

        Self {
            secrets: Arc::new(secret_bytes),
            max_drift_secs,
        }
    }

    /// Check if rotation is active (multiple secrets configured)
    pub fn is_rotation_active(&self) -> bool {
        self.secrets.len() > 1
    }
}

/// Header names for signature verification
pub const SIGNATURE_HEADER: &str = "X-Signature";
pub const TIMESTAMP_HEADER: &str = "X-Timestamp";

/// Result of signature verification
#[derive(Debug)]
enum VerificationResult {
    /// Signature matched using secret at given index
    Valid { secret_index: usize },
    /// No secrets matched
    Invalid,
}

/// HMAC verification middleware
///
/// Extracts signature and timestamp from headers, verifies HMAC-SHA256,
/// and checks timestamp is within acceptable drift window.
///
/// During secret rotation, tries all configured secrets and logs which one matched.
pub async fn hmac_verify(
    State(state): State<HmacState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    // Extract signature header
    let signature = match headers.get(SIGNATURE_HEADER) {
        Some(sig) => match sig.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Invalid signature header encoding",
                );
            }
        },
        None => {
            return error_response(StatusCode::UNAUTHORIZED, "Missing X-Signature header");
        }
    };

    // Extract timestamp header
    let timestamp_str = match headers.get(TIMESTAMP_HEADER) {
        Some(ts) => match ts.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Invalid timestamp header encoding",
                );
            }
        },
        None => {
            return error_response(StatusCode::UNAUTHORIZED, "Missing X-Timestamp header");
        }
    };

    // Parse timestamp
    let timestamp: i64 = match timestamp_str.parse() {
        Ok(ts) => ts,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid timestamp format");
        }
    };

    // Check timestamp drift (replay protection)
    let now = Utc::now().timestamp();
    let drift = (now - timestamp).abs();
    if drift > state.max_drift_secs {
        tracing::warn!(
            timestamp = timestamp,
            now = now,
            drift = drift,
            max_drift = state.max_drift_secs,
            "Request timestamp outside acceptable window"
        );
        return error_response(
            StatusCode::UNAUTHORIZED,
            &format!(
                "Request expired (drift: {}s, max: {}s)",
                drift, state.max_drift_secs
            ),
        );
    }

    // Read body for signature verification
    let (parts, body) = request.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Failed to read request body");
        }
    };

    // Try verification with each secret
    let verification_result =
        verify_with_secrets(&state.secrets, &signature, &timestamp_str, &body_bytes);

    match verification_result {
        VerificationResult::Valid { secret_index } => {
            if secret_index > 0 {
                // Using a previous/rotated secret
                tracing::info!(
                    secret_index = secret_index,
                    "HMAC verified with rotated secret (grace period active)"
                );
            } else {
                tracing::debug!(
                    timestamp = timestamp,
                    body_size = body_bytes.len(),
                    "HMAC verification successful"
                );
            }

            // Reconstruct request with body and continue
            let request = Request::from_parts(parts, Body::from(body_bytes));
            next.run(request).await
        }
        VerificationResult::Invalid => {
            tracing::warn!(
                provided_signature = %signature,
                secrets_tried = state.secrets.len(),
                "HMAC signature verification failed"
            );
            error_response(StatusCode::UNAUTHORIZED, "Invalid signature")
        }
    }
}

/// Verify signature against multiple secrets
fn verify_with_secrets(
    secrets: &[Vec<u8>],
    signature: &str,
    timestamp_str: &str,
    body_bytes: &[u8],
) -> VerificationResult {
    for (index, secret) in secrets.iter().enumerate() {
        let mut mac = match Hmac::<Sha256>::new_from_slice(secret) {
            Ok(m) => m,
            Err(_) => {
                tracing::error!(secret_index = index, "Failed to create HMAC instance");
                continue;
            }
        };

        mac.update(timestamp_str.as_bytes());
        mac.update(body_bytes);

        let expected_signature = hex::encode(mac.finalize().into_bytes());

        // Constant-time comparison to prevent timing attacks
        if constant_time_compare(signature, &expected_signature) {
            return VerificationResult::Valid {
                secret_index: index,
            };
        }
    }

    VerificationResult::Invalid
}

/// Constant-time string comparison to prevent timing attacks
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// Create an error response
fn error_response(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "status": "rejected",
        "reason": "authentication_failed",
        "details": message
    });

    (status, Json(body)).into_response()
}

/// Extension trait to add the verified timestamp to request extensions
pub struct VerifiedTimestamp(pub i64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_compare() {
        assert!(constant_time_compare("abc", "abc"));
        assert!(!constant_time_compare("abc", "abd"));
        assert!(!constant_time_compare("abc", "ab"));
        assert!(!constant_time_compare("abc", "abcd"));
    }

    #[test]
    fn test_hmac_state_single_secret() {
        let state = HmacState::new("secret".to_string(), 60);
        assert!(!state.is_rotation_active());
        assert_eq!(state.secrets.len(), 1);
    }

    #[test]
    fn test_hmac_state_with_rotation() {
        let state = HmacState::with_rotation(
            vec!["new-secret".to_string(), "old-secret".to_string()],
            60,
        );
        assert!(state.is_rotation_active());
        assert_eq!(state.secrets.len(), 2);
    }

    #[test]
    fn test_hmac_state_filters_empty_secrets() {
        let state = HmacState::with_rotation(
            vec![
                "secret1".to_string(),
                "".to_string(),
                "secret2".to_string(),
            ],
            60,
        );
        assert_eq!(state.secrets.len(), 2);
    }

    #[test]
    fn test_verify_with_primary_secret() {
        let secrets = vec![b"primary-secret".to_vec(), b"old-secret".to_vec()];

        let timestamp = "1234567890";
        let body = b"test body";

        // Generate signature with primary secret
        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());

        let result = verify_with_secrets(&secrets, &signature, timestamp, body);
        match result {
            VerificationResult::Valid { secret_index } => assert_eq!(secret_index, 0),
            _ => panic!("Expected valid result with secret_index 0"),
        }
    }

    #[test]
    fn test_verify_with_rotated_secret() {
        let secrets = vec![b"new-secret".to_vec(), b"old-secret".to_vec()];

        let timestamp = "1234567890";
        let body = b"test body";

        // Generate signature with OLD secret (simulating rotation)
        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets[1]).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());

        let result = verify_with_secrets(&secrets, &signature, timestamp, body);
        match result {
            VerificationResult::Valid { secret_index } => assert_eq!(secret_index, 1),
            _ => panic!("Expected valid result with secret_index 1"),
        }
    }

    #[test]
    fn test_verify_invalid_signature() {
        let secrets = vec![b"secret".to_vec()];
        let result = verify_with_secrets(&secrets, "invalid-signature", "123", b"body");
        assert!(matches!(result, VerificationResult::Invalid));
    }
}
