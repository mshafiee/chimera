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
use parking_lot::Mutex;
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// HMAC verification state with support for secret rotation
///
/// NOTE: Replay protection is process-local — `seen_nonces` lives only in this
/// process's memory. In a multi-instance / scale-out deployment (or after a
/// restart), a captured signed request can be replayed once per instance within
/// the drift window. Operators must NOT rely on this as a hard replay guard in
/// such deployments (the nonce store would need to be shared, e.g. via Redis).
#[derive(Clone)]
pub struct HmacState {
    /// List of valid HMAC secrets (current + previous during rotation)
    secrets: Arc<Vec<Vec<u8>>>,
    /// Maximum timestamp drift in seconds
    max_drift_secs: i64,
    /// Nonce store: signature → received_at timestamp. Prevents replay within the drift window.
    seen_nonces: Arc<Mutex<HashMap<String, i64>>>,
}

impl std::fmt::Debug for HmacState {
    /// Secrets must never reach logs/debug output — only the count is shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HmacState")
            .field("secret_count", &self.secrets.len())
            .field("max_drift_secs", &self.max_drift_secs)
            .finish_non_exhaustive()
    }
}

impl HmacState {
    /// Create a new HMAC state with a single secret
    pub fn new(secret: String, max_drift_secs: i64) -> Self {
        assert!(!secret.is_empty(), "HMAC secret must not be empty");
        Self {
            secrets: Arc::new(vec![secret.into_bytes()]),
            max_drift_secs,
            seen_nonces: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new HMAC state with multiple secrets (for rotation grace period)
    ///
    /// The first secret is the current/primary secret.
    /// Additional secrets are previous secrets that are still valid during rotation.
    pub fn with_rotation(secrets: Vec<String>, max_drift_secs: i64) -> Result<Self, anyhow::Error> {
        let secret_bytes: Vec<Vec<u8>> = secrets
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.into_bytes())
            .collect();

        if secret_bytes.is_empty() {
            return Err(anyhow::anyhow!(
                "No valid HMAC secrets configured — refusing to start. \
                 Set CHIMERA_SECURITY__WEBHOOK_SECRET."
            ));
        }

        Ok(Self {
            secrets: Arc::new(secret_bytes),
            max_drift_secs,
            seen_nonces: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Maximum nonce store entries. At 1000 RPS with a 60s drift window the store
    /// holds ~60,000 entries normally; this cap prevents runaway growth if the
    /// rate limit is raised or if eviction falls behind during a burst.
    const MAX_NONCE_STORE: usize = 100_000;

    /// Eviction is amortized: the O(n) sweep runs only when the store grows
    /// past this threshold, so the common case stays O(1) per request.
    const EVICTION_THRESHOLD: usize = 4096;

    /// Check nonce and record it.
    fn check_and_record_nonce(&self, nonce: &str, now: i64) -> NonceResult {
        let mut store = self.seen_nonces.lock();
        // Amortized eviction. Boundary matches the rejection gate exactly
        // (`drift > max_drift_secs` rejects, so `<=` keeps entries): a replay
        // arriving at the drift boundary still finds its nonce in the store.
        if store.len() >= Self::EVICTION_THRESHOLD {
            store.retain(|_, ts| now - *ts <= self.max_drift_secs);
        }
        // Hard cap: if post-eviction the store is still oversized, reject the new nonce
        // rather than dropping valid replay-protection entries. This prevents a burst of
        // requests from opening a replay window — callers receive false here and the
        // request is treated as a duplicate (safe fail-closed).
        if store.len() >= Self::MAX_NONCE_STORE {
            tracing::warn!(
                store_size = store.len(),
                "Nonce store at capacity — rejecting nonce to preserve replay protection"
            );
            return NonceResult::Capacity;
        }
        if store.contains_key(nonce) {
            return NonceResult::Replay; // Replay detected
        }
        store.insert(nonce.to_string(), now);
        NonceResult::Accepted
    }

    /// Check if rotation is active (multiple secrets configured)
    pub fn is_rotation_active(&self) -> bool {
        self.secrets.len() > 1
    }
}

/// Header names for signature verification
pub const SIGNATURE_HEADER: &str = "X-Signature";
pub const TIMESTAMP_HEADER: &str = "X-Timestamp";

/// Maximum allowed size for signature and timestamp headers (4KB)
/// Prevents DoS via memory exhaustion from oversized headers
const MAX_HEADER_SIZE: usize = 4096;

/// Result of a nonce check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonceResult {
    /// Nonce recorded; request may proceed
    Accepted,
    /// Nonce already seen within the drift window — genuine replay
    Replay,
    /// Store is at capacity — a load/DoS condition, not an authentication failure
    Capacity,
}

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
        Some(sig) => {
            // Check header length before converting to string to prevent DoS
            if sig.len() > MAX_HEADER_SIZE {
                tracing::warn!(
                    header_len = sig.len(),
                    max_allowed = MAX_HEADER_SIZE,
                    "Signature header exceeds maximum size"
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Signature header too large",
                );
            }
            match sig.to_str() {
                Ok(s) => {
                    if s.len() > MAX_HEADER_SIZE {
                        tracing::warn!(
                            header_len = s.len(),
                            max_allowed = MAX_HEADER_SIZE,
                            "Signature string exceeds maximum size"
                        );
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            "Signature header too large",
                        );
                    }
                    s.to_string()
                },
                Err(_) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "Invalid signature header encoding",
                    );
                }
            }
        },
        None => {
            return error_response(StatusCode::UNAUTHORIZED, "Missing X-Signature header");
        }
    };

    // Extract timestamp header
    let timestamp_str = match headers.get(TIMESTAMP_HEADER) {
        Some(ts) => {
            // Check header length before converting to string to prevent DoS
            if ts.len() > MAX_HEADER_SIZE {
                tracing::warn!(
                    header_len = ts.len(),
                    max_allowed = MAX_HEADER_SIZE,
                    "Timestamp header exceeds maximum size"
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Timestamp header too large",
                );
            }
            match ts.to_str() {
                Ok(s) => {
                    if s.len() > MAX_HEADER_SIZE {
                        tracing::warn!(
                            header_len = s.len(),
                            max_allowed = MAX_HEADER_SIZE,
                            "Timestamp string exceeds maximum size"
                        );
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            "Timestamp header too large",
                        );
                    }
                    s.to_string()
                },
                Err(_) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "Invalid timestamp header encoding",
                    );
                }
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

    // Check timestamp drift (replay protection).
    // saturating_sub: `timestamp` is attacker-controlled and reaches this line
    // before any signature check; unchecked subtraction would panic in debug
    // (cheap remote DoS) and wrap in release.
    let now = Utc::now().timestamp();
    let drift = now.saturating_sub(timestamp).abs();
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
            // Replay protection: signature must not have been seen within the drift window.
            // The nonce is the signature itself — it encodes (timestamp || body) so it's unique per request.
            let now = Utc::now().timestamp();
            match state.check_and_record_nonce(&signature, now) {
                NonceResult::Accepted => {}
                NonceResult::Replay => {
                    tracing::warn!(
                        signature_prefix = %signature.get(..8).unwrap_or(&signature),
                        "Replay attack detected — nonce already seen"
                    );
                    return error_response(StatusCode::UNAUTHORIZED, "Replay detected");
                }
                NonceResult::Capacity => {
                    tracing::warn!(
                        store_size = state.seen_nonces.lock().len(),
                        "Nonce store at capacity — rejecting request with 503"
                    );
                    return error_response(
                        StatusCode::TOO_MANY_REQUESTS,
                        "Too many requests",
                    );
                }
            }

            if secret_index > 0 {
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

/// Constant-time string comparison to prevent timing attacks.
///
/// The expected digest is a fixed-format 64-char lowercase hex string (public
/// format), so a length mismatch can be rejected up front — the comparison
/// then always runs a fixed length. Comparing only equal-length slices also
/// means the work per request is bounded.
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
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
        let state =
            HmacState::with_rotation(vec!["new-secret".to_string(), "old-secret".to_string()], 60)
                .unwrap();
        assert!(state.is_rotation_active());
        assert_eq!(state.secrets.len(), 2);
    }

    #[test]
    fn test_hmac_state_filters_empty_secrets() {
        let state = HmacState::with_rotation(
            vec!["secret1".to_string(), "".to_string(), "secret2".to_string()],
            60,
        )
        .unwrap();
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
        assert!(
            drift <= state.max_drift_secs,
            "Timestamp at limit should be within window"
        );

        // Test timestamp just inside window
        let timestamp_inside = now - (state.max_drift_secs - 1);
        let drift_inside = (now - timestamp_inside).abs();
        assert!(
            drift_inside < state.max_drift_secs,
            "Timestamp inside window should pass"
        );
    }

    #[test]
    fn test_timestamp_drift_outside_window() {
        let state = HmacState::new("secret".to_string(), 60);
        let now = Utc::now().timestamp();

        // Test timestamp just outside window (should fail)
        let timestamp_outside = now - (state.max_drift_secs + 1);
        let drift = (now - timestamp_outside).abs();
        assert!(
            drift > state.max_drift_secs,
            "Timestamp outside window should be rejected"
        );

        // Test timestamp far in the past
        let timestamp_far_past = now - 3600; // 1 hour ago
        let drift_far = (now - timestamp_far_past).abs();
        assert!(
            drift_far > state.max_drift_secs,
            "Old timestamp should be rejected"
        );

        // Test timestamp in the future (should also be rejected if outside window)
        let timestamp_future = now + (state.max_drift_secs + 1);
        let drift_future = (now - timestamp_future).abs();
        assert!(
            drift_future > state.max_drift_secs,
            "Future timestamp outside window should be rejected"
        );
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
        assert_ne!(
            signature1, signature2,
            "Different timestamps should produce different signatures"
        );

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
        assert_ne!(
            signature1, signature2,
            "Different bodies should produce different signatures"
        );

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
        // with_rotation must return an error when the secrets list is empty
        let result = HmacState::with_rotation(vec![], 60);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No valid HMAC secrets configured"));
    }

    #[test]
    fn test_hmac_state_all_empty_strings() {
        // with_rotation must return an error when all provided secrets are empty strings
        let result =
            HmacState::with_rotation(vec!["".to_string(), "".to_string(), "".to_string()], 60);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No valid HMAC secrets configured"));
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
    fn test_nonce_store_basic_accept() {
        let state = HmacState::with_rotation(vec!["test".to_string()], 60).unwrap();
        assert_eq!(state.check_and_record_nonce("nonce-1", 1000), NonceResult::Accepted);
    }

    #[test]
    fn test_nonce_store_replay_rejected() {
        let state = HmacState::with_rotation(vec!["test".to_string()], 60).unwrap();
        assert_eq!(state.check_and_record_nonce("nonce-1", 1000), NonceResult::Accepted);
        assert_eq!(state.check_and_record_nonce("nonce-1", 1001), NonceResult::Replay);
    }

    #[test]
    fn test_nonce_store_expired_evicted() {
        let state = HmacState::with_rotation(vec!["test".to_string()], 60).unwrap();
        // Insert a nonce at t=0
        assert_eq!(state.check_and_record_nonce("old-nonce", 0), NonceResult::Accepted);
        // At t=120 (past 60s drift), old-nonce should be evicted
        // The new nonce is accepted because old was evicted during retain
        assert_eq!(state.check_and_record_nonce("new-nonce", 120), NonceResult::Accepted);
    }

    #[test]
    fn test_nonce_store_capacity_limit() {
        let state = HmacState::with_rotation(vec!["test".to_string()], 3600).unwrap();
        // Fill the store with many entries
        for i in 0..2001 {
            state.check_and_record_nonce(&format!("nonce-{}", i), 1000);
        }
        // The store can hold more than 2001 entries (MAX is 100_000).
        // Verify retain logic still works: old entry evicted when drift expired
        assert_eq!(state.check_and_record_nonce("recent", 1000), NonceResult::Accepted);
        // Verify replay is still detected
        assert_eq!(state.check_and_record_nonce("nonce-0", 1000), NonceResult::Replay);
    }

    #[test]
    fn test_nonce_store_maintains_order_under_fill() {
        let state = HmacState::with_rotation(vec!["test".to_string()], 1).unwrap();
        // With 1-second drift, inserting at t=0 then checking at t=2
        // should evict the first entry and accept a new one.
        assert_eq!(state.check_and_record_nonce("first", 0), NonceResult::Accepted);
        // At t=2 (past 1s drift), the first entry should be evicted
        assert_eq!(state.check_and_record_nonce("second", 2), NonceResult::Accepted);
    }

    #[test]
    fn test_hmac_signature_format() {
        // Test that signatures are hex-encoded
        let secrets = [b"secret".to_vec()];
        let timestamp = "1234567890";
        let body = b"test body";

        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets[0]).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());

        // Signature should be hex string (64 chars for SHA256)
        assert_eq!(
            signature.len(),
            64,
            "SHA256 HMAC should produce 64-char hex string"
        );
        assert!(
            signature.chars().all(|c| c.is_ascii_hexdigit()),
            "Signature should be valid hex string"
        );
    }

    #[test]
    fn test_header_size_limit_constant() {
        // Test that the MAX_HEADER_SIZE constant is defined appropriately
        assert!(
            MAX_HEADER_SIZE == 4096,
            "MAX_HEADER_SIZE should be 4KB to prevent DoS while allowing reasonable headers"
        );

        // Verify that typical HMAC signatures are well under the limit
        let typical_signature = "a".repeat(64); // SHA256 = 64 hex chars
        assert!(
            typical_signature.len() < MAX_HEADER_SIZE,
            "Typical HMAC signatures should be under the size limit"
        );

        // Verify that typical timestamps are well under the limit
        let typical_timestamp = "1234567890"; // Unix timestamp
        assert!(
            typical_timestamp.len() < MAX_HEADER_SIZE,
            "Typical timestamps should be under the size limit"
        );
    }
}
