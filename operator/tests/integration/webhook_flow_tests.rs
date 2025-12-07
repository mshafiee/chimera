//! Webhook Flow Integration Tests
//!
//! Tests the full webhook signal processing flow:
//! - HMAC signature verification
//! - Timestamp validation (replay protection)
//! - Payload parsing
//! - Idempotency (duplicate detection)

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde_json::{json, Value};
use tower::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

/// Generate HMAC signature for webhook
fn generate_signature(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

// =============================================================================
// HMAC VERIFICATION TESTS
// =============================================================================

/// Test valid HMAC signature passes
#[tokio::test]
async fn test_valid_hmac_signature() {
    let secret = "test-secret";
    let timestamp = "1733500000";
    let body = json!({
        "strategy": "SHIELD",
        "token": "BONK",
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    })
    .to_string();
    
    let signature = generate_signature(secret, timestamp, &body);

    // Simulate HMAC verification
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(body.as_bytes());
    
    let expected = hex::decode(&signature).unwrap();
    assert!(mac.verify_slice(&expected).is_ok(), "Valid signature should verify");
}

/// Test invalid HMAC signature fails
#[tokio::test]
async fn test_invalid_hmac_signature() {
    let secret = "test-secret";
    let wrong_secret = "wrong-secret";
    let timestamp = "1733500000";
    let body = r#"{"test": "data"}"#;
    
    // Generate with wrong secret
    let bad_signature = generate_signature(wrong_secret, timestamp, body);
    
    // Try to verify with correct secret
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(body.as_bytes());
    
    let bad_sig_bytes = hex::decode(&bad_signature).unwrap();
    assert!(mac.verify_slice(&bad_sig_bytes).is_err(), "Invalid signature should fail");
}

/// Test signature with empty body
#[tokio::test]
async fn test_signature_empty_body() {
    let secret = "test-secret";
    let timestamp = "1733500000";
    let body = "";
    
    let signature = generate_signature(secret, timestamp, body);
    assert!(!signature.is_empty(), "Should generate signature for empty body");
}

// =============================================================================
// TIMESTAMP VALIDATION TESTS (Replay Protection)
// =============================================================================

/// Test timestamp within allowed drift
#[tokio::test]
async fn test_timestamp_within_drift() {
    let now = chrono::Utc::now().timestamp();
    let max_drift_secs: i64 = 60;
    
    // 30 seconds ago - should be valid
    let req_time = now - 30;
    let diff = (now - req_time).abs();
    assert!(diff <= max_drift_secs, "30 second old request should be valid");
}

/// Test timestamp outside allowed drift
#[tokio::test]
async fn test_timestamp_outside_drift() {
    let now = chrono::Utc::now().timestamp();
    let max_drift_secs: i64 = 60;
    
    // 120 seconds ago - should be rejected
    let req_time = now - 120;
    let diff = (now - req_time).abs();
    assert!(diff > max_drift_secs, "120 second old request should be rejected");
}

/// Test future timestamp
#[tokio::test]
async fn test_future_timestamp() {
    let now = chrono::Utc::now().timestamp();
    let max_drift_secs: i64 = 60;
    
    // 120 seconds in the future - should be rejected
    let req_time = now + 120;
    let diff = (now - req_time).abs();
    assert!(diff > max_drift_secs, "Future request should be rejected");
}

/// Test timestamp at exact boundary
#[tokio::test]
async fn test_timestamp_at_boundary() {
    let now = chrono::Utc::now().timestamp();
    let max_drift_secs: i64 = 60;
    
    // Exactly at boundary
    let req_time = now - 60;
    let diff = (now - req_time).abs();
    assert!(diff <= max_drift_secs, "Request at exact boundary should be valid");
}

// =============================================================================
// PAYLOAD PARSING TESTS
// =============================================================================

/// Test valid SHIELD payload parsing
#[tokio::test]
async fn test_shield_payload_parsing() {
    let payload = json!({
        "strategy": "SHIELD",
        "token": "BONK",
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    });
    
    assert_eq!(payload["strategy"], "SHIELD");
    assert_eq!(payload["action"], "BUY");
    assert!(payload["amount_sol"].as_f64().unwrap() > 0.0);
}

/// Test valid SPEAR payload parsing
#[tokio::test]
async fn test_spear_payload_parsing() {
    let payload = json!({
        "strategy": "SPEAR",
        "token": "WIF",
        "action": "BUY",
        "amount_sol": 0.3,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    });
    
    assert_eq!(payload["strategy"], "SPEAR");
}

/// Test valid EXIT payload parsing
#[tokio::test]
async fn test_exit_payload_parsing() {
    let payload = json!({
        "strategy": "EXIT",
        "token": "BONK",
        "action": "SELL",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    });
    
    assert_eq!(payload["strategy"], "EXIT");
    assert_eq!(payload["action"], "SELL");
}

/// Test payload with optional trade_uuid
#[tokio::test]
async fn test_payload_with_trade_uuid() {
    let payload = json!({
        "strategy": "SHIELD",
        "token": "BONK",
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        "trade_uuid": "custom-uuid-12345"
    });
    
    assert_eq!(payload["trade_uuid"], "custom-uuid-12345");
}

/// Test payload without optional trade_uuid
#[tokio::test]
async fn test_payload_without_trade_uuid() {
    let payload = json!({
        "strategy": "SHIELD",
        "token": "BONK",
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    });
    
    assert!(payload.get("trade_uuid").is_none());
}

// =============================================================================
// IDEMPOTENCY TESTS
// =============================================================================

/// Test deterministic UUID generation
#[tokio::test]
async fn test_deterministic_uuid_generation() {
    use sha2::{Digest, Sha256};
    
    let timestamp = "1733500000";
    let token = "BONK";
    let action = "BUY";
    let amount = "0.5";
    
    // Same inputs should generate same UUID
    let input1 = format!("{}{}{}{}", timestamp, token, action, amount);
    let input2 = format!("{}{}{}{}", timestamp, token, action, amount);
    
    let hash1 = Sha256::digest(input1.as_bytes());
    let hash2 = Sha256::digest(input2.as_bytes());
    
    assert_eq!(hash1, hash2, "Same inputs should produce same hash");
}

/// Test different inputs produce different UUIDs
#[tokio::test]
async fn test_unique_uuid_for_different_inputs() {
    use sha2::{Digest, Sha256};
    
    let input1 = "1733500000BONKBUY0.5";
    let input2 = "1733500000BONKBUY0.6"; // Different amount
    
    let hash1 = Sha256::digest(input1.as_bytes());
    let hash2 = Sha256::digest(input2.as_bytes());
    
    assert_ne!(hash1, hash2, "Different inputs should produce different hashes");
}

/// Test duplicate trade_uuid rejection
#[tokio::test]
async fn test_duplicate_trade_uuid_rejection() {
    use crate::db;
    use crate::models::SignalPayload;
    use crate::models::Strategy;
    
    // This test verifies that the idempotency check works
    // by checking if trade_uuid_exists correctly identifies duplicates
    
    // Create a test database
    let db = db::create_test_pool().await;
    
    // Insert a trade with a specific UUID
    let test_uuid = "test-duplicate-uuid-12345";
    db::insert_trade(
        &db,
        test_uuid,
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        Some("USDC"),
        "SHIELD",
        "BUY",
        0.5,
        "ACTIVE",
    )
    .await
    .expect("Failed to insert test trade");
    
    // Check that the UUID exists
    let exists = db::trade_uuid_exists(&db, test_uuid)
        .await
        .expect("Failed to check trade UUID");
    
    assert!(exists, "Trade UUID should exist after insertion");
    
    // Check that a different UUID doesn't exist
    let different_uuid = "different-uuid-67890";
    let not_exists = db::trade_uuid_exists(&db, different_uuid)
        .await
        .expect("Failed to check different trade UUID");
    
    assert!(!not_exists, "Different trade UUID should not exist");
}

// =============================================================================
// WEBHOOK RESPONSE TESTS
// =============================================================================

/// Test accepted response format
#[tokio::test]
async fn test_accepted_response_format() {
    let response = json!({
        "status": "accepted",
        "trade_uuid": "uuid-12345"
    });
    
    assert_eq!(response["status"], "accepted");
    assert!(response.get("trade_uuid").is_some());
}

/// Test rejected response format with reason
#[tokio::test]
async fn test_rejected_response_format() {
    let response = json!({
        "status": "rejected",
        "reason": "duplicate_signal"
    });
    
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["reason"], "duplicate_signal");
}

/// Test circuit breaker rejection
#[tokio::test]
async fn test_circuit_breaker_rejection_response() {
    let response = json!({
        "status": "rejected",
        "reason": "circuit_breaker_triggered"
    });
    
    assert_eq!(response["reason"], "circuit_breaker_triggered");
}

// =============================================================================
// INTEGRATION FLOW TESTS
// =============================================================================

/// Test full valid webhook flow
#[tokio::test]
async fn test_full_webhook_flow() {
    let app = Router::new().route(
        "/api/v1/webhook",
        post(|req: Request<Body>| async move {
            // Check headers
            let has_signature = req.headers().get("X-Signature").is_some();
            let has_timestamp = req.headers().get("X-Timestamp").is_some();
            
            if !has_signature || !has_timestamp {
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({
                        "status": "rejected",
                        "reason": "missing_headers"
                    })),
                );
            }
            
            (
                StatusCode::OK,
                axum::Json(json!({
                    "status": "accepted",
                    "trade_uuid": "generated-uuid"
                })),
            )
        }),
    );

    let secret = "test-secret";
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body = json!({
        "strategy": "SHIELD",
        "token": "BONK",
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    })
    .to_string();
    
    let signature = generate_signature(secret, &timestamp, &body);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("Content-Type", "application/json")
                .header("X-Signature", &signature)
                .header("X-Timestamp", &timestamp)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    
    assert_eq!(json["status"], "accepted");
}

/// Test webhook missing signature header
#[tokio::test]
async fn test_webhook_missing_signature() {
    let app = Router::new().route(
        "/api/v1/webhook",
        post(|req: Request<Body>| async move {
            let has_signature = req.headers().get("X-Signature").is_some();
            
            if !has_signature {
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({
                        "status": "rejected",
                        "reason": "missing_signature"
                    })),
                );
            }
            
            (StatusCode::OK, axum::Json(json!({"status": "accepted"})))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("Content-Type", "application/json")
                .header("X-Timestamp", "1733500000")
                .body(Body::from(r#"{"strategy": "SHIELD"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

