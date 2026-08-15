//! HTTP handler tests for `operator/src/handlers/monitoring.rs`.
//!
//! The Helius webhook handler is exercised end-to-end: payloads are delivered
//! to the real route and the handler's auth / rate-limit / dedup / RPC-verify /
//! wallet-resolution / selection paths run against the real DB. Webhook
//! registration calls (register/delete/toggle) hit the local Helius mock.
//!
//! NOTE: `verify_signature_exists` reads `HELIUS_RPC_BASE_URL` at call time;
//! it is intentionally NOT redirected to the mock (the pre-existing
//! `helius_rpc_verify_tests` pin its default). With the test API key the real
//! host returns HTTP 401 → the handler's Err paths are exercised
//! deterministically; the Ok(true)/Ok(false) branches are network-gated.

use axum::http::StatusCode;
use axum::Router;
use chimera_operator::handlers::helius_webhook_handler;
use serde_json::json;
use std::sync::Arc;

#[path = "../common/harness.rs"]
mod harness;

use chimera_operator::middleware::Role;
use harness::{
    api_get, api_post, auth_headers, build, json_body, seed_position, seed_trade, seed_wallet,
    seed_wallet_monitoring, test_config, TOKEN_A, WALLET_A, WALLET_B,
};

fn webhook_payload(signature: &str, wallet: &str, direction: &str) -> serde_json::Value {
    let (token_inputs, native_output) = if direction == "SELL" {
        (
            json!([{"userAccount": wallet, "mint": TOKEN_A, "rawTokenAmount": {"tokenAmount": "1000000", "decimals": 6}}]),
            json!({"account": "sol", "amount": "100000000"}),
        )
    } else {
        (json!([]), json!({"account": "sol", "amount": "100000000"}))
    };
    let token_outputs = if direction == "BUY" {
        json!([{"userAccount": wallet, "mint": TOKEN_A, "rawTokenAmount": {"tokenAmount": "1000000", "decimals": 6}}])
    } else {
        json!([])
    };
    json!([{
        "accountData": [
            {
                "account": wallet,
                "nativeBalanceChange": 0,
                "tokenBalanceChanges": [
                    {
                        "mint": TOKEN_A,
                        "rawTokenAmount": {"tokenAmount": "1000000", "decimals": 6},
                        "tokenAccount": "tok-acc",
                        "userAccount": wallet
                    }
                ]
            }
        ],
        "nativeTransfers": [],
        "signature": signature,
        "slot": 1,
        "timestamp": 1,
        "type": "SWAP",
        "events": {
            "swap": {
                "swapper": wallet,
                "nativeInput": null,
                "nativeOutput": native_output,
                "tokenInputs": token_inputs,
                "tokenOutputs": token_outputs
            }
        }
    }])
}

const WEBHOOK_URL: &str = "/api/v1/monitoring/helius-webhook";

#[tokio::test]
async fn helius_webhook_auth_enforce_and_mismatch() {
    // Config with auth header in enforce mode.
    let mut config = test_config();
    let monitoring = config.monitoring.as_mut().unwrap();
    monitoring.helius_webhook_auth_header = Some("secret-token".to_string());
    monitoring.helius_auth_enforce = true;
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Wrong header → 401
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-auth-bad", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct header → accepted
    let mut headers: axum::http::HeaderMap = Default::default();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        "secret-token".parse().unwrap(),
    );
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        headers,
        webhook_payload("sig-auth-good", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_auth_dry_run_accepts_mismatch() {
    let mut config = test_config();
    let monitoring = config.monitoring.as_mut().unwrap();
    monitoring.helius_webhook_auth_header = Some("secret-token".to_string());
    monitoring.helius_auth_enforce = false; // dry-run
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-dryrun", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_rate_limit_and_dedup() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Exhaust the rate limiter → events skipped, still 200.
    for _ in 0..80 {
        let _ = h.monitoring_state.webhook_rate_limiter.try_acquire();
    }
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-ratelimited", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Dedup: same signature within 5 minutes → skipped.
    let h2 = build(test_config()).await;
    seed_wallet(&h2.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    let payload = webhook_payload("sig-dup", WALLET_A, "SELL");
    let resp = api_post(&h2.app, WEBHOOK_URL, Default::default(), payload.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = api_post(&h2.app, WEBHOOK_URL, Default::default(), payload).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_rpc_verify_enforce_drops_unverifiable() {
    let mut config = test_config();
    config.monitoring.as_mut().unwrap().rpc_verify_enforce = true;
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // RPC returns 401 for the test key → Err → enforce drops the event.
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-enforce-drop", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Fail-open regression (2026-08-15): an RPC *error* during signature
/// re-verification must NOT drop the event. The daily quota exhausted by
/// ~07:00 UTC made every verify Err, and enforce mode then discarded every
/// arriving webhook for 17h/day. The event is already HMAC + auth-header
/// authenticated; RPC unavailability is not evidence of forgery. A SELL with
/// an active position must still be queued.
#[tokio::test]
async fn helius_webhook_rpc_verify_error_fails_open_and_processes() {
    let mut config = test_config();
    let mon = config.monitoring.as_mut().unwrap();
    mon.rpc_verify_enforce = true;
    // Force verification of every event so the RPC error path is exercised
    // (the default 5% sampling may skip this signature).
    mon.rpc_verify_sample_rate = 1.0;
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_trade(
        &h.pool, "t-fopos", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t-fopos", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.0", "0.01", None,
    )
    .await;

    // RPC returns 401 for the test key → Err → fail-open accepts.
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-failopen-processed", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = $1 AND trade_uuid != 't-fopos'",
    )
    .bind(WALLET_A)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "RPC error must fail open: the HMAC-authenticated event still processes"
    );
}

#[tokio::test]
async fn helius_webhook_sell_admitted_and_queued() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    // SELL admission requires an ACTIVE position for the token.
    seed_trade(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.0", "0.01", None,
    )
    .await;

    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-sell-admitted", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The admitted SELL must be queued into the trades table. The engine
    // worker spawned at Engine construction may already have advanced the
    // status, so assert on the monitoring-inserted row by excluding the
    // position seed trade.
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = $1 AND trade_uuid != 't-pos'",
    )
    .bind(WALLET_A)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "admitted SELL must insert a trade row");
}

#[tokio::test]
async fn helius_webhook_buy_rejected_low_wqs() {
    let h = build(test_config()).await;
    // Low WQS → BUY rejected by the hard WQS gate.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(30.0)).await;

    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-buy-rejected", WALLET_A, "BUY"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_auto_adds_unknown_wallet() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Warm the 30s active-wallet cache, then remove the row from the DB: the
    // next event still matches via the cache but get_wallet returns None →
    // the auto-add branch runs (this is the only way to reach it).
    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-cache-warm", WALLET_A, "BUY"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    sqlx::query("DELETE FROM wallets WHERE address = $1")
        .bind(WALLET_A)
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-new-wallet", WALLET_A, "BUY"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets WHERE address = $1")
        .bind(WALLET_A)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "wallet must be auto-added");
}

#[tokio::test]
async fn helius_webhook_no_tracked_wallet_and_non_swap() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Payload whose userAccount matches no ACTIVE wallet → skipped.
    let mut payload = webhook_payload("sig-untracked", WALLET_B, "SELL");
    // WALLET_B is not in the DB at all.
    let resp = api_post(&h.app, WEBHOOK_URL, Default::default(), payload.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Non-SWAP transaction type → no swap parsed.
    payload[0]["type"] = json!("TRANSFER");
    let resp = api_post(&h.app, WEBHOOK_URL, Default::default(), payload).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn monitoring_status_authorized() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("wh-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/status",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["enabled"], true);
    assert_eq!(body["active_wallets"], 1);
    assert!(body["webhook_rate"].is_number());
    assert!(body["webhook_credits"].is_number());
}

#[tokio::test]
async fn enable_wallet_monitoring_paths() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet(&h.pool, WALLET_B, "CANDIDATE", Some(50.0)).await;

    // Forbidden for readonly
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Readonly),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Unknown wallet → 404
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/wallets/5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsZ/enable",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Non-ACTIVE wallet → 400
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_B),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Already enabled with webhook id → short-circuit 200
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("wh-existing"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Fresh wallet (monitoring row with no webhook id) → registers via mock
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        "ACTIVE",
        Some(70.0),
    )
    .await;
    seed_wallet_monitoring(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        None,
        false,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/wallets/5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX/enable",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Enabled state persisted with a mock webhook id
    let (id,): (Option<String>,) =
        sqlx::query_as("SELECT helius_webhook_id FROM wallet_monitoring WHERE wallet_address = $1")
            .bind("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(id.as_deref().unwrap_or("").starts_with("mock-webhook"));
}

#[tokio::test]
async fn enable_wallet_monitoring_missing_config() {
    let mut config = test_config();
    config.monitoring = None;
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // With monitoring config but no webhook URL → 500
    let mut config = test_config();
    config.monitoring.as_mut().unwrap().helius_webhook_url = None;
    let h = build(config).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn disable_wallet_monitoring_paths() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Forbidden for readonly
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/disable", WALLET_A),
        auth_headers(Role::Readonly),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // No monitoring row → 404
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/disable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Monitoring row with webhook id → mock delete → 200
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("mock-webhook-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/disable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (enabled, id): (bool, Option<String>) = sqlx::query_as(
        "SELECT monitoring_enabled, helius_webhook_id FROM wallet_monitoring WHERE wallet_address = $1",
    )
    .bind(WALLET_A)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert!(!enabled);
    assert!(id.is_none());

    // Monitoring row WITHOUT webhook id → skips delete → 200
    let h2 = build(test_config()).await;
    seed_wallet(&h2.pool, WALLET_B, "ACTIVE", Some(85.0)).await;
    seed_wallet_monitoring(
        &h2.pool,
        WALLET_B,
        None,
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h2.app,
        &format!("/api/v1/monitoring/wallets/{}/disable", WALLET_B),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn wallet_monitoring_states_variants() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(85.0)).await;
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        "ACTIVE",
        Some(85.0),
    )
    .await;

    // Webhook active + healthy → method webhook, status active
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("wh-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    // Polling (no webhook id), disabled → method polling, status inactive
    seed_wallet_monitoring(
        &h.pool,
        WALLET_B,
        None,
        false,
        Some("unknown"),
        Some("active"),
        2,
        Some("boom"),
    )
    .await;
    // Error status: unhealthy health + failed webhook status + attempts
    seed_wallet_monitoring(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        Some("wh-3"),
        true,
        Some("unhealthy"),
        Some("failed"),
        4,
        None,
    )
    .await;

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/wallets/states",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let states = body["wallet_states"].as_array().unwrap();
    assert_eq!(states.len(), 3);

    let by_addr = |addr: &str| {
        states
            .iter()
            .find(|s| s["address"] == addr)
            .unwrap_or_else(|| panic!("missing state for {addr}"))
            .clone()
    };

    let a = by_addr(WALLET_A);
    assert_eq!(a["method"], "webhook");
    assert_eq!(a["status"], "active");
    assert_eq!(a["success_rate"], 100.0);
    assert!(a["next_fetch"].is_null());

    let b = by_addr(WALLET_B);
    assert_eq!(b["method"], "polling");
    assert_eq!(b["status"], "inactive");
    assert_eq!(b["failed_fetches"], 2);
    assert_eq!(b["success_rate"], 90.0); // penalty for last_registration_error
    assert!(b["next_fetch"].is_string());

    let x = by_addr("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX");
    assert_eq!(x["status"], "error");
    assert_eq!(x["failed_fetches"], 4);
    assert_eq!(x["success_rate"], 100.0); // no last_registration_error
    assert!(x["last_activity"].is_string());
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

/// Build a MonitoringState variant (all fields public) with optional
/// selection / entry-confirmation / circuit-breaker, mounted on a dedicated
/// router.
fn monitoring_app(
    h: &harness::Harness,
    selection: Option<Arc<chimera_operator::engine::SelectionService>>,
    entry_confirmation: Option<
        Arc<chimera_operator::engine::entry_confirmation::EntryConfirmationManager>,
    >,
    circuit_breaker: Option<Arc<chimera_operator::circuit_breaker::CircuitBreaker>>,
) -> Router {
    use chimera_operator::monitoring::{HeliusClient, MonitoringState};
    use std::collections::HashMap;

    let monitoring = MonitoringState {
        db: h.db.clone(),
        engine: h.engine_handle.clone(),
        config: h.config_arc.clone(),
        webhook_rate_limiter: h.webhook_rate_limiter.clone(),
        rpc_rate_limiter: h.rpc_rate_limiter.clone(),
        helius_client: h.helius_client.clone(),
        signal_aggregator: Arc::new(
            chimera_operator::monitoring::signal_aggregator::SignalAggregator::new(h.db.clone()),
        ),
        pre_validator: Arc::new(chimera_operator::monitoring::PreValidator::new(
            h.config_arc.clone(),
        )),
        exit_detector: Arc::new(
            chimera_operator::monitoring::ExitDetector::new().with_db(h.db.clone()),
        ),
        wallet_performance: Arc::new(
            chimera_operator::monitoring::WalletPerformanceTracker::new_with_config(
                h.db.clone(),
                h.config_arc.clone(),
            ),
        ),
        circuit_breaker,
        token_parser: Some(h.token_parser.clone()),
        portfolio_heat: None,
        processed_signatures: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        active_wallet_cache: Arc::new(parking_lot::RwLock::new(None)),
        selection,
        entry_confirmation,
        helius_auth_header: None,
        helius_auth_enforce: false,
        rpc_verify_enforce: false,
        rpc_verify_sample_rate: 0.05,
    };
    Router::new()
        .route(WEBHOOK_URL, axum::routing::post(helius_webhook_handler))
        .with_state(Arc::new(monitoring))
}

#[tokio::test]
async fn helius_webhook_blocked_by_circuit_breaker() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_trade(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.0", "0.01", None,
    )
    .await;
    h.api_state
        .circuit_breaker
        .manual_trip("test", "blocked".to_string())
        .await
        .unwrap();

    let app = monitoring_app(
        &h,
        Some(h.selection_service.clone()),
        None,
        Some(h.circuit_breaker.clone()),
    );
    let resp = api_post(
        &app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-cb-blocked", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid != 't-pos'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "CB-tripped event must be dropped");
}

#[tokio::test]
async fn helius_webhook_no_selection_service() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let app = monitoring_app(&h, None, None, None);
    let resp = api_post(
        &app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-no-selection", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_duplicate_bucket_dedup() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_trade(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.0", "0.01", None,
    )
    .await;

    let app = monitoring_app(
        &h,
        Some(h.selection_service.clone()),
        None,
        Some(h.circuit_breaker.clone()),
    );
    // Two distinct signatures, same 5-minute bucket → same monitoring UUID →
    // second insert hits the unique constraint → queue_monitoring_signal
    // returns false → handler continues without error.
    let resp = api_post(
        &app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-bucket-1", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = api_post(
        &app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-bucket-2", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid != 't-pos'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "duplicate bucket must not insert a second trade");
}

#[tokio::test]
async fn helius_webhook_parse_no_swap_and_error() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    // Pure SOL swap: only SOL legs → no speculative token → parse Ok(None)
    // with a tracked wallet → diagnostic counters + logs.
    let mut payload = webhook_payload("sig-no-swap", WALLET_A, "SELL");
    payload[0]["events"]["swap"]["tokenInputs"] = json!([]);
    payload[0]["events"]["swap"]["tokenOutputs"] = json!([]);
    payload[0]["accountData"][0]["tokenBalanceChanges"] = json!([
        {
            "mint": harness::SOL_MINT,
            "rawTokenAmount": {"tokenAmount": "100000000", "decimals": 9},
            "tokenAccount": "tok-acc",
            "userAccount": WALLET_A
        }
    ]);
    let resp = api_post(&h.app, WEBHOOK_URL, Default::default(), payload.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Malformed token amount → parse Err.
    payload[0]["signature"] = json!("sig-parse-err");
    payload[0]["events"]["swap"]["tokenInputs"] = json!([
        {
            "userAccount": WALLET_A,
            "mint": TOKEN_A,
            "rawTokenAmount": {"tokenAmount": "not-a-number", "decimals": 6}
        }
    ]);
    let resp = api_post(&h.app, WEBHOOK_URL, Default::default(), payload).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_processed_signatures_cleanup() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_trade(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t-pos", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "1.0", "0.01", None,
    )
    .await;

    // Pre-fill the signature dedup map past the 5000-entry cleanup threshold.
    {
        let mut seen = h.monitoring_state.processed_signatures.lock();
        for i in 0..5001 {
            seen.insert(format!("pre-{i}"), std::time::Instant::now());
        }
    }

    let resp = api_post(
        &h.app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-cleanup", WALLET_A, "SELL"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn helius_webhook_entry_confirmation_queued() {
    use chimera_operator::engine::entry_confirmation::{
        EntryConfirmationConfig, EntryConfirmationManager,
    };

    let h = build(test_config()).await;
    // High-WQS single wallet (WQS 85 ≥ 80 → SHIELD strategy); consensus-OR-
    // proven gate enabled → BUY rejected with SINGLE_WALLET_UNPROVEN → entry
    // confirmation path. The token-safety cache is pre-seeded so the fast
    // check passes without RPC access.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let parser = harness::make_token_parser_with_seeded_safety(TOKEN_A, "SHIELD");
    let selection = harness::make_selection_service_with_parser(h.db.clone(), parser.clone(), true);
    let entry_confirmation = Arc::new(EntryConfirmationManager::new(
        EntryConfirmationConfig {
            enabled: true,
            wait_secs: 300,
            max_drawdown_pct: "3".parse().unwrap(),
        },
        h.db.clone(),
        h.engine_handle.clone(),
        Some(parser),
        selection.clone(),
    ));
    let app = monitoring_app(
        &h,
        Some(selection),
        Some(entry_confirmation),
        Some(h.circuit_breaker.clone()),
    );

    let resp = api_post(
        &app,
        WEBHOOK_URL,
        Default::default(),
        webhook_payload("sig-entry-conf", WALLET_A, "BUY"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn monitoring_status_db_error() {
    let h = build(test_config()).await;
    // Drop the wallet_monitoring table → the active-wallets count query
    // errors → handler logs and reports 0 (200).
    sqlx::query("DROP TABLE wallet_monitoring CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/status",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["active_wallets"], 0);
}

#[tokio::test]
async fn enable_wallet_monitoring_db_error() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    // Drop the wallets table → get_wallet errors → 500.
    sqlx::query("DROP TABLE wallets CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn enable_wallet_monitoring_upsert_error_after_register() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    // Register succeeds against the mock, then the wallet_monitoring upsert
    // fails (table dropped) → cleanup delete + 500.
    sqlx::query("DROP TABLE wallet_monitoring CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/enable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn disable_wallet_monitoring_db_error() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("mock-webhook-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    sqlx::query("DROP TABLE wallet_monitoring CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/wallets/{}/disable", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn wallet_monitoring_states_db_error() {
    let h = build(test_config()).await;
    sqlx::query("DROP TABLE wallet_monitoring CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/wallets/states",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["wallet_states"].as_array().unwrap().len(), 0);
}
