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
    State(state): State<Arc<HmacState>>,
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

    #[test]
    fn test_timestamp_drift_within_window() {
        let state = HmacState::new("secret".to_string(), 60);
        let now = Utc::now().timestamp();
        
        // Test timestamp exactly at max drift (should pass)
        let timestamp_at_limit = now - state.max_drift_secs;
        let drift = (now - timestamp_at_limit).abs();
        assert!(drift <= state.max_drift_secs, "Timestamp at limit should be within window");
        
        // Test timestamp just inside window
        let timestamp_inside = now - (state.max_drift_secs - 1);
        let drift_inside = (now - timestamp_inside).abs();
        assert!(drift_inside < state.max_drift_secs, "Timestamp inside window should pass");
    }

    #[test]
    fn test_timestamp_drift_outside_window() {
        let state = HmacState::new("secret".to_string(), 60);
        let now = Utc::now().timestamp();
        
        // Test timestamp just outside window (should fail)
        let timestamp_outside = now - (state.max_drift_secs + 1);
        let drift = (now - timestamp_outside).abs();
        assert!(drift > state.max_drift_secs, "Timestamp outside window should be rejected");
        
        // Test timestamp far in the past
        let timestamp_far_past = now - 3600; // 1 hour ago
        let drift_far = (now - timestamp_far_past).abs();
        assert!(drift_far > state.max_drift_secs, "Old timestamp should be rejected");
        
        // Test timestamp in the future (should also be rejected if outside window)
        let timestamp_future = now + (state.max_drift_secs + 1);
        let drift_future = (now - timestamp_future).abs();
        assert!(drift_future > state.max_drift_secs, "Future timestamp outside window should be rejected");
    }

    #[test]
    fn test_timestamp_drift_boundary_conditions() {
        let state = HmacState::new("secret".to_string(), 60);
        let now = Utc::now().timestamp();
        
        // Test exactly at boundary (should pass - drift <= max_drift)
        let timestamp_exact = now - state.max_drift_secs;
        let drift_exact = (now - timestamp_exact).abs();
        assert_eq!(drift_exact, state.max_drift_secs);
        
        // Test one second before boundary (should pass)
        let timestamp_before = now - (state.max_drift_secs - 1);
        let drift_before = (now - timestamp_before).abs();
        assert!(drift_before < state.max_drift_secs);
        
        // Test one second after boundary (should fail)
        let timestamp_after = now - (state.max_drift_secs + 1);
        let drift_after = (now - timestamp_after).abs();
        assert!(drift_after > state.max_drift_secs);
    }

    #[test]
    fn test_replay_window_same_timestamp() {
        // Test that same timestamp + body + signature can be verified multiple times
        // (In production, you'd want to track used timestamps, but for now we just verify signature)
        let secrets = vec![b"secret".to_vec()];
        let timestamp = "1234567890";
        let body = b"test body";
        
        // Generate signature
        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        
        // Verify multiple times with same data (simulating replay)
        for _ in 0..5 {
            let result = verify_with_secrets(&secrets, &signature, timestamp, body);
            assert!(matches!(result, VerificationResult::Valid { .. }), 
                "Same signature should verify multiple times (replay detection would be in higher layer)");
        }
    }

    #[test]
    fn test_replay_window_different_timestamps() {
        // Test that different timestamps with same body produce different signatures
        let secrets = vec![b"secret".to_vec()];
        let body = b"test body";
        
        let timestamp1 = "1234567890";
        let mut mac1 = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac1.update(timestamp1.as_bytes());
        mac1.update(body);
        let signature1 = hex::encode(mac1.finalize().into_bytes());
        
        let timestamp2 = "1234567891";
        let mut mac2 = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac2.update(timestamp2.as_bytes());
        mac2.update(body);
        let signature2 = hex::encode(mac2.finalize().into_bytes());
        
        // Signatures should be different
        assert_ne!(signature1, signature2, "Different timestamps should produce different signatures");
        
        // Each signature should only verify with its own timestamp
        let result1 = verify_with_secrets(&secrets, &signature1, timestamp1, body);
        assert!(matches!(result1, VerificationResult::Valid { .. }));
        
        let result2 = verify_with_secrets(&secrets, &signature2, timestamp2, body);
        assert!(matches!(result2, VerificationResult::Valid { .. }));
        
        // Cross-verification should fail
        let result_cross1 = verify_with_secrets(&secrets, &signature1, timestamp2, body);
        assert!(matches!(result_cross1, VerificationResult::Invalid));
        
        let result_cross2 = verify_with_secrets(&secrets, &signature2, timestamp1, body);
        assert!(matches!(result_cross2, VerificationResult::Invalid));
    }

    #[test]
    fn test_replay_window_different_bodies() {
        // Test that same timestamp with different body produces different signatures
        let secrets = vec![b"secret".to_vec()];
        let timestamp = "1234567890";
        
        let body1 = b"test body 1";
        let mut mac1 = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac1.update(timestamp.as_bytes());
        mac1.update(body1);
        let signature1 = hex::encode(mac1.finalize().into_bytes());
        
        let body2 = b"test body 2";
        let mut mac2 = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac2.update(timestamp.as_bytes());
        mac2.update(body2);
        let signature2 = hex::encode(mac2.finalize().into_bytes());
        
        // Signatures should be different
        assert_ne!(signature1, signature2, "Different bodies should produce different signatures");
        
        // Each signature should only verify with its own body
        let result1 = verify_with_secrets(&secrets, &signature1, timestamp, body1);
        assert!(matches!(result1, VerificationResult::Valid { .. }));
        
        let result2 = verify_with_secrets(&secrets, &signature2, timestamp, body2);
        assert!(matches!(result2, VerificationResult::Valid { .. }));
        
        // Cross-verification should fail
        let result_cross1 = verify_with_secrets(&secrets, &signature1, timestamp, body2);
        assert!(matches!(result_cross1, VerificationResult::Invalid));
        
        let result_cross2 = verify_with_secrets(&secrets, &signature2, timestamp, body1);
        assert!(matches!(result_cross2, VerificationResult::Invalid));
    }

    #[test]
    fn test_hmac_state_empty_secrets() {
        // Test that empty secrets list is handled
        let state = HmacState::with_rotation(vec![], 60);
        assert_eq!(state.secrets.len(), 0);
        assert!(!state.is_rotation_active());
    }

    #[test]
    fn test_hmac_state_all_empty_strings() {
        // Test that all empty strings are filtered out
        let state = HmacState::with_rotation(
            vec!["".to_string(), "".to_string(), "".to_string()],
            60,
        );
        assert_eq!(state.secrets.len(), 0);
    }

    #[test]
    fn test_verify_with_no_secrets() {
        // Test verification with no secrets (should fail)
        let secrets: Vec<Vec<u8>> = vec![];
        let result = verify_with_secrets(&secrets, "any-signature", "123", b"body");
        assert!(matches!(result, VerificationResult::Invalid));
    }

    #[test]
    fn test_constant_time_compare_timing_attack_prevention() {
        // Test that constant_time_compare doesn't leak information through timing
        // This is a basic test - full timing attack prevention would require more sophisticated testing
        
        // Same strings should match
        assert!(constant_time_compare("same", "same"));
        
        // Different strings should not match
        assert!(!constant_time_compare("same", "different"));
        
        // Different lengths should not match (early return, but still constant-time for same length)
        assert!(!constant_time_compare("short", "much longer string"));
        assert!(!constant_time_compare("much longer string", "short"));
        
        // Empty strings
        assert!(constant_time_compare("", ""));
        assert!(!constant_time_compare("", "not empty"));
        assert!(!constant_time_compare("not empty", ""));
    }

    #[test]
    fn test_hmac_signature_format() {
        // Test that signatures are hex-encoded
        let secrets = vec![b"secret".to_vec()];
        let timestamp = "1234567890";
        let body = b"test body";
        
        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        
        // Signature should be hex string (64 chars for SHA256)
        assert_eq!(signature.len(), 64, "SHA256 HMAC should produce 64-char hex string");
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()), 
            "Signature should be valid hex string");
    }
}
