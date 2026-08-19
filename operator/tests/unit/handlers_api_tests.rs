//! HTTP handler tests for `operator/src/handlers/api.rs`.
//!
//! End-to-end axum oneshot tests against a real Postgres test database:
//! seed rows, call GET/POST/PUT endpoints on the real router, assert JSON.

use axum::{
    http::StatusCode,
    routing::{get, post},
    Router,
};
use chimera_operator::handlers::{require_role_from_request, ApiState};
use chimera_operator::middleware::{AuthExtension, AuthenticatedUser, Role};
use chimera_operator::monitoring::rate_limiter::RateLimiter;
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;
use tower::ServiceExt;

#[path = "../common/harness.rs"]
mod harness;

use harness::{
    api_get, api_post, api_put, auth_headers, build, json_body, seed_closed_position_with_pnl,
    seed_config_audit, seed_dead_letter, seed_dead_letter_with_payload, seed_position,
    seed_reconciliation_run, seed_shadow_exit, seed_shadow_position, seed_trade, seed_wallet,
    test_config, Harness, TOKEN_A, TOKEN_B, WALLET_A, WALLET_B,
};

/// Assert a JSON value equals a decimal string, ignoring trailing-zero scale
/// differences (Decimal serializes with full scale, e.g. "0.001700000000000000").
fn assert_dec_eq(body: &serde_json::Value, key: &str, expected: &str) {
    let actual = body[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"));
    let actual = rust_decimal::Decimal::from_str(actual).unwrap();
    let expected = rust_decimal::Decimal::from_str(expected).unwrap();
    assert_eq!(actual, expected, "field {key}");
}

// =============================================================================
// POSITIONS
// =============================================================================

#[tokio::test]
async fn list_positions_empty_and_with_state_filter() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/positions", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["positions"].as_array().unwrap().len(), 0);
    assert_eq!(body["total"], 0);
    assert!(body["total_unrealized_pnl_sol"].is_null() == false);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.5", None,
    )
    .await;
    seed_position(
        &h.pool, "t1", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.5", "0.01", None,
    )
    .await;
    seed_trade(
        &h.pool, "t2", WALLET_A, TOKEN_A, "BUY", "EXITING", "SHIELD", "2.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t2", WALLET_A, TOKEN_A, "SHIELD", "EXITING", "2.0", "0.02", None,
    )
    .await;
    seed_trade(
        &h.pool, "t3", WALLET_A, TOKEN_A, "BUY", "CLOSED", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool,
        "t3",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "CLOSED",
        "1.0",
        "0.03",
        Some(chrono::Utc::now()),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/positions", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 3);

    let resp = api_get(&h.app, "/api/v1/positions?state=ACTIVE", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["positions"][0]["state"], "ACTIVE");
}

#[tokio::test]
async fn get_position_found_and_not_found() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.5", None,
    )
    .await;
    seed_position(
        &h.pool, "t1", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.5", "0.01", None,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/positions/t1", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["trade_uuid"], "t1");

    let resp = api_get(&h.app, "/api/v1/positions/missing-uuid", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// =============================================================================
// WALLETS
// =============================================================================

#[tokio::test]
async fn list_wallets_filter_by_status() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet(&h.pool, WALLET_B, "CANDIDATE", Some(50.0)).await;

    let resp = api_get(&h.app, "/api/v1/wallets", auth_headers(Role::Readonly)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 2);

    let resp = api_get(
        &h.app,
        "/api/v1/wallets?status=ACTIVE",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["wallets"][0]["address"], WALLET_A);
}

#[tokio::test]
async fn get_wallet_validation_and_not_found() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;

    // Invalid address format → 400
    let resp = api_get(
        &h.app,
        "/api/v1/wallets/not-a-pubkey",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Valid pubkey format but not in DB → 404
    let resp = api_get(
        &h.app,
        "/api/v1/wallets/5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Found
    let resp = api_get(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["address"], WALLET_A);
    assert_eq!(body["status"], "ACTIVE");
}

#[tokio::test]
async fn update_wallet_validation_errors() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "CANDIDATE", Some(80.0)).await;

    // Non-operator role → 403
    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Readonly),
        json!({"status": "ACTIVE"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Invalid status → 400
    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "BOGUS"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // TTL with non-ACTIVE status → 400
    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "CANDIDATE", "ttl_hours": 24}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Negative TTL → 400
    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "ACTIVE", "ttl_hours": -1}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // TTL above i32::MAX → 400
    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "ACTIVE", "ttl_hours": 3_000_000_000i64}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Unknown wallet → 404
    let resp = api_put(
        &h.app,
        "/api/v1/wallets/5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        auth_headers(Role::Operator),
        json!({"status": "ACTIVE"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_wallet_promote_to_active_success() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "CANDIDATE", Some(85.0)).await;

    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Admin),
        json!({"status": "ACTIVE", "ttl_hours": 168, "reason": "test promotion"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["wallet"]["status"], "ACTIVE");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Status changed to ACTIVE"));

    // Config audit row written
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM config_audit WHERE key = $1")
        .bind(format!("wallet:{}", WALLET_A))
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Wallet state persisted
    let (status,): (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET_A)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE");

    // Auto webhook registration spawns (via mock): wait for the
    // wallet_monitoring row to appear, then let the spawn finish its
    // success logging before the test ends.
    wait_for(
        || async {
            let (count,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM wallet_monitoring WHERE wallet_address = $1")
                    .bind(WALLET_A)
                    .fetch_one(&h.pool)
                    .await
                    .unwrap();
            count > 0
        },
        3000,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
}

#[tokio::test]
async fn update_wallet_demote_from_active_cleanup_webhook() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet_monitoring_row(&h, WALLET_A, Some("mock-webhook-abc"), true).await;

    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "REJECTED", "reason": "underperforming"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["wallet"]["status"], "REJECTED");

    // Cleanup spawns (dry-run delete): wait for the audit event, then let the
    // spawn finish its success logging.
    wait_for(
        || async {
            let (count,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM webhook_lifecycle_audit WHERE wallet_address = $1 AND action = 'delete'",
            )
            .bind(WALLET_A)
            .fetch_one(&h.pool)
            .await
            .unwrap();
            count > 0
        },
        3000,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

#[tokio::test]
async fn update_wallet_demote_without_monitoring_row() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "CANDIDATE"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["wallet"]["status"], "CANDIDATE");
    // Let the cleanup spawn finish its failure logging.
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
}

#[tokio::test]
async fn update_wallet_promote_with_toxic_detector() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "CANDIDATE", Some(85.0)).await;

    let toxic = Arc::new(chimera_operator::experiment::ToxicFlowDetector::new(
        Default::default(),
    ));
    let state = Arc::new(ApiState {
        db: h.db.clone(),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        config: h.config.clone(),
        notifier: h.api_state.notifier.clone(),
        engine: None,
        metrics: h.api_state.metrics.clone(),
        signal_aggregator: None,
        market_regime_detector: None,
        helius_client: h.api_state.helius_client.clone(),
        webhook_rate_limiter: h.api_state.webhook_rate_limiter.clone(),
        price_cache: h.api_state.price_cache.clone(),
        toxic_detector: Some(toxic),
        run_context: None,
        decision_recorder: None,
        profitability_verdict: h.api_state.profitability_verdict.clone(),
    });
    let app = Router::new()
        .route(
            "/wallets/:address",
            axum::routing::put(chimera_operator::handlers::update_wallet),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(chimera_operator::middleware::AuthState::with_auth_config(
                HashMap::from([("test-operator".to_string(), Role::Operator)]),
                "test".to_string(),
            )),
            chimera_operator::middleware::bearer_auth,
        ));

    let resp = api_put(
        &app,
        &format!("/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "ACTIVE", "ttl_hours": 24}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Let the auto-register spawn finish its logging.
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
}

#[tokio::test]
async fn update_wallet_same_status_no_notification() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet_monitoring_row(&h, WALLET_A, Some("mock-webhook-abc"), true).await;

    let resp = api_put(
        &h.app,
        &format!("/api/v1/wallets/{}", WALLET_A),
        auth_headers(Role::Operator),
        json!({"status": "ACTIVE"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // No lifecycle audit events should be emitted for a same-status update.
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM webhook_lifecycle_audit WHERE wallet_address = $1")
            .bind(WALLET_A)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

// =============================================================================
// SHADOW LEADERBOARD
// =============================================================================

#[tokio::test]
async fn shadow_leaderboard_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/shadow/leaderboard", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body.as_array().unwrap().len(), 0);

    // Seed admitted non-pump positions with exits.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_shadow_position(
        &h.pool,
        "shadow-1",
        WALLET_A,
        TOKEN_A,
        true,
        chrono::Utc::now(),
    )
    .await;
    seed_shadow_exit(&h.pool, "shadow-1", "mirror_main", "0.25", "8.0").await;
    // Second exit on a DIFFERENT token: same-hour duplicates on one
    // (wallet, token) are deduped since 2026-08-14.
    seed_shadow_position(
        &h.pool,
        "shadow-2",
        WALLET_A,
        TOKEN_B,
        true,
        chrono::Utc::now(),
    )
    .await;
    seed_shadow_exit(&h.pool, "shadow-2", "mirror_main", "-0.05", "-2.0").await;
    // Pump token excluded by the NOT LIKE '%pump' filter.
    seed_shadow_position(
        &h.pool,
        "shadow-3",
        WALLET_A,
        "tokpump",
        true,
        chrono::Utc::now(),
    )
    .await;
    seed_shadow_exit(&h.pool, "shadow-3", "mirror_main", "9.0", "50.0").await;
    // Duplicate of shadow-1 (same wallet+token, same hour): must be dropped
    // by the dedup so the row below still reports exits_7d = 2.
    seed_shadow_position(
        &h.pool,
        "shadow-4",
        WALLET_A,
        TOKEN_A,
        true,
        chrono::Utc::now(),
    )
    .await;
    seed_shadow_exit(&h.pool, "shadow-4", "mirror_main", "9.0", "50.0").await;

    let resp = api_get(&h.app, "/api/v1/shadow/leaderboard", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["wallet_address"], WALLET_A);
    assert_eq!(rows[0]["exits_7d"], 2);
    assert_eq!(rows[0]["wins_7d"], 1);
    assert_eq!(rows[0]["total_pnl_sol"], 0.2);
}

// =============================================================================
// CONFIG
// =============================================================================

#[tokio::test]
async fn get_config_shape() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/config", auth_headers(Role::Admin)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["jito_enabled"], true);
    assert!(body["monitoring"].is_object());
    assert!(body["circuit_breakers"].is_object());
    assert!(body["position_sizing"].is_object());
    assert!(body["notifications"]["rules"]["wallet_promoted"] == true);
}

#[tokio::test]
async fn get_config_without_monitoring() {
    let mut config = test_config();
    config.monitoring = None;
    let h = build(config).await;

    let resp = api_get(&h.app, "/api/v1/config", auth_headers(Role::Admin)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["monitoring"].is_null());
}

#[tokio::test]
async fn update_config_full_success() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Admin);

    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({
            "circuit_breakers": {
                "max_loss_24h": 600.0,
                "max_consecutive_losses": 4,
                "max_drawdown_percent": 20.0,
                "cool_down_minutes": 45
            },
            "strategy_allocation": {"shield_percent": 60, "spear_percent": 40},
            "strategy": {"max_position_sol": 2.0, "min_position_sol": 0.05},
            "monitoring": {
                "enabled": true,
                "webhook_registration_batch_size": 5,
                "webhook_registration_delay_ms": 1500,
                "webhook_processing_rate_limit": 30,
                "rpc_polling_enabled": true,
                "rpc_poll_interval_secs": 45,
                "rpc_poll_batch_size": 10,
                "rpc_poll_rate_limit": 20,
                "max_active_wallets": 15
            },
            "profit_management": {
                "targets": [5.0, 10.0, 20.0],
                "tiered_exit_percent": 50.0,
                "trailing_stop_activation": 3.0,
                "trailing_stop_distance": 1.5,
                "hard_stop_loss": 25.0,
                "time_exit_hours": 48
            },
            "position_sizing": {
                "base_size_sol": 0.5,
                "max_size_sol": 2.0,
                "min_size_sol": 0.1,
                "consensus_multiplier": 2.5,
                "max_concurrent_positions": 8
            },
            "mev_protection": {
                "always_use_jito": true,
                "exit_tip_sol": 0.0004,
                "consensus_tip_sol": 0.0003,
                "standard_tip_sol": 0.0002
            },
            "token_safety": {
                "min_liquidity_shield_usd": 50000.0,
                "min_liquidity_spear_usd": 100000.0,
                "honeypot_detection_enabled": true,
                "cache_capacity": 2000,
                "cache_ttl_seconds": 600
            },
            "notifications": {
                "telegram": {"enabled": false, "rate_limit_seconds": 5},
                "rules": {
                    "circuit_breaker_triggered": true,
                    "wallet_drained": true,
                    "position_exited": false,
                    "wallet_promoted": true,
                    "daily_summary": false,
                    "rpc_fallback": true
                },
                "daily_summary": {"enabled": true, "hour_utc": 8, "minute": 30}
            },
            "queue": {"capacity": 500, "load_shed_threshold_percent": 80}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["circuit_breakers"]["max_loss_24h"], 600.0);
    assert_eq!(body["strategy_allocation"]["shield_percent"], 60);
    assert_eq!(body["monitoring"]["max_active_wallets"], 15);

    // Verify in-memory config mutated
    let config = h.config.read().await;
    assert_eq!(config.circuit_breakers.max_consecutive_losses, 4);
    assert_eq!(config.queue.capacity, 500);
    assert_eq!(config.notifications.daily_summary.minute, 30);
    drop(config);

    // Verify audit rows written outside the lock
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM config_audit")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(count >= 20, "expected many audit entries, got {}", count);
}

#[tokio::test]
async fn update_config_forbidden_for_non_admin() {
    let h = build(test_config()).await;

    let resp = api_put(
        &h.app,
        "/api/v1/config",
        auth_headers(Role::Operator),
        json!({"queue": {"capacity": 100}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_config_validation_errors() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Admin);

    // Allocation must sum to 100
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"strategy_allocation": {"shield_percent": 70, "spear_percent": 20}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Webhook rate limit > 50
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"monitoring": {"webhook_processing_rate_limit": 60}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // RPC poll rate limit > 50
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"monitoring": {"rpc_poll_rate_limit": 60}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Profit targets not ascending
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"profit_management": {"targets": [10.0, 5.0]}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Tiered exit percent out of range
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"profit_management": {"tiered_exit_percent": 101.0}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Hard stop loss out of range
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"profit_management": {"hard_stop_loss": 150.0}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Base size out of min/max bounds (default min 0.1, max 1.0 → use 5.0)
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"position_sizing": {"base_size_sol": 5.0}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Max size < base size
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"position_sizing": {"max_size_sol": 0.01}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Min size > base size
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"position_sizing": {"min_size_sol": 5.0}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Consensus multiplier out of range
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"position_sizing": {"consensus_multiplier": 9.0}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Daily summary hour > 23
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"notifications": {"daily_summary": {"hour_utc": 24}}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Daily summary minute > 59
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"notifications": {"daily_summary": {"minute": 60}}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Load shed threshold > 100
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"queue": {"load_shed_threshold_percent": 101}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn update_config_rollback_on_validation_failure() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Admin);

    // First apply a valid change.
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({"circuit_breakers": {"max_consecutive_losses": 4}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Now send a partial update that mutates then fails validation: queue
    // capacity valid but load shed invalid happens in the same section, so use
    // strategy allocation (mutated first, then fails) — allocation sum check
    // happens inside the same block. Simpler: monitoring rate limit 60 fails
    // before mutation of prior fields? No: circuit_breakers applied first,
    // then monitoring fails → snapshot must restore.
    let resp = api_put(
        &h.app,
        "/api/v1/config",
        headers.clone(),
        json!({
            "circuit_breakers": {"max_consecutive_losses": 9},
            "monitoring": {"webhook_processing_rate_limit": 60}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let config = h.config.read().await;
    assert_eq!(
        config.circuit_breakers.max_consecutive_losses, 4,
        "failed update must roll back the earlier mutation"
    );
}

// =============================================================================
// CIRCUIT BREAKER
// =============================================================================

#[tokio::test]
async fn reset_circuit_breaker_success_and_forbidden() {
    let h = build(test_config()).await;

    // Non-admin → 403
    let resp = api_post(
        &h.app,
        "/api/v1/config/circuit-breaker/reset",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Trip first so reset observes a state change.
    let resp = api_post(
        &h.app,
        "/api/v1/config/circuit-breaker/trip",
        auth_headers(Role::Admin),
        json!({"reason": "emergency"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["new_state"], "TRIPPED");
    assert!(body["message"].as_str().unwrap().contains("emergency"));

    let resp = api_post(
        &h.app,
        "/api/v1/config/circuit-breaker/reset",
        auth_headers(Role::Admin),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["previous_state"], "TRIPPED");
    assert_eq!(body["new_state"], "ACTIVE");

    // Kill switch persisted as INACTIVE after reset
    let (state,): (String,) = sqlx::query_as("SELECT state FROM kill_switch_state WHERE id = 1")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(state, "INACTIVE");
}

#[tokio::test]
async fn trip_circuit_breaker_default_reason() {
    let h = build(test_config()).await;

    let resp = api_post(
        &h.app,
        "/api/v1/config/circuit-breaker/trip",
        auth_headers(Role::Admin),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Emergency kill switch activated"));

    // kill_switch_state row ACTIVE + audit entry
    let (state,): (String,) = sqlx::query_as("SELECT state FROM kill_switch_state WHERE id = 1")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(state, "ACTIVE");
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM config_audit WHERE key = 'kill_switch'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);

    // CB state mutated in memory: extreme config values applied
    let config = h.config.read().await;
    assert_eq!(config.circuit_breakers.max_consecutive_losses, 1);
    drop(config);
}

// =============================================================================
// TRADES
// =============================================================================

#[tokio::test]
async fn list_trades_filters_and_pagination_clamp() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(70.0)).await;
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.5"),
    )
    .await;
    seed_trade(
        &h.pool, "t2", WALLET_B, TOKEN_A, "BUY", "ACTIVE", "SPEAR", "2.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t3", WALLET_A, TOKEN_B, "SELL", "FAILED", "EXIT", "0.5", None,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/trades", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 3);
    assert_eq!(body["limit"], 100);

    // status + strategy + wallet filters
    let resp = api_get(
        &h.app,
        "/api/v1/trades?status=CLOSED&strategy=SHIELD&wallet_address=WALLET_A",
        Default::default(),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["total"], 0, "wallet_address is a literal filter value");

    let resp = api_get(
        &h.app,
        &format!(
            "/api/v1/trades?status=CLOSED&strategy=SHIELD&wallet_address={}",
            WALLET_A
        ),
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["trades"][0]["trade_uuid"], "t1");

    // Negative limit/offset clamp: limit -1 → 1, offset -5 → 0
    let resp = api_get(
        &h.app,
        "/api/v1/trades?limit=-1&offset=-5",
        Default::default(),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["limit"], 1);
    assert_eq!(body["offset"], 0);

    // Oversized limit clamps to 1000
    let resp = api_get(&h.app, "/api/v1/trades?limit=99999", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["limit"], 1000);
}

#[tokio::test]
async fn export_trades_csv_json_pdf() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.5"),
    )
    .await;

    // CSV (default)
    let resp = api_get(&h.app, "/api/v1/trades/export", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("content-type").unwrap(), "text/csv");
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("text/csv"));
    assert!(resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("chimera_trades_all_now.csv"));

    // JSON
    let resp = api_get(
        &h.app,
        "/api/v1/trades/export?format=JSON",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("application/json"));

    // PDF export is not implemented in this printpdf build → explicit 500
    let resp = api_get(
        &h.app,
        "/api/v1/trades/export?format=pdf",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // Filename sanitization runs on every export (dates pass through the
    // sanitizer); use a valid ISO date so the DB filter accepts it.
    let uri = "/api/v1/trades/export?from=2024-01-01T00:00:00Z&to=2024-06-30T23:59:59Z";
    let resp = api_get(&h.app, uri, Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let disposition = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(disposition.contains("2024-01-01T00:00:00Z_2024-06-30T23:59:59Z"));
}

// =============================================================================
// METRICS
// =============================================================================

#[tokio::test]
async fn performance_metrics_empty() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/metrics/performance", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["pnl_24h"], "0");
    assert!(body["pnl_24h_change_percent"].is_null());
}

#[tokio::test]
async fn performance_metrics_with_pnl() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    // Current-window (24h) realized PnL via CLOSED positions.
    seed_closed_position_with_pnl(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "0.5",
        chrono::Utc::now() - chrono::Duration::hours(1),
    )
    .await;
    seed_closed_position_with_pnl(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "-0.2",
        chrono::Utc::now() - chrono::Duration::hours(2),
    )
    .await;
    // Prior-window (48h..24h) realized PnL.
    seed_closed_position_with_pnl(
        &h.pool,
        "t3",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "0.25",
        chrono::Utc::now() - chrono::Duration::hours(36),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/metrics/performance", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_dec_eq(&body, "pnl_24h", "0.3");
    assert!(body["pnl_24h_change_percent"].is_number());
}

#[tokio::test]
async fn cost_metrics_empty_and_with_trades() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/metrics/costs", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_dec_eq(&body, "avg_jito_tip_sol", "0");
    assert_dec_eq(&body, "roi_percent", "0");

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, jito_tip_sol, dex_fee_sol, slippage_cost_sol, total_cost_sol, pnl_sol, created_at) \
         VALUES ('t1', $1, $2, 'SHIELD', 'BUY', 1.0, 'CLOSED', 0.001, 0.0005, 0.0002, 0.0017, 0.05, NOW() - INTERVAL '1 day')",
    )
    .bind(WALLET_A)
    .bind(TOKEN_A)
    .execute(&h.pool)
    .await
    .unwrap();
    // get_pnl_30d reads CLOSED positions, not trades.
    seed_closed_position_with_pnl(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "0.05",
        chrono::Utc::now() - chrono::Duration::hours(1),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/metrics/costs", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_dec_eq(&body, "total_costs_30d_sol", "0.0017");
    assert_dec_eq(&body, "avg_jito_tip_sol", "0.001");
    // net = 0.05 - 0.0017
    assert_dec_eq(&body, "net_profit_30d_sol", "0.0483");
}

#[tokio::test]
async fn strategy_performance_missing_param_and_data() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/metrics/strategy", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SELL",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.4"),
    )
    .await;
    seed_trade(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "SELL",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("-0.1"),
    )
    .await;
    // total_pnl sums pnl_usd — mirror pnl_sol into pnl_usd.
    sqlx::query("UPDATE trades SET pnl_usd = pnl_sol * 100")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/metrics/strategy?strategy=SHIELD&days=7",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["strategy"], "SHIELD");
    assert_dec_eq(&body, "total_pnl", "30");
    assert_eq!(body["trade_count"], 2);

    // Default days when param invalid
    let resp = api_get(
        &h.app,
        "/api/v1/metrics/strategy?strategy=SHIELD&days=abc",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn trade_latency_ranges() {
    let h = build(test_config()).await;
    for range in ["24h", "7d", "30d", "bogus"] {
        let resp = api_get(
            &h.app,
            &format!("/api/v1/metrics/trade-latency?range={range}"),
            Default::default(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert!(body["histogram"].is_array());
        assert_eq!(body["time_range"], range);
        assert_eq!(body["sample_size"], 0);
    }
}

#[tokio::test]
async fn database_performance() {
    let h = build(test_config()).await;
    let resp = api_get(
        &h.app,
        "/api/v1/metrics/database-performance",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["connection_pool"]["max_connections"].is_number());
    assert!(body["cache_performance"]["hit_rate"].is_number());
}

#[tokio::test]
async fn rpc_latency_with_and_without_samples() {
    let h = build(test_config()).await;

    // NOTE: the RPC metrics are process-global and shared with other tests, so
    // an "empty state" assertion is not possible; assert the populated paths.

    // Populate the process-global RPC metrics
    chimera_operator::metrics::rpc_latency_metric()
        .with_label_values(&["primary", "getLatestBlockhash"])
        .observe(12.5);
    chimera_operator::metrics::rpc_latency_metric()
        .with_label_values(&["primary", "getLatestBlockhash"])
        .observe(17.5);
    chimera_operator::metrics::rpc_errors_metric()
        .with_label_values(&["primary", "getLatestBlockhash"])
        .inc();
    // A second child with samples exercises the merged-bucket branch; a third
    // child created WITHOUT samples exercises the zero-sample skip branch.
    chimera_operator::metrics::rpc_latency_metric()
        .with_label_values(&["polling", "getLatestBlockhash"])
        .observe(25.0);
    let _ =
        chimera_operator::metrics::rpc_latency_metric().with_label_values(&["jito", "getBalance"]);

    let resp = api_get(&h.app, "/api/v1/metrics/rpc-latency", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let endpoints = body["endpoints"].as_array().unwrap();
    // Process-global metrics may include children recorded by other tests,
    // so assert the specific primary/polling rows instead of exact counts.
    let primary = endpoints
        .iter()
        .find(|e| e["endpoint"] == "primary")
        .expect("primary endpoint");
    assert_eq!(primary["request_count"], 2);
    assert_eq!(primary["error_rate_percent"], 50.0);
    let polling = endpoints
        .iter()
        .find(|e| e["endpoint"] == "polling")
        .expect("polling endpoint");
    assert_eq!(polling["request_count"], 1);
    assert!(body["sample_size"].as_u64().unwrap() >= 3);
    assert!(body["overall_avg_ms"].as_f64().unwrap() > 10.0);
}

#[tokio::test]
async fn request_rate_with_limiter() {
    let h = build(test_config()).await;

    // Exhaust the limiter so current rate is high → warning status paths.
    for _ in 0..60 {
        h.monitoring_state.webhook_rate_limiter.try_acquire();
        let _ = h
            .api_state
            .webhook_rate_limiter
            .as_ref()
            .unwrap()
            .try_acquire();
    }

    let resp = api_get(&h.app, "/api/v1/metrics/request-rate", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["current_rps"].as_f64().unwrap() >= 0.0);
    assert_eq!(body["rate_limits"].as_array().unwrap().len(), 2);
    assert!(body["overall_status"].is_string());
}

#[tokio::test]
async fn request_rate_zero_limits() {
    let mut config = test_config();
    config
        .monitoring
        .as_mut()
        .unwrap()
        .webhook_processing_rate_limit = 0;
    config.rpc.rate_limit_per_second = 0;
    let h = build(config).await;

    let limiter = Arc::new(RateLimiter::new(1, 1));
    let state = alt_api_state_with_limiter(&h, None, Some(limiter.clone()));
    let app = Router::new()
        .route(
            "/request-rate",
            get(chimera_operator::handlers::get_request_rate),
        )
        .with_state(state);
    assert!(limiter.try_acquire());

    let resp = api_get(&app, "/request-rate", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // webhook_rate 1 > 0 → warning; rpc_rate 0.6 > 0 → warning; overall:
    // rate > limit (0) → throttled; utilization falls back to 0.0 for both
    // zero limits.
    assert_eq!(body["rate_limits"][0]["status"], "warning");
    assert_eq!(body["rate_limits"][1]["status"], "warning");
    assert_eq!(body["overall_status"], "throttled");
    assert_eq!(body["rate_limits"][0]["utilization_percent"], 0.0);
    assert_eq!(body["rate_limits"][1]["utilization_percent"], 0.0);
}

#[tokio::test]
async fn request_rate_without_limiter() {
    let h = build(test_config()).await;
    let state = alt_api_state(&h, None);
    let app = Router::new()
        .route(
            "/request-rate",
            get(chimera_operator::handlers::get_request_rate),
        )
        .with_state(state);
    let resp = api_get(&app, "/request-rate", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["current_rps"], 0.0);
    assert_eq!(body["overall_status"], "healthy");
}

#[tokio::test]
async fn rpc_status_fallback_without_engine() {
    let h = build(test_config()).await;
    let state = alt_api_state(&h, None);
    let app = Router::new()
        .route("/config", get(chimera_operator::handlers::get_config))
        .with_state(state);
    let resp = api_get(&app, "/config", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["rpc_status"]["active"], "jito"); // jito enabled by default
    assert_eq!(body["rpc_status"]["fallback_triggered"], false);
}

// =============================================================================
// INCIDENTS (DEAD LETTER + CONFIG AUDIT)
// =============================================================================

#[tokio::test]
async fn dead_letter_queue_list_and_pagination() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/incidents/dead-letter", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 0);

    seed_dead_letter(&h.pool, "dl-1", true, 0).await;
    seed_dead_letter(&h.pool, "dl-2", false, 2).await;

    let resp = api_get(
        &h.app,
        "/api/v1/incidents/dead-letter?limit=1&offset=0",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["total"], 2);

    // Oversized limit clamps to 200
    let resp = api_get(
        &h.app,
        "/api/v1/incidents/dead-letter?limit=9999",
        Default::default(),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn retry_dead_letter_item_paths() {
    let h = build(test_config()).await;

    // Forbidden for readonly (needs an existing DLQ row to reach the auth gate)
    seed_dead_letter(&h.pool, "dl-f", true, 0).await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-f/retry",
        auth_headers(Role::Readonly),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Not found
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/nope/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Non-retryable
    seed_dead_letter(&h.pool, "dl-2", false, 0).await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-2/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Retry limit reached
    seed_dead_letter(&h.pool, "dl-3", true, 3).await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-3/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Circular-import note: the success path now really re-queues, which needs a
    // trade row in DEAD_LETTER and a deserializable SignalPayload. A row with an
    // empty '{}' payload is malformed -> BadRequest.
    seed_trade(
        &h.pool,
        "dl-mal",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "DEAD_LETTER",
        "SHIELD",
        "0.25",
        None,
    )
    .await;
    seed_dead_letter(&h.pool, "dl-mal", true, 0).await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-mal/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // A DEAD_LETTER trade with no matching trade row (only a DLQ row) cannot be
    // re-queued -> the conditional DEAD_LETTER->QUEUED guard matches nothing.
    seed_dead_letter(&h.pool, "dl-norow", true, 0).await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-norow/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Success: a real DEAD_LETTER trade + valid SignalPayload is re-queued,
    // moving the trade to QUEUED and incrementing the DLQ retry count.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool,
        "dl-1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "DEAD_LETTER",
        "SHIELD",
        "0.25",
        None,
    )
    .await;
    seed_dead_letter_with_payload(
        &h.pool, "dl-1", WALLET_A, TOKEN_A, "BUY", "SHIELD", "0.25", true, 0,
    )
    .await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-1/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["retry_attempt"], 1);

    // The trade moved DEAD_LETTER -> QUEUED (re-queued into the engine).
    let status: String = sqlx::query_scalar("SELECT status FROM trades WHERE trade_uuid = 'dl-1'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(status, "QUEUED");

    // Retrying the same DLQ item now that it's QUEUED (not DEAD_LETTER) is refused.
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-1/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Circuit breaker tripped → 503
    h.api_state
        .circuit_breaker
        .manual_trip("test", "trip".to_string())
        .await
        .unwrap();
    seed_trade(
        &h.pool,
        "dl-4",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "DEAD_LETTER",
        "SHIELD",
        "0.25",
        None,
    )
    .await;
    seed_dead_letter_with_payload(
        &h.pool, "dl-4", WALLET_A, TOKEN_A, "BUY", "SHIELD", "0.25", true, 0,
    )
    .await;
    let resp = api_post(
        &h.app,
        "/api/v1/incidents/dead-letter/dl-4/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn config_audit_list() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/incidents/config-audit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total"], 0);

    seed_config_audit(&h.pool, "queue.capacity", Some("100"), "200", "admin").await;

    let resp = api_get(
        &h.app,
        "/api/v1/incidents/config-audit?limit=1",
        Default::default(),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["key"], "queue.capacity");
}

// =============================================================================
// METRICS UPDATES (reconciliation / secret rotation)
// =============================================================================

#[tokio::test]
async fn update_reconciliation_metrics_all_fields() {
    let h = build(test_config()).await;

    // Forbidden for readonly
    let resp = api_post(
        &h.app,
        "/api/v1/metrics/reconciliation",
        auth_headers(Role::Readonly),
        json!({"checked": 10}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/metrics/reconciliation",
        auth_headers(Role::Operator),
        json!({"checked": 10, "discrepancies": 2, "unresolved": 3}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "updated");

    // Negative deltas ignored (warn path)
    let resp = api_post(
        &h.app,
        "/api/v1/metrics/reconciliation",
        auth_headers(Role::Operator),
        json!({"checked": -1, "discrepancies": -1, "unresolved": 4}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Zero deltas: no inc, but unresolved gauge still set
    let resp = api_post(
        &h.app,
        "/api/v1/metrics/reconciliation",
        auth_headers(Role::Operator),
        json!({"checked": 0, "discrepancies": 0}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM config_audit WHERE changed_by = 'SYSTEM_METRICS'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(count >= 5);
}

#[tokio::test]
async fn update_secret_rotation_metrics() {
    let h = build(test_config()).await;

    let resp = api_post(
        &h.app,
        "/api/v1/metrics/secret-rotation",
        auth_headers(Role::Readonly),
        json!({"last_success_timestamp": 1}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/metrics/secret-rotation",
        auth_headers(Role::Operator),
        json!({"last_success_timestamp": 1700000000, "days_until_due": 30}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = api_post(
        &h.app,
        "/api/v1/metrics/secret-rotation",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// =============================================================================
// require_role_from_request (pure helper)
// =============================================================================

#[test]
fn require_role_missing_extension_and_roles() {
    use chimera_operator::error::AppError;

    let mut extensions = axum::http::Extensions::new();
    match require_role_from_request(&extensions, Role::Readonly) {
        Err(AppError::Auth(_)) => {}
        other => panic!("expected Auth error, got {:?}", other.map(|_| ())),
    }

    extensions.insert(AuthExtension(AuthenticatedUser {
        identifier: "user".to_string(),
        role: Role::Readonly,
    }));
    match require_role_from_request(&extensions, Role::Admin) {
        Err(AppError::Forbidden(_)) => {}
        other => panic!("expected Forbidden error, got {:?}", other.map(|_| ())),
    }

    let auth = require_role_from_request(&extensions, Role::Readonly).unwrap();
    assert_eq!(auth.0.identifier, "user");
}

// =============================================================================
// RECONCILIATION
// =============================================================================

#[tokio::test]
async fn reconciliation_status_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(
        &h.app,
        "/api/v1/reconciliation/status",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "completed");

    seed_reconciliation_run(&h.pool, "t1", "ACTIVE", Some("state mismatch"), false).await;
    seed_reconciliation_run(&h.pool, "t2", "ACTIVE", None, true).await;

    let resp = api_get(
        &h.app,
        "/api/v1/reconciliation/status",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["discrepancy_count"], 1);
    assert_eq!(body["unresolved_count"], 1);
    assert_eq!(body["recent_discrepancies"].as_array().unwrap().len(), 1);
    assert_eq!(body["recent_discrepancies"][0]["type"], "state mismatch");
}

#[tokio::test]
async fn reconciliation_history_and_stats() {
    let h = build(test_config()).await;
    seed_reconciliation_run(&h.pool, "t1", "ACTIVE", None, true).await;
    seed_reconciliation_run(&h.pool, "t2", "ACTIVE", Some("x"), true).await;
    seed_reconciliation_run(&h.pool, "t3", "ACTIVE", Some("y"), false).await;

    let resp = api_get(
        &h.app,
        "/api/v1/reconciliation/history",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // Runs are grouped by calendar day; all seeds share the same date.
    assert_eq!(body["runs"].as_array().unwrap().len(), 1);
    assert_eq!(body["total_runs"], 1);
    assert_eq!(body["runs"][0]["checked_count"], 3);

    // Empty history: success_rate defaults to 100
    let h2 = build(test_config()).await;
    let resp = api_get(
        &h2.app,
        "/api/v1/reconciliation/history",
        auth_headers(Role::Readonly),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["success_rate"], 100.0);

    let resp = api_get(
        &h.app,
        "/api/v1/reconciliation/stats?range=7d",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["total_reconciliations"].is_number());
    assert!(body["most_common_discrepancy_types"].is_array());
}

#[tokio::test]
async fn trigger_reconciliation_success() {
    let h = build(test_config()).await;

    let resp = api_post(
        &h.app,
        "/api/v1/reconciliation/trigger",
        auth_headers(Role::Readonly),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/reconciliation/trigger",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["run_id"].is_string());
    assert!(body["scheduled_at"].is_string());

    // Audit entry logged
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM config_audit WHERE key = 'reconciliation.manual_trigger'",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    // Let the spawned reconciliation run finish its result logging.
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
}

#[tokio::test]
async fn resolve_discrepancy_success_and_forbidden() {
    let h = build(test_config()).await;
    seed_reconciliation_run(&h.pool, "t1", "ACTIVE", Some("state mismatch"), false).await;

    let resp = api_post(
        &h.app,
        "/api/v1/reconciliation/discrepancies/1/resolve",
        auth_headers(Role::Readonly),
        json!({"resolution": "confirmed"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/reconciliation/discrepancies/1/resolve",
        auth_headers(Role::Operator),
        json!({"resolution": "confirmed"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);

    let (resolved_at,): (Option<chrono::DateTime<chrono::Utc>>,) =
        sqlx::query_as("SELECT resolved_at FROM reconciliation_log WHERE id = 1")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(resolved_at.is_some());
}

// =============================================================================
// DEBUG + ADMIN
// =============================================================================

#[tokio::test]
async fn clear_monitoring_caches_not_implemented() {
    let h = build(test_config()).await;

    let state = alt_api_state(&h, None);
    let app = Router::new()
        .route(
            "/clear-caches",
            post(chimera_operator::handlers::clear_monitoring_caches),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(chimera_operator::middleware::AuthState::with_auth_config(
                HashMap::from([
                    ("test-operator".to_string(), Role::Operator),
                    ("test-admin".to_string(), Role::Admin),
                ]),
                "test".to_string(),
            )),
            chimera_operator::middleware::bearer_auth,
        ));

    // Non-admin → 403
    let resp = api_post(
        &app,
        "/clear-caches",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Admin → 503 (explicit not-implemented)
    let resp = api_post(&app, "/clear-caches", auth_headers(Role::Admin), json!({})).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn debug_backtest_smoke_paths() {
    let h = build(test_config()).await;

    // Empty wallet address → 400
    let resp = api_post(
        &h.app,
        "/api/v1/debug/backtest-smoke",
        auth_headers(Role::Operator),
        json!({"wallet_address": "   "}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Unknown wallet: no trades
    let resp = api_post(
        &h.app,
        "/api/v1/debug/backtest-smoke",
        auth_headers(Role::Operator),
        json!({"wallet_address": WALLET_A}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_trades"], 0);
    assert_eq!(body["passed"], false);
    assert!(body["notes"]
        .as_str()
        .unwrap()
        .contains("no CLOSED trades yet"));

    // CLOSED trades without pnl_sol
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "CLOSED", "SHIELD", "1.0", None,
    )
    .await;
    let resp = api_post(
        &h.app,
        "/api/v1/debug/backtest-smoke",
        auth_headers(Role::Operator),
        json!({"wallet_address": WALLET_A}),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["closed_trades"], 1);
    assert_eq!(body["passed"], false);
    assert!(body["notes"].as_str().unwrap().contains("FAIL"));

    // With pnl populated → PASS
    sqlx::query("UPDATE trades SET pnl_sol = 0.1 WHERE trade_uuid = 't1'")
        .execute(&h.pool)
        .await
        .unwrap();
    let resp = api_post(
        &h.app,
        "/api/v1/debug/backtest-smoke",
        auth_headers(Role::Operator),
        json!({"wallet_address": WALLET_A}),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["passed"], true);
    assert!(body["notes"].as_str().unwrap().contains("PASS"));
}

// =============================================================================
// HELPERS
// =============================================================================

use std::collections::HashMap;

/// Build an ApiState variant with optional engine/limiter for fallback-path
/// tests (all other fields cloned from the harness state).
fn alt_api_state(
    h: &Harness,
    engine: Option<Arc<chimera_operator::engine::EngineHandle>>,
) -> Arc<ApiState> {
    alt_api_state_with_limiter(h, engine, None)
}

fn alt_api_state_with_limiter(
    h: &Harness,
    engine: Option<Arc<chimera_operator::engine::EngineHandle>>,
    limiter: Option<Arc<RateLimiter>>,
) -> Arc<ApiState> {
    Arc::new(ApiState {
        db: h.db.clone(),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        config: h.config.clone(),
        notifier: h.api_state.notifier.clone(),
        engine,
        metrics: h.api_state.metrics.clone(),
        signal_aggregator: None,
        market_regime_detector: None,
        helius_client: h.api_state.helius_client.clone(),
        webhook_rate_limiter: limiter,
        price_cache: h.api_state.price_cache.clone(),
        toxic_detector: None,
        run_context: None,
        decision_recorder: None,
        profitability_verdict: h.api_state.profitability_verdict.clone(),
    })
}

/// Poll `f` until it returns true or `timeout_ms` elapses (for assertions on
/// tokio::spawned handler side effects).
async fn wait_for<F, Fut>(mut f: F, timeout_ms: u64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if f().await {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("condition not met within {timeout_ms}ms");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn seed_wallet_monitoring_row(
    h: &Harness,
    wallet: &str,
    webhook_id: Option<&str>,
    enabled: bool,
) {
    harness::seed_wallet_monitoring(
        &h.pool,
        wallet,
        webhook_id,
        enabled,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

#[tokio::test]
async fn trip_circuit_breaker_forbidden_for_non_admin() {
    let h = build(test_config()).await;
    let resp = api_post(
        &h.app,
        "/api/v1/config/circuit-breaker/trip",
        auth_headers(Role::Operator),
        json!({"reason": "nope"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_config_monitoring_section_when_config_has_none() {
    // When config.monitoring is None, the monitoring update block creates a
    // default MonitoringConfig; the final validate() then fails because the
    // default has no helius_api_key → 400 (and the snapshot is restored).
    let mut config = test_config();
    config.monitoring = None;
    let h = build(config).await;

    let resp = api_put(
        &h.app,
        "/api/v1/config",
        auth_headers(Role::Admin),
        json!({"monitoring": {"enabled": true, "max_active_wallets": 12}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Rollback: monitoring must still be None.
    let config = h.config.read().await;
    assert!(config.monitoring.is_none());
}

#[tokio::test]
async fn request_rate_warning_and_throttled() {
    let mut config = test_config();
    config
        .monitoring
        .as_mut()
        .unwrap()
        .webhook_processing_rate_limit = 1;
    config.rpc.rate_limit_per_second = 1;
    let h = build(config).await;

    // Custom ApiState: limit 1 → 1 successful acquire gives rps 1 = the limit.
    let limiter = Arc::new(RateLimiter::new(1, 1));
    let state = alt_api_state_with_limiter(&h, None, Some(limiter.clone()));
    let app = Router::new()
        .route(
            "/request-rate",
            get(chimera_operator::handlers::get_request_rate),
        )
        .with_state(state);

    // Acquire the single credit → rps 1 → rate == limit > 0.9*limit → warning.
    assert!(limiter.try_acquire());

    let resp = api_get(&app, "/request-rate", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let webhook_status = body["rate_limits"][0]["status"]
        .as_str()
        .unwrap()
        .to_string();
    let rpc_status = body["rate_limits"][1]["status"]
        .as_str()
        .unwrap()
        .to_string();
    // webhook_rate 1 > 0.9*1 → warning; rpc_rate 0.6 <= 0.9*1 → ok;
    // overall: rate > 0.9*limit → warning.
    assert_eq!(webhook_status, "warning");
    assert_eq!(rpc_status, "ok");
    assert_eq!(body["overall_status"], "warning");
    assert!(body["current_rps"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn rpc_status_helius_when_no_engine_and_jito_disabled() {
    let h = build(test_config()).await;
    // Disable jito in the shared config for this test.
    {
        let mut config = h.config.write().await;
        config.jito.enabled = false;
    }
    let state = alt_api_state(&h, None);
    let app = Router::new()
        .route("/config", get(chimera_operator::handlers::get_config))
        .with_state(state);
    let resp = api_get(&app, "/config", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["rpc_status"]["active"], "helius");
}

#[tokio::test]
async fn trigger_reconciliation_without_engine() {
    let h = build(test_config()).await;
    let state = alt_api_state(&h, None);
    let app = Router::new()
        .route(
            "/trigger",
            post(chimera_operator::handlers::trigger_reconciliation),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(chimera_operator::middleware::AuthState::with_auth_config(
                HashMap::from([
                    ("test-operator".to_string(), Role::Operator),
                    ("test-admin".to_string(), Role::Admin),
                ]),
                "test".to_string(),
            )),
            chimera_operator::middleware::bearer_auth,
        ));

    let resp = api_post(&app, "/trigger", auth_headers(Role::Operator), json!({})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["run_id"].is_string());
}
