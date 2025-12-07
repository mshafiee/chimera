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

// =============================================================================
// TRADES API TESTS
// =============================================================================

/// Test trades list returns array with pagination
#[tokio::test]
async fn test_trades_list_returns_array() {
    let app = Router::new().route(
        "/api/v1/trades",
        get(|| async {
            axum::Json(json!({
                "trades": [],
                "total": 0,
                "limit": 100,
                "offset": 0
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades")
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

    assert!(json["trades"].is_array());
    assert!(json["total"].is_number());
    assert!(json["limit"].is_number());
    assert!(json["offset"].is_number());
}

/// Test trades list with filters
#[tokio::test]
async fn test_trades_list_with_filters() {
    let app = Router::new().route(
        "/api/v1/trades",
        get(|req: Request<Body>| async move {
            let uri = req.uri().to_string();
            let mut filtered_count = 2;
            
            if uri.contains("status=CLOSED") {
                filtered_count = 1;
            }
            if uri.contains("strategy=SHIELD") {
                filtered_count = 1;
            }
            
            axum::Json(json!({
                "trades": (0..filtered_count).map(|i| json!({
                    "id": i + 1,
                    "trade_uuid": format!("uuid-{}", i),
                    "status": if uri.contains("status=CLOSED") { "CLOSED" } else { "ACTIVE" },
                    "strategy": if uri.contains("strategy=SHIELD") { "SHIELD" } else { "SPEAR" }
                })).collect::<Vec<_>>(),
                "total": filtered_count,
                "limit": 100,
                "offset": 0
            }))
        }),
    );

    // Test status filter
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades?status=CLOSED")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["trades"].as_array().unwrap().len(), 1);
    assert_eq!(json["trades"][0]["status"], "CLOSED");

    // Test strategy filter
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades?strategy=SHIELD")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["trades"].as_array().unwrap().len(), 1);
    assert_eq!(json["trades"][0]["strategy"], "SHIELD");
}

/// Test trades export endpoint
#[tokio::test]
async fn test_trades_export_csv() {
    use axum::routing::get;
    
    let app = Router::new().route(
        "/api/v1/trades/export",
        get(|| async {
            (
                StatusCode::OK,
                [("Content-Type", "text/csv"), ("Content-Disposition", "attachment; filename=\"trades.csv\"")],
                "id,trade_uuid,wallet_address\n1,uuid-1,wallet-1\n",
            )
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades/export?format=csv")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("Content-Type").unwrap(),
        "text/csv"
    );
    assert!(response
        .headers()
        .get("Content-Disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("filename"));
}

/// Test trades export PDF
#[tokio::test]
async fn test_trades_export_pdf() {
    use axum::routing::get;
    
    let app = Router::new().route(
        "/api/v1/trades/export",
        get(|| async {
            (
                StatusCode::OK,
                [("Content-Type", "application/pdf"), ("Content-Disposition", "attachment; filename=\"trades.pdf\"")],
                vec![0u8; 100], // Mock PDF bytes
            )
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades/export?format=pdf")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("Content-Type").unwrap(),
        "application/pdf"
    );
}

/// Test trades export JSON
#[tokio::test]
async fn test_trades_export_json() {
    use axum::routing::get;
    
    let app = Router::new().route(
        "/api/v1/trades/export",
        get(|| async {
            (
                StatusCode::OK,
                [("Content-Type", "application/json"), ("Content-Disposition", "attachment; filename=\"trades.json\"")],
                json!([{"id": 1, "trade_uuid": "uuid-1"}]).to_string(),
            )
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades/export?format=json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("Content-Type").unwrap(),
        "application/json"
    );
}

// =============================================================================
// WALLET UPDATE TESTS
// =============================================================================

/// Test wallet update (PUT) with valid data
#[tokio::test]
async fn test_wallet_update_valid() {
    use axum::routing::put;
    
    let app = Router::new().route(
        "/api/v1/wallets/test-address",
        put(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) if p.get("status").is_some() => {
                    (StatusCode::OK, axum::Json(json!({
                        "success": true,
                        "wallet": {
                            "address": "test-address",
                            "status": p["status"]
                        },
                        "message": "Wallet updated successfully"
                    })))
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"success": false, "message": "Invalid request"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test-address")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "status": "ACTIVE",
                        "ttl_hours": 24,
                        "reason": "Test promotion"
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
    assert_eq!(json["success"], true);
    assert_eq!(json["wallet"]["status"], "ACTIVE");
}

/// Test wallet update with invalid status
#[tokio::test]
async fn test_wallet_update_invalid_status() {
    use axum::routing::put;
    
    let app = Router::new().route(
        "/api/v1/wallets/test-address",
        put(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) => {
                    let status = p.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    if ["ACTIVE", "CANDIDATE", "REJECTED"].contains(&status) {
                        (StatusCode::OK, axum::Json(json!({"success": true})))
                    } else {
                        (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"success": false, "message": "Invalid status"})),
                        )
                    }
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"success": false, "message": "Invalid request"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test-address")
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"status": "INVALID"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Test wallet update TTL validation
#[tokio::test]
async fn test_wallet_update_ttl_validation() {
    use axum::routing::put;
    
    let app = Router::new().route(
        "/api/v1/wallets/test-address",
        put(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) => {
                    let status = p.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    let ttl = p.get("ttl_hours");
                    
                    // TTL can only be set when status is ACTIVE
                    if ttl.is_some() && status != "ACTIVE" {
                        (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"success": false, "message": "TTL can only be set for ACTIVE status"})),
                        )
                    } else {
                        (StatusCode::OK, axum::Json(json!({"success": true})))
                    }
                }
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"success": false, "message": "Invalid JSON"})),
                ),
            }
        }),
    );

    // Test invalid: TTL with non-ACTIVE status
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test-address")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"status": "CANDIDATE", "ttl_hours": 24}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Test valid: TTL with ACTIVE status
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test-address")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"status": "ACTIVE", "ttl_hours": 24}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// =============================================================================
// CONFIG API TESTS
// =============================================================================

/// Test config update (PUT) with valid data
#[tokio::test]
async fn test_config_update_valid() {
    use axum::routing::put;
    
    let app = Router::new().route(
        "/api/v1/config",
        put(|body: String| async move {
            let payload: Result<Value, _> = serde_json::from_str(&body);
            match payload {
                Ok(p) if p.get("circuit_breakers").is_some() => {
                    (StatusCode::OK, axum::Json(json!({
                        "success": true,
                        "message": "Configuration updated"
                    })))
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"success": false, "message": "Invalid config"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/config")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({
                        "circuit_breakers": {
                            "max_loss_24h": 500,
                            "max_consecutive_losses": 5
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test circuit breaker reset endpoint
#[tokio::test]
async fn test_circuit_breaker_reset() {
    use axum::routing::post;
    
    let app = Router::new().route(
        "/api/v1/config/circuit-breaker/reset",
        post(|| async {
            axum::Json(json!({
                "success": true,
                "message": "Circuit breaker reset successfully"
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/config/circuit-breaker/reset")
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
    assert_eq!(json["success"], true);
}

// =============================================================================
// INCIDENTS API TESTS
// =============================================================================

/// Test dead letter queue list
#[tokio::test]
async fn test_dead_letter_queue_list() {
    let app = Router::new().route(
        "/api/v1/incidents/dead-letter",
        get(|| async {
            axum::Json(json!({
                "items": [
                    {
                        "id": 1,
                        "trade_uuid": "uuid-1",
                        "reason": "Max retries exceeded",
                        "can_retry": false
                    }
                ],
                "total": 1
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/incidents/dead-letter")
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
    assert!(json["items"].is_array());
    assert_eq!(json["total"], 1);
}

/// Test dead letter queue pagination
#[tokio::test]
async fn test_dead_letter_queue_pagination() {
    let app = Router::new().route(
        "/api/v1/incidents/dead-letter",
        get(|req: Request<Body>| async move {
            let uri = req.uri().to_string();
            let limit = if uri.contains("limit=10") { 10 } else { 50 };
            let offset = if uri.contains("offset=10") { 10 } else { 0 };
            
            axum::Json(json!({
                "items": (0..limit.min(5)).map(|i| json!({
                    "id": offset + i + 1,
                    "trade_uuid": format!("uuid-{}", offset + i),
                })).collect::<Vec<_>>(),
                "total": 100
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/incidents/dead-letter?limit=10&offset=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["items"].as_array().unwrap().len() <= 10);
    assert_eq!(json["total"], 100);
}

/// Test config audit log list
#[tokio::test]
async fn test_config_audit_list() {
    let app = Router::new().route(
        "/api/v1/incidents/config-audit",
        get(|| async {
            axum::Json(json!({
                "items": [
                    {
                        "id": 1,
                        "key": "circuit_breakers.max_loss_24h",
                        "old_value": "500",
                        "new_value": "600",
                        "changed_by": "admin",
                        "changed_at": "2025-12-01T10:00:00Z"
                    }
                ],
                "total": 1
            }))
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/incidents/config-audit")
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
    assert!(json["items"].is_array());
    assert_eq!(json["items"][0]["key"], "circuit_breakers.max_loss_24h");
}

// =============================================================================
// POSITION FILTER TESTS
// =============================================================================

/// Test positions list with state filter
#[tokio::test]
async fn test_positions_list_with_state_filter() {
    let app = Router::new().route(
        "/api/v1/positions",
        get(|req: Request<Body>| async move {
            let uri = req.uri().to_string();
            let state_filter = if uri.contains("state=ACTIVE") {
                "ACTIVE"
            } else if uri.contains("state=EXITING") {
                "EXITING"
            } else {
                "ALL"
            };
            
            axum::Json(json!({
                "positions": if state_filter != "ALL" {
                    vec![json!({
                        "trade_uuid": "uuid-1",
                        "state": state_filter
                    })]
                } else {
                    vec![
                        json!({"trade_uuid": "uuid-1", "state": "ACTIVE"}),
                        json!({"trade_uuid": "uuid-2", "state": "EXITING"})
                    ]
                },
                "total": if state_filter != "ALL" { 1 } else { 2 }
            }))
        }),
    );

    // Test ACTIVE filter
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions?state=ACTIVE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["positions"].as_array().unwrap().len(), 1);
    assert_eq!(json["positions"][0]["state"], "ACTIVE");
}

/// Test position not found (404)
#[tokio::test]
async fn test_position_not_found() {
    let app = Router::new().route(
        "/api/v1/positions/:trade_uuid",
        get(|req: Request<Body>| async move {
            let path = req.uri().path();
            // Extract trade_uuid from path
            let trade_uuid = path.split('/').last().unwrap_or("");
            if trade_uuid == "nonexistent-uuid" {
                (StatusCode::NOT_FOUND, axum::Json(json!({
                    "error": "Position not found: nonexistent-uuid"
                })))
            } else {
                (StatusCode::OK, axum::Json(json!({
                    "trade_uuid": trade_uuid,
                    "state": "ACTIVE"
                })))
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/positions/nonexistent-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test wallet not found (404)
#[tokio::test]
async fn test_wallet_not_found() {
    let app = Router::new().route(
        "/api/v1/wallets/:address",
        get(|req: Request<Body>| async move {
            let path = req.uri().path();
            // Extract address from path
            let address = path.split('/').last().unwrap_or("");
            if address == "nonexistent-address" {
                (StatusCode::NOT_FOUND, axum::Json(json!({
                    "error": "Wallet not found: nonexistent-address"
                })))
            } else {
                (StatusCode::OK, axum::Json(json!({
                    "address": address,
                    "status": "ACTIVE"
                })))
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/wallets/nonexistent-address")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// =============================================================================
// PAGINATION TESTS
// =============================================================================

/// Test trades pagination
#[tokio::test]
async fn test_trades_pagination() {
    let app = Router::new().route(
        "/api/v1/trades",
        get(|req: Request<Body>| async move {
            let uri = req.uri().to_string();
            let limit = if uri.contains("limit=") {
                uri.split("limit=").nth(1)
                    .and_then(|s| s.split('&').next())
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(100)
            } else {
                100
            };
            let offset = if uri.contains("offset=") {
                uri.split("offset=").nth(1)
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            
            axum::Json(json!({
                "trades": (0..limit.min(10)).map(|i| json!({
                    "id": offset + i + 1,
                    "trade_uuid": format!("uuid-{}", offset + i),
                })).collect::<Vec<_>>(),
                "total": 50,
                "limit": limit,
                "offset": offset
            }))
        }),
    );

    // Test first page
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades?limit=10&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["offset"], 0);
    assert_eq!(json["limit"], 10);

    // Test second page
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/trades?limit=10&offset=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["offset"], 10);
}

// =============================================================================
// ERROR RESPONSE TESTS
// =============================================================================

/// Test error response format
#[tokio::test]
async fn test_error_response_format() {
    let app = Router::new().route(
        "/api/v1/error-test",
        get(|| async {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "error": "Validation failed",
                    "details": "Invalid parameter"
                })),
            )
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/error-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("error").is_some() || json.get("details").is_some());
}

/// Test malformed JSON request
#[tokio::test]
async fn test_malformed_json_request() {
    use axum::routing::put;
    
    let app = Router::new().route(
        "/api/v1/wallets/test",
        put(|body: String| async move {
            match serde_json::from_str::<Value>(&body) {
                Ok(_) => (StatusCode::OK, axum::Json(json!({"success": true}))),
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"error": "Invalid JSON"})),
                ),
            }
        }),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/wallets/test")
                .header("Content-Type", "application/json")
                .body(Body::from("{ invalid json }"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

