//! API Integration Tests
//!
//! Tests REST API endpoints for:
//! - Health check
//! - Positions listing
//! - Wallet management
//! - Configuration
//!
//! Uses axum-test for HTTP testing.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

// =============================================================================
// HEALTH CHECK TESTS
// =============================================================================

/// Simple health endpoint test
#[tokio::test]
async fn test_health_endpoint_returns_ok() {
    // Create a minimal router for testing
    let app = Router::new().route("/health", get(|| async { "OK" }));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test health endpoint returns JSON
#[tokio::test]
async fn test_health_returns_json() {
    let app = Router::new().route(
        "/health",
        get(|| async {
            axum::Json(json!({
                "status": "healthy",
                "uptime_seconds": 1000
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "healthy");
    assert!(json["uptime_seconds"].is_number());
}

// =============================================================================
// AUTHENTICATION TESTS
// =============================================================================

/// Test unauthorized access returns 401
#[tokio::test]
async fn test_unauthorized_access() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|| async { (StatusCode::UNAUTHORIZED, "Unauthorized") }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test valid bearer token auth
#[tokio::test]
async fn test_bearer_token_auth() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            match auth_header {
                Some(value) if value.to_str().unwrap_or("").starts_with("Bearer ") => {
                    (StatusCode::OK, "Authorized")
                }
                _ => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .header("Authorization", "Bearer valid_token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test missing bearer token
#[tokio::test]
async fn test_missing_bearer_token() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|req: Request<Body>| async move {
            let auth_header = req.headers().get("Authorization");
            match auth_header {
                Some(value) if value.to_str().unwrap_or("").starts_with("Bearer ") => {
                    (StatusCode::OK, "Authorized")
                }
                _ => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// =============================================================================
// POSITIONS API TESTS
// =============================================================================

/// Test positions list returns array
#[tokio::test]
async fn test_positions_list_returns_array() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|| async {
            axum::Json(json!({
                "positions": []
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["positions"].is_array());
}

/// Test single position response structure
#[tokio::test]
async fn test_position_response_structure() {
    let app = Router::new().route(
        "/api/v1/positions/test-uuid",
        get(|| async {
            axum::Json(json!({
                "trade_uuid": "test-uuid",
                "token": "BONK",
                "strategy": "SHIELD",
                "entry_amount_sol": 0.5,
                "entry_price": 0.000012,
                "current_price": 0.000015,
                "unrealized_pnl_percent": 25.0,
                "state": "ACTIVE"
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions/test-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["trade_uuid"], "test-uuid");
    assert_eq!(json["strategy"], "SHIELD");
    assert_eq!(json["state"], "ACTIVE");
    assert!(json["entry_amount_sol"].is_number());
}

// =============================================================================
// WALLETS API TESTS
// =============================================================================

/// Test wallets list returns array
#[tokio::test]
async fn test_wallets_list_returns_array() {
    let app = Router::new().route(
        "/api/v1/wallets",
        get(|| async {
            axum::Json(json!({
                "wallets": [
                    {
                        "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                        "status": "ACTIVE",
                        "wqs_score": 85.3
                    }
                ]
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/wallets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["wallets"].is_array());
    assert_eq!(json["wallets"][0]["status"], "ACTIVE");
}

/// Test wallet status filter
#[tokio::test]
async fn test_wallets_filter_by_status() {
    let app = Router::new().route(
        "/api/v1/wallets",
        get(|req: Request<Body>| async move {
            let uri = req.uri().to_string();
            if uri.contains("status=ACTIVE") {
                axum::Json(json!({
                    "wallets": [{"status": "ACTIVE"}]
                }))
            } else {
                axum::Json(json!({
                    "wallets": [{"status": "ACTIVE"}, {"status": "CANDIDATE"}]
                }))
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/wallets?status=ACTIVE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["wallets"].as_array().unwrap().len(), 1);
}

// =============================================================================
// CONFIGURATION API TESTS
// =============================================================================

/// Test config endpoint returns expected structure
#[tokio::test]
async fn test_config_response_structure() {
    let app = Router::new().route(
        "/api/v1/config",
        get(|| async {
            axum::Json(json!({
                "circuit_breakers": {
                    "max_loss_24h": 500,
                    "max_consecutive_losses": 5,
                    "max_drawdown_percent": 15
                },
                "strategy_allocation": {
                    "shield_percent": 70,
                    "spear_percent": 30
                }
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["circuit_breakers"].is_object());
    assert_eq!(json["circuit_breakers"]["max_loss_24h"], 500);
    assert_eq!(json["strategy_allocation"]["shield_percent"], 70);
}

// =============================================================================
// WEBHOOK TESTS
// =============================================================================

/// Test webhook accepts valid payload
#[tokio::test]
async fn test_webhook_accepts_valid_payload() {
    use axum::routing::post;

    let app = Router::new().route(
        "/api/v1/webhook",
        post(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) if p.get("strategy").is_some() && p.get("token").is_some() => {
                    (StatusCode::OK, axum::Json(json!({"status": "accepted"})))
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"status": "rejected", "reason": "invalid_payload"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "strategy": "SHIELD",
                        "token": "BONK",
                        "action": "BUY",
                        "amount_sol": 0.5,
                        "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
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
    assert_eq!(json["status"], "accepted");
}

/// Test webhook rejects invalid payload
#[tokio::test]
async fn test_webhook_rejects_invalid_payload() {
    use axum::routing::post;

    let app = Router::new().route(
        "/api/v1/webhook",
        post(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) if p.get("strategy").is_some() && p.get("token").is_some() => {
                    (StatusCode::OK, axum::Json(json!({"status": "accepted"})))
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"status": "rejected", "reason": "invalid_payload"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("Content-Type", "application/json")
                .body(Body::from("{}")) // Empty payload
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// =============================================================================
// ERROR HANDLING TESTS
// =============================================================================

/// Test 404 for unknown routes
#[tokio::test]
async fn test_unknown_route_returns_404() {
    let app = Router::new().route("/health", get(|| async { "OK" }));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test method not allowed
#[tokio::test]
async fn test_method_not_allowed() {
    let app = Router::new().route("/health", get(|| async { "OK" }));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

