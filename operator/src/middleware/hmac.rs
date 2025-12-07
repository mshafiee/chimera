//! HMAC verification middleware
//!
//! Verifies webhook signatures and prevents replay attacks.
//! 
//! Security checks:
//! 1. HMAC-SHA256 signature verification
//! 2. Timestamp within acceptable drift window
//! 3. Request body integrity

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

/// HMAC verification state
#[derive(Clone)]
pub struct HmacState {
    /// HMAC secret key
    secret: Arc<Vec<u8>>,
    /// Maximum timestamp drift in seconds
    max_drift_secs: i64,
}

impl HmacState {
    /// Create a new HMAC state
    pub fn new(secret: String, max_drift_secs: i64) -> Self {
        Self {
            secret: Arc::new(secret.into_bytes()),
            max_drift_secs,
        }
    }
}

/// Header names for signature verification
pub const SIGNATURE_HEADER: &str = "X-Signature";
pub const TIMESTAMP_HEADER: &str = "X-Timestamp";

/// HMAC verification middleware
///
/// Extracts signature and timestamp from headers, verifies HMAC-SHA256,
/// and checks timestamp is within acceptable drift window.
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
            &format!("Request expired (drift: {}s, max: {}s)", drift, state.max_drift_secs),
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

    // Verify HMAC signature
    // Signature = HMAC_SHA256(timestamp + body, secret)
    let mut mac = match Hmac::<Sha256>::new_from_slice(&state.secret) {
        Ok(m) => m,
        Err(_) => {
            tracing::error!("Failed to create HMAC instance - invalid secret");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal error");
        }
    };

    mac.update(timestamp_str.as_bytes());
    mac.update(&body_bytes);

    let expected_signature = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison to prevent timing attacks
    if !constant_time_compare(&signature, &expected_signature) {
        tracing::warn!(
            provided_signature = %signature,
            "HMAC signature verification failed"
        );
        return error_response(StatusCode::UNAUTHORIZED, "Invalid signature");
    }

    tracing::debug!(
        timestamp = timestamp,
        body_size = body_bytes.len(),
        "HMAC verification successful"
    );

    // Reconstruct request with body and continue
    let request = Request::from_parts(parts, Body::from(body_bytes));
    next.run(request).await
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
    fn test_hmac_generation() {
        let secret = b"test-secret";
        let timestamp = "1234567890";
        let body = b"test body";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(body);

        let signature = hex::encode(mac.finalize().into_bytes());
        
        // Verify signature is deterministic
        let mut mac2 = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac2.update(timestamp.as_bytes());
        mac2.update(body);
        let signature2 = hex::encode(mac2.finalize().into_bytes());
        
        assert_eq!(signature, signature2);
    }
}
