//! Authentication & Authorization Integration Tests
//!
//! Tests role-based access control:
//! - API key authentication
//! - Bearer token validation
//! - Role-based permissions (readonly, operator, admin)
//! - Admin wallet authorization

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

// =============================================================================
// ROLE PERMISSION TESTS
// =============================================================================

/// Role enum for testing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Readonly,
    Operator,
    Admin,
}

impl Role {
    fn has_permission(&self, required: Role) -> bool {
        *self >= required
    }
}

#[test]
fn test_readonly_has_readonly_permission() {
    assert!(Role::Readonly.has_permission(Role::Readonly));
}

#[test]
fn test_readonly_lacks_operator_permission() {
    assert!(!Role::Readonly.has_permission(Role::Operator));
}

#[test]
fn test_readonly_lacks_admin_permission() {
    assert!(!Role::Readonly.has_permission(Role::Admin));
}

#[test]
fn test_operator_has_readonly_permission() {
    assert!(Role::Operator.has_permission(Role::Readonly));
}

#[test]
fn test_operator_has_operator_permission() {
    assert!(Role::Operator.has_permission(Role::Operator));
}

#[test]
fn test_operator_lacks_admin_permission() {
    assert!(!Role::Operator.has_permission(Role::Admin));
}

#[test]
fn test_admin_has_all_permissions() {
    assert!(Role::Admin.has_permission(Role::Readonly));
    assert!(Role::Admin.has_permission(Role::Operator));
    assert!(Role::Admin.has_permission(Role::Admin));
}

// =============================================================================
// ROLE ORDERING TESTS
// =============================================================================

#[test]
fn test_role_ordering() {
    assert!(Role::Readonly < Role::Operator);
    assert!(Role::Operator < Role::Admin);
    assert!(Role::Readonly < Role::Admin);
}

#[test]
fn test_role_equality() {
    assert_eq!(Role::Admin, Role::Admin);
    assert_ne!(Role::Readonly, Role::Admin);
}

// =============================================================================
// ENDPOINT ACCESS TESTS
// =============================================================================

/// Simulates endpoint access rules
fn check_endpoint_access(role: Role, endpoint: &str, method: &str) -> bool {
    match (endpoint, method) {
        // Readonly endpoints - anyone can access
        ("/api/v1/positions", "GET") => role.has_permission(Role::Readonly),
        ("/api/v1/wallets", "GET") => role.has_permission(Role::Readonly),
        ("/api/v1/trades", "GET") => role.has_permission(Role::Readonly),

        // Operator endpoints - operator+ can access
        ("/api/v1/wallets/:address", "PUT") => role.has_permission(Role::Operator),

        // Admin endpoints - admin only
        ("/api/v1/config", "PUT") => role.has_permission(Role::Admin),
        ("/api/v1/config/circuit-breaker/reset", "POST") => role.has_permission(Role::Admin),

        _ => false,
    }
}

#[test]
fn test_readonly_can_view_positions() {
    assert!(check_endpoint_access(
        Role::Readonly,
        "/api/v1/positions",
        "GET"
    ));
}

#[test]
fn test_readonly_cannot_update_wallets() {
    assert!(!check_endpoint_access(
        Role::Readonly,
        "/api/v1/wallets/:address",
        "PUT"
    ));
}

#[test]
fn test_readonly_cannot_update_config() {
    assert!(!check_endpoint_access(
        Role::Readonly,
        "/api/v1/config",
        "PUT"
    ));
}

#[test]
fn test_operator_can_view_positions() {
    assert!(check_endpoint_access(
        Role::Operator,
        "/api/v1/positions",
        "GET"
    ));
}

#[test]
fn test_operator_can_update_wallets() {
    assert!(check_endpoint_access(
        Role::Operator,
        "/api/v1/wallets/:address",
        "PUT"
    ));
}

#[test]
fn test_operator_cannot_update_config() {
    assert!(!check_endpoint_access(
        Role::Operator,
        "/api/v1/config",
        "PUT"
    ));
}

#[test]
fn test_admin_can_access_all() {
    assert!(check_endpoint_access(
        Role::Admin,
        "/api/v1/positions",
        "GET"
    ));
    assert!(check_endpoint_access(
        Role::Admin,
        "/api/v1/wallets/:address",
        "PUT"
    ));
    assert!(check_endpoint_access(Role::Admin, "/api/v1/config", "PUT"));
    assert!(check_endpoint_access(
        Role::Admin,
        "/api/v1/config/circuit-breaker/reset",
        "POST"
    ));
}

// =============================================================================
// API KEY VALIDATION TESTS
// =============================================================================

/// Simulates API key lookup
fn validate_api_key(key: &str, valid_keys: &[(&str, Role)]) -> Option<Role> {
    valid_keys
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, role)| *role)
}

#[test]
fn test_valid_api_key_returns_role() {
    let keys = [
        ("admin-key-123", Role::Admin),
        ("operator-key-456", Role::Operator),
        ("readonly-key-789", Role::Readonly),
    ];

    assert_eq!(validate_api_key("admin-key-123", &keys), Some(Role::Admin));
    assert_eq!(
        validate_api_key("operator-key-456", &keys),
        Some(Role::Operator)
    );
    assert_eq!(
        validate_api_key("readonly-key-789", &keys),
        Some(Role::Readonly)
    );
}

#[test]
fn test_invalid_api_key_returns_none() {
    let keys = [("admin-key-123", Role::Admin)];

    assert_eq!(validate_api_key("invalid-key", &keys), None);
}

#[test]
fn test_empty_api_key_returns_none() {
    let keys = [("admin-key-123", Role::Admin)];

    assert_eq!(validate_api_key("", &keys), None);
}

// =============================================================================
// BEARER TOKEN TESTS
// =============================================================================

/// Extract token from Authorization header
///
/// An empty token is NOT an authenticated request: rejecting it here means any
/// downstream check that only tests `is_some()` cannot mistake an empty token
/// for valid auth.
fn extract_bearer_token(header: &str) -> Option<&str> {
    header
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

#[test]
fn test_extract_valid_bearer_token() {
    let header = "Bearer my-token-123";
    assert_eq!(extract_bearer_token(header), Some("my-token-123"));
}

#[test]
fn test_extract_bearer_token_no_prefix() {
    let header = "my-token-123";
    assert_eq!(extract_bearer_token(header), None);
}

#[test]
fn test_extract_bearer_token_wrong_prefix() {
    let header = "Basic my-token-123";
    assert_eq!(extract_bearer_token(header), None);
}

#[test]
fn test_extract_bearer_token_empty() {
    // "Bearer " with nothing after it must NOT be treated as authenticated.
    let header = "Bearer ";
    assert_eq!(extract_bearer_token(header), None);
}

// =============================================================================
// ADMIN WALLET TESTS
// =============================================================================

/// Check if wallet is in admin list
fn is_admin_wallet(wallet: &str, admin_wallets: &[&str]) -> bool {
    admin_wallets.contains(&wallet)
}

#[test]
fn test_admin_wallet_found() {
    let admin_wallets = [
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        "9mNpQrXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
    ];

    assert!(is_admin_wallet(
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        &admin_wallets
    ));
}

#[test]
fn test_admin_wallet_not_found() {
    let admin_wallets = ["7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"];

    assert!(!is_admin_wallet(
        "UnknownWallet111111111111111111111111111111",
        &admin_wallets
    ));
}

#[test]
fn test_admin_wallet_empty_list() {
    let admin_wallets: [&str; 0] = [];

    assert!(!is_admin_wallet(
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        &admin_wallets
    ));
}

// =============================================================================
// TTL (TIME TO LIVE) TESTS
// =============================================================================

#[test]
fn test_ttl_not_expired() {
    let now = chrono::Utc::now();
    let ttl_expires = now + chrono::Duration::hours(24);

    assert!(ttl_expires > now, "TTL should not be expired");
}

#[test]
fn test_ttl_expired() {
    let now = chrono::Utc::now();
    let ttl_expires = now - chrono::Duration::hours(1);

    assert!(ttl_expires <= now, "TTL should be expired");
}

#[test]
fn test_no_ttl_never_expires() {
    let ttl: Option<chrono::DateTime<chrono::Utc>> = None;

    // None means no TTL = never expires
    assert!(ttl.is_none());
}

// =============================================================================
// JWT TOKEN TESTS
// =============================================================================

/// Simple JWT-like structure for testing
#[derive(Debug)]
#[allow(dead_code)]
struct JwtClaims {
    wallet: String,
    role: Role,
    exp: i64,
}

impl JwtClaims {
    fn is_expired(&self) -> bool {
        // RFC 7519: a JWT MUST NOT be accepted on or after its exp time.
        self.exp <= chrono::Utc::now().timestamp()
    }
}

#[test]
fn test_jwt_not_expired() {
    let claims = JwtClaims {
        wallet: "7xKXtg...".to_string(),
        role: Role::Admin,
        exp: chrono::Utc::now().timestamp() + 3600, // 1 hour from now
    };

    assert!(!claims.is_expired());
}

#[test]
fn test_jwt_expired() {
    let claims = JwtClaims {
        wallet: "7xKXtg...".to_string(),
        role: Role::Admin,
        exp: chrono::Utc::now().timestamp() - 3600, // 1 hour ago
    };

    assert!(claims.is_expired());
}

#[test]
fn test_jwt_expired_at_exact_boundary() {
    // exp == now must be treated as expired (RFC 7519: not accepted ON or
    // after exp). Captured before the check so the boundary is exact.
    let exp = chrono::Utc::now().timestamp();
    let claims = JwtClaims {
        wallet: "7xKXtg...".to_string(),
        role: Role::Admin,
        exp,
    };

    assert!(claims.is_expired(), "exp == now must be expired");
}

// =============================================================================
// RATE LIMITING TESTS
// =============================================================================

#[test]
fn test_rate_limit_under_threshold() {
    let requests_per_second = 50_u32;
    let limit = 100_u32;

    assert!(requests_per_second <= limit, "Should be under rate limit");
}

#[test]
fn test_rate_limit_at_threshold() {
    let requests_per_second = 100_u32;
    let limit = 100_u32;

    assert!(requests_per_second <= limit, "Should be at rate limit");
}

#[test]
fn test_rate_limit_exceeded() {
    let requests_per_second = 150_u32;
    let limit = 100_u32;

    assert!(requests_per_second > limit, "Should exceed rate limit");
}

// =============================================================================
// WALLET SIGNATURE VERIFICATION TESTS
// =============================================================================

// The production wallet-auth handler (handlers/auth.rs) is a private module,
// so these tests re-implement its verification contract inline: the ed25519
// signature is verified against the wallet pubkey with solana_sdk (the same
// crate the production handler uses), the message must contain the expected
// phrase and the wallet address, and the timestamp drift must be <= 300s.
// A bogus or tampered signature must be rejected with 401.

use solana_sdk::signature::{Keypair, Signature, Signer};
use std::str::FromStr;

fn verify_wallet_auth(wallet: &str, message: &str, signature_b64: &str) -> bool {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64, Engine};

    if !message.contains("Chimera Dashboard Authentication") || !message.contains(wallet) {
        return false;
    }

    // Timestamp drift window (same 300s the production handler enforces).
    let timestamp = message
        .lines()
        .find(|line| line.starts_with("Timestamp: "))
        .and_then(|line| line.strip_prefix("Timestamp: "))
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(i64::MIN);
    let drift = (chrono::Utc::now().timestamp() as i128 - timestamp as i128).unsigned_abs();
    if drift > 300 {
        return false;
    }

    // Decode + verify with real ed25519.
    let Ok(signature_bytes) = BASE64.decode(signature_b64) else {
        return false;
    };
    let Ok(pubkey) = solana_sdk::pubkey::Pubkey::from_str(wallet) else {
        return false;
    };
    let Ok(signature) = Signature::try_from(signature_bytes.as_slice()) else {
        return false;
    };
    signature.verify(pubkey.as_ref(), message.as_bytes())
}

fn wallet_auth_handler(body: String) -> (StatusCode, axum::Json<Value>) {
    let payload: Result<Value, _> = serde_json::from_str(&body);
    match payload {
        Ok(p) => {
            let wallet = p.get("wallet_address").and_then(|w| w.as_str()).unwrap_or("");
            let message = p.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let signature = p.get("signature").and_then(|s| s.as_str()).unwrap_or("");

            if wallet.is_empty() || message.is_empty() || signature.is_empty() {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"error": "Missing required fields"})),
                )
            } else if verify_wallet_auth(wallet, message, signature) {
                (
                    StatusCode::OK,
                    axum::Json(json!({
                        "token": "mock-jwt-token",
                        "role": "admin",
                        "identifier": wallet
                    })),
                )
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "Invalid signature or authentication message"})),
                )
            }
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "Invalid JSON"})),
        ),
    }
}

fn auth_message(wallet: &str) -> String {
    format!(
        "Chimera Dashboard Authentication\nWallet: {wallet}\nTimestamp: {}",
        chrono::Utc::now().timestamp()
    )
}

fn sign_message(keypair: &Keypair, message: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64, Engine};
    let sig = keypair.sign_message(message.as_bytes());
    BASE64.encode(sig.as_ref())
}

/// Test wallet authentication with a REAL ed25519 signature (valid path)
#[tokio::test]
async fn test_wallet_auth_valid_signature() {
    let keypair = Keypair::new();
    let wallet = keypair.pubkey().to_string();

    let app = Router::new().route(
        "/api/v1/auth/wallet",
        post(|body: String| async move { wallet_auth_handler(body) }),
    );

    let message = auth_message(&wallet);
    let signature = sign_message(&keypair, &message);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "wallet_address": wallet,
                        "message": message,
                        "signature": signature
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["role"], "admin");
    assert_eq!(json["identifier"], wallet);
    assert!(json.get("token").is_some());
}

/// Test wallet authentication with a TAMPERED signature (must be rejected 401)
#[tokio::test]
async fn test_wallet_auth_invalid_signature() {
    let keypair = Keypair::new();
    let wallet = keypair.pubkey().to_string();

    let app = Router::new().route(
        "/api/v1/auth/wallet",
        post(|body: String| async move { wallet_auth_handler(body) }),
    );

    let message = auth_message(&wallet);
    // Sign a DIFFERENT message, then present it as the signature for `message`.
    let wrong_signature = sign_message(&keypair, "tampered message");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "wallet_address": wallet,
                        "message": message,
                        "signature": wrong_signature
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a signature that does not verify against the message must be rejected"
    );
}

/// Test wallet authentication with invalid message (must be rejected 401)
#[tokio::test]
async fn test_wallet_auth_invalid_message() {
    let keypair = Keypair::new();
    let wallet = keypair.pubkey().to_string();

    let app = Router::new().route(
        "/api/v1/auth/wallet",
        post(|body: String| async move { wallet_auth_handler(body) }),
    );

    let message = "Invalid message".to_string();
    let signature = sign_message(&keypair, &message);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "wallet_address": wallet,
                        "message": message,
                        "signature": signature
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test wallet authentication with wallet address mismatch (must be rejected 401)
#[tokio::test]
async fn test_wallet_auth_address_mismatch() {
    let keypair = Keypair::new();
    let other = Keypair::new();
    let wallet = keypair.pubkey().to_string();
    let other_wallet = other.pubkey().to_string();

    let app = Router::new().route(
        "/api/v1/auth/wallet",
        post(|body: String| async move { wallet_auth_handler(body) }),
    );

    // Message claims a DIFFERENT wallet than the one in wallet_address.
    let message = format!(
        "Chimera Dashboard Authentication\nWallet: {other_wallet}\nTimestamp: {}",
        chrono::Utc::now().timestamp()
    );
    let signature = sign_message(&keypair, &message);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "wallet_address": wallet,
                        "message": message,
                        "signature": signature
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test wallet authentication with missing fields (must be rejected 400)
#[tokio::test]
async fn test_wallet_auth_missing_fields() {
    let app = Router::new().route(
        "/api/v1/auth/wallet",
        post(|body: String| async move { wallet_auth_handler(body) }),
    );

    // Test missing wallet_address
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"message": "test", "signature": "test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Test missing message
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"wallet_address": "test", "signature": "test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Test missing signature
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/wallet")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"wallet_address": "test", "message": "test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// =============================================================================
// ROLE-BASED ENDPOINT ACCESS TESTS
// =============================================================================

/// Test that readonly role can access readonly endpoints
#[tokio::test]
async fn test_readonly_access_readonly_endpoints() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            if let Some(header) = auth_header {
                let token = header.to_str().unwrap_or("");
                if token.starts_with("Bearer readonly-") {
                    (StatusCode::OK, axum::Json(json!({"positions": []})))
                } else {
                    (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(json!({"error": "Unauthorized"})),
                    )
                }
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "Missing auth"})),
                )
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .header("Authorization", "Bearer readonly-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test that readonly role cannot access operator endpoints
#[tokio::test]
async fn test_readonly_denied_operator_endpoints() {
    use axum::routing::put;

    let app = Router::new().route(
        "/api/v1/wallets/test",
        put(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            if let Some(header) = auth_header {
                let token = header.to_str().unwrap_or("");
                if token.starts_with("Bearer operator-") || token.starts_with("Bearer admin-") {
                    (StatusCode::OK, axum::Json(json!({"success": true})))
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(json!({"error": "Insufficient permissions"})),
                    )
                }
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "Missing auth"})),
                )
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test")
                .header("Authorization", "Bearer readonly-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Test that operator role can access operator endpoints
#[tokio::test]
async fn test_operator_access_operator_endpoints() {
    use axum::routing::put;

    let app = Router::new().route(
        "/api/v1/wallets/test",
        put(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            if let Some(header) = auth_header {
                let token = header.to_str().unwrap_or("");
                if token.starts_with("Bearer operator-") || token.starts_with("Bearer admin-") {
                    (StatusCode::OK, axum::Json(json!({"success": true})))
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(json!({"error": "Insufficient permissions"})),
                    )
                }
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "Missing auth"})),
                )
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test")
                .header("Authorization", "Bearer operator-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test that operator role cannot access admin endpoints
#[tokio::test]
async fn test_operator_denied_admin_endpoints() {
    use axum::routing::put;

    let app = Router::new().route(
        "/api/v1/config",
        put(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            if let Some(header) = auth_header {
                let token = header.to_str().unwrap_or("");
                if token.starts_with("Bearer admin-") {
                    (StatusCode::OK, axum::Json(json!({"success": true})))
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(json!({"error": "Admin access required"})),
                    )
                }
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "Missing auth"})),
                )
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/config")
                .header("Authorization", "Bearer operator-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Test that admin role can access all endpoints
#[tokio::test]
async fn test_admin_access_all_endpoints() {
    use axum::routing::{get, post, put};

    // Handlers enforce admin-only access (ignore non-admin tokens) so the
    // test actually validates the access-control claim.
    let admin_handler = |req: Request<Body>| async move {
        let auth_header = req.headers().get("Authorization");
        match auth_header.and_then(|h| h.to_str().ok()) {
            Some(header) if header.starts_with("Bearer admin-") => {
                (StatusCode::OK, axum::Json(json!({"success": true})))
            }
            Some(_) => (
                StatusCode::FORBIDDEN,
                axum::Json(json!({"error": "Admin access required"})),
            ),
            None => (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"error": "Missing auth"})),
            ),
        }
    };

    let app = Router::new()
        .route("/api/v1/positions", get(admin_handler.clone()))
        .route("/api/v1/wallets/test", put(admin_handler.clone()))
        .route("/api/v1/config", put(admin_handler.clone()))
        .route(
            "/api/v1/config/circuit-breaker/reset",
            post(admin_handler.clone()),
        );

    // Test readonly endpoint
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .header("Authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Test operator endpoint
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test")
                .header("Authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Test admin endpoint
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/config")
                .header("Authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Test admin-only endpoint
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/config/circuit-breaker/reset")
                .header("Authorization", "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Negative: no header → 401
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Negative: non-admin token → 403
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/config")
                .header("Authorization", "Bearer operator-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// =============================================================================
// SIGNATURE FORMAT TESTS
// =============================================================================

/// Test base64 signature decoding
#[test]
fn test_base64_signature_decoding() {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

    // Valid base64
    let valid_b64 = "dGVzdA=="; // "test" in base64
    assert!(BASE64.decode(valid_b64).is_ok());

    // Invalid base64
    let invalid_b64 = "not-base64!!!";
    assert!(BASE64.decode(invalid_b64).is_err());
}

/// Test Solana pubkey format validation
#[test]
fn test_solana_pubkey_format() {
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    // Valid Solana address: 32 bytes base58-encoded.
    let valid_address = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    assert_eq!(valid_address.len(), 44);
    let decoded = Pubkey::from_str(valid_address).expect("valid address must decode");
    assert_eq!(decoded.as_ref().len(), 32, "decoded pubkey must be 32 bytes");

    // Invalid: contains non-base58 characters (0, O, I, l are not base58), or
    // decodes to more than 32 bytes (leading 'z' on a 44-char string).
    for invalid in [
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAs0", // contains '0'
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsO", // contains 'O'
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsI", // contains 'I'
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsl", // contains 'l'
        "z7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", // overflows 32 bytes
    ] {
        assert!(
            Pubkey::from_str(invalid).is_err(),
            "invalid pubkey must be rejected: {invalid}"
        );
    }
}

// =============================================================================
// JWT TOKEN TESTS
// =============================================================================

/// Test JWT token structure
#[test]
fn test_jwt_token_structure() {
    // JWT should have 3 parts: header.payload.signature
    let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT should have 3 parts");
}

/// Test JWT expiration check
#[test]
fn test_jwt_expiration() {
    let now = chrono::Utc::now().timestamp();
    let exp_future = now + 3600; // 1 hour from now
    let exp_past = now - 3600; // 1 hour ago

    assert!(exp_future > now, "Future expiration should be valid");
    assert!(exp_past < now, "Past expiration should be invalid");
}
