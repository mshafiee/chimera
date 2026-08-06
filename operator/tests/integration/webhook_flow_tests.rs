//! Webhook Flow Integration Tests
//!
//! Tests the full webhook signal processing flow through the REAL production
//! components:
//! - `hmac_verify` middleware (signature verification, size limits, drift)
//! - `webhook_handler` (payload validation, idempotency, selection pipeline)
//!
//! The middleware tests use a minimal 202 stub handler (the middleware is the
//! code under test); the full-flow test wires the real handler the same way
//! main.rs does (handler state + hmac_verify layer).

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tower::ServiceExt;

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::Database;
use chimera_operator::engine::{SelectionService, SelectionConfig};
use chimera_operator::handlers::{webhook_handler, WebhookState};
use chimera_operator::middleware::{hmac_verify, HmacState};
use chimera_operator::monitoring::SignalAggregator;
use chimera_operator::price_cache::{PriceCache, PriceSource};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal_macros::dec;
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

/// Generate the same HMAC the middleware expects: hex(HMAC-SHA256(secret,
/// timestamp || body)) — update order MUST match middleware/hmac.rs
/// verify_with_secrets.
fn generate_signature(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

const SECRET: &str = "test-secret";

/// A stub handler returning the real handler's accept contract (202 Accepted).
async fn stub_accepted() -> (StatusCode, axum::Json<Value>) {
    (
        StatusCode::ACCEPTED,
        axum::Json(json!({"status": "accepted", "trade_uuid": "stub"})),
    )
}

/// Router with the REAL hmac_verify middleware in front of a stub handler —
/// mirrors main.rs's webhook route composition.
fn middleware_app(max_drift_secs: i64) -> Router {
    let hmac_state = Arc::new(HmacState::new(SECRET.to_string(), max_drift_secs));
    Router::new()
        .route("/api/v1/webhook", post(stub_accepted))
        .layer(from_fn_with_state(hmac_state, hmac_verify))
}

fn signed_request(timestamp: &str, body: &str, secret: &str) -> Request<Body> {
    let signature = generate_signature(secret, timestamp, body);
    Request::builder()
        .method("POST")
        .uri("/api/v1/webhook")
        .header("Content-Type", "application/json")
        .header("X-Signature", signature)
        .header("X-Timestamp", timestamp)
        .body(Body::from(body.to_string()))
        .unwrap()
}

// =============================================================================
// REAL MIDDLEWARE TESTS
// =============================================================================

#[tokio::test]
async fn test_valid_hmac_passes_middleware() {
    let app = middleware_app(300);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body = json!({"strategy": "SHIELD", "token": "BONK"}).to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, SECRET))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "valid HMAC must pass the middleware"
    );
}

#[tokio::test]
async fn test_invalid_hmac_rejected() {
    let app = middleware_app(300);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body = json!({"strategy": "SHIELD", "token": "BONK"}).to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, "wrong-secret"))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "signature produced with a different secret must be rejected"
    );
}

#[tokio::test]
async fn test_missing_signature_header_rejected() {
    let app = middleware_app(300);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("X-Timestamp", chrono::Utc::now().timestamp().to_string())
                .body(Body::from(r#"{"strategy": "SHIELD"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_oversized_signature_header_rejected() {
    let app = middleware_app(300);

    // Oversized header (> 4096 bytes) must be rejected with 400 by the
    // middleware's size guard BEFORE any HMAC work (DoS protection).
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("X-Signature", "a".repeat(5000))
                .header("X-Timestamp", chrono::Utc::now().timestamp().to_string())
                .body(Body::from(r#"{"strategy": "SHIELD"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "oversized signature header must be rejected with 400"
    );
}

#[tokio::test]
async fn test_oversized_timestamp_header_rejected() {
    let app = middleware_app(300);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("X-Signature", "a".repeat(64))
                .header("X-Timestamp", "1".repeat(5000))
                .body(Body::from(r#"{"strategy": "SHIELD"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "oversized timestamp header must be rejected with 400"
    );
}

#[tokio::test]
async fn test_stale_timestamp_rejected() {
    let app = middleware_app(60);

    // 120 seconds in the past with max_drift 60: rejected even with a VALID
    // signature (drift is checked before signature verification).
    let timestamp = (chrono::Utc::now().timestamp() - 120).to_string();
    let body = json!({"strategy": "SHIELD", "token": "BONK"}).to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, SECRET))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "stale timestamp must be rejected by the drift gate"
    );
}

#[tokio::test]
async fn test_future_timestamp_rejected() {
    let app = middleware_app(60);

    let timestamp = (chrono::Utc::now().timestamp() + 120).to_string();
    let body = json!({"strategy": "SHIELD", "token": "BONK"}).to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, SECRET))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "future timestamp must be rejected by the drift gate"
    );
}

#[tokio::test]
async fn test_timestamp_within_drift_accepted() {
    // A wide drift window so the request can never cross the boundary between
    // signing and verification.
    let app = middleware_app(600);
    let timestamp = (chrono::Utc::now().timestamp() - 100).to_string();
    let body = json!({"strategy": "SHIELD", "token": "BONK"}).to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, SECRET))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "timestamp inside the drift window must pass"
    );
}

// =============================================================================
// FULL FLOW: REAL HANDLER BEHIND REAL MIDDLEWARE
// =============================================================================

const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

async fn build_real_webhook_app() -> (Router, Arc<dyn Database>, crate::common::TestDbGuard) {
    let (db, guard) = crate::common::create_test_pg_db().await;

    let config = AppConfig::default(); // trade_mode: Paper — never trades live

    // Register the wallet so the selection pipeline's wallet gate admits it
    // (an unknown wallet is rejected before any token checks).
    db.upsert_wallet(
        WALLET,
        Some(dec!(90.0)),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("wallet upsert must succeed");
    db.update_wallet_status_ext(WALLET, "ACTIVE", None, None)
        .await
        .expect("wallet activation must succeed");

    // Pre-seed the price cache with USDC decimals so the handler's decimals
    // lookup is hermetic (no RPC fetch for a non-cached token).
    let price_cache = Arc::new(PriceCache::new().unwrap());
    price_cache.set_price(USDC_MINT, dec!(1), PriceSource::Jupiter, Some(6));

    let token_parser = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        Arc::new(TokenCache::default_config()),
        Arc::new(
            TokenMetadataFetcher::new("https://api.mainnet-beta.solana.com")
                .with_price_cache(price_cache.clone()),
        ),
    ));

    let position_sizer = Arc::new(chimera_operator::engine::PositionSizer::new(
        db.clone(),
        Arc::new(chimera_operator::config::PositionSizingConfig::default()),
    ));
    let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
    let market_regime = Arc::new(chimera_operator::engine::MarketRegimeDetector::new(
        price_cache.clone(),
    ));
    let helius = Arc::new(
        chimera_operator::monitoring::helius::HeliusClient::new(
            "test_key".to_string(),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        )
        .expect("HeliusClient must construct"),
    );

    let selection_config = SelectionConfig {
        total_capital_sol: config.position_sizing.total_capital_sol,
        max_position_sol: config.strategy.max_position_sol,
        shield_signal_quality_threshold: config.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config.strategy.spear_signal_quality_threshold,
        shield_percent: config.strategy.shield_percent,
        spear_percent: config.strategy.spear_percent,
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        min_liquidity_pumpfun_usd: config.token_safety.min_liquidity_pumpfun_usd,
        allow_graduated_pumpfun: config.token_safety.allow_graduated_pumpfun,
        min_token_age_hours: config.token_safety.min_token_age_hours,
        min_token_age_pumpfun_hours: config.token_safety.min_token_age_pumpfun_hours,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec!(0.10),
        spear_lite_wqs_threshold: 40.0,
        // Gate off in this flow test (single-wallet admission is exercised by
        // the dedicated consensus-OR-proven tests in selection_service_tests).
        require_consensus_or_proven: false,
        min_proven_trades: 10,
        require_proven_positive_pnl: true,
        mirror_gate_enabled: true,
        mirror_gate_min_avg_pct: dec!(1.5),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
    };
    let selection = Arc::new(SelectionService::new(
        db.clone(),
        token_parser.clone(),
        None, // portfolio_heat
        Some(signal_aggregator),
        Some(market_regime),
        Some(helius),
        Some(position_sizer),
        selection_config,
    ));

    let (_engine, engine_handle) = chimera_operator::Engine::new(config.clone(), db.clone());

    let state = Arc::new(WebhookState {
        db: db.clone(),
        engine: engine_handle,
        token_parser,
        circuit_breaker: Arc::new(chimera_operator::CircuitBreaker::new(
            config.circuit_breakers.clone(),
            db.clone(),
            config.position_sizing.total_capital_sol,
        )),
        portfolio_heat: None,
        signal_aggregator: None,
        market_regime: None,
        helius_client: None,
        position_sizer: None,
        total_capital_sol: config.position_sizing.total_capital_sol,
        max_position_sol: config.strategy.max_position_sol,
        shield_signal_quality_threshold: config.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config.strategy.spear_signal_quality_threshold,
        shield_percent: config.strategy.shield_percent,
        spear_percent: config.strategy.spear_percent,
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        selection,
    });

    let hmac_state = Arc::new(HmacState::new(SECRET.to_string(), 300));
    let app = Router::new()
        .route("/api/v1/webhook", post(webhook_handler))
        .with_state(state)
        .layer(from_fn_with_state(hmac_state, hmac_verify));

    (app, db, guard)
}

#[tokio::test]
async fn test_full_webhook_flow_through_production_components() {
    // The REAL handler behind the REAL middleware (main.rs composition). A
    // USDC BUY is deterministic and hermetic: it passes HMAC, payload
    // validation, and the decimals lookup (seeded cache), then the selection
    // pipeline rejects stablecoins as non-speculative — proving every stage
    // ran. The ACCEPTED path requires live token-safety data, so it is not
    // exercised here.
    let (app, _db, _guard) = build_real_webhook_app().await;

    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body = json!({
        "strategy": "SHIELD",
        "token": "USDC",
        "token_address": USDC_MINT,
        "action": "BUY",
        "amount_sol": 0.5,
        "wallet_address": WALLET
    })
    .to_string();

    let response = app
        .oneshot(signed_request(&timestamp, &body, SECRET))
        .await
        .unwrap();

    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "valid HMAC must pass the middleware"
    );
    assert_eq!(
        json["status"], "rejected",
        "USDC BUY must be rejected by the selection pipeline, got: {json}"
    );
    let reason = json["reason"].as_str().unwrap_or("").to_lowercase();
    assert!(
        reason.contains("stablecoin") || reason.contains("speculative"),
        "rejection reason must come from the selection pipeline's token gate, got: {json}"
    );
    // The selection rejection is a 400 BAD_REQUEST (the payload itself was
    // well-formed and passed HMAC + decimals lookup).
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "selection rejection surfaces as 400: {json}"
    );
}

#[tokio::test]
async fn test_webhook_missing_signature_rejected_by_middleware() {
    let (app, _db, _guard) = build_real_webhook_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhook")
                .header("Content-Type", "application/json")
                .header("X-Timestamp", chrono::Utc::now().timestamp().to_string())
                .body(Body::from(r#"{"strategy": "SHIELD"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "missing signature must be rejected by the real middleware"
    );
}

// =============================================================================
// IDEMPOTENCY: generate_trade_uuid DETERMINISM (production function)
// =============================================================================

#[tokio::test]
async fn test_deterministic_uuid_generation() {
    // The real idempotency key: SignalPayload::generate_trade_uuid hashes
    // wallet||token||action||amount||strategy||token_address||exit_fraction
    // (NO timestamp — retries with the same payload must dedupe).
    let payload = chimera_operator::SignalPayload {
        strategy: chimera_operator::Strategy::Shield,
        token: "BONK".to_string(),
        token_address: Some("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string()),
        action: chimera_operator::Action::Buy,
        amount_sol: dec!(0.5),
        wallet_address: WALLET.to_string(),
        trade_uuid: None,
        exit_fraction: None,
    };

    let uuid1 = payload.generate_trade_uuid(1_733_500_000);
    let uuid2 = payload.generate_trade_uuid(1_733_500_001);
    assert_eq!(
        uuid1, uuid2,
        "identical payloads must generate the same UUID regardless of timestamp"
    );

    let mut different_amount = payload.clone();
    different_amount.amount_sol = dec!(0.6);
    assert_ne!(
        uuid1,
        different_amount.generate_trade_uuid(1_733_500_000),
        "different amounts must produce different UUIDs"
    );
}

#[tokio::test]
async fn test_duplicate_trade_uuid_rejection() {
    use chimera_operator::db_abstraction::InsertTrade;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    // This test verifies that the idempotency check works
    // by checking if trade_uuid_exists correctly identifies duplicates.

    // Create an isolated test database (avoids shared-DB duplicate-key residue).
    let (db, _temp_dir) = crate::common::create_test_pg_db().await;

    // Insert a trade with a specific UUID
    let test_uuid = "test-duplicate-uuid-12345";
    db.insert_trade(&InsertTrade {
        trade_uuid: test_uuid.to_string(),
        wallet_address: WALLET.to_string(),
        token_address: USDC_MINT.to_string(),
        token_symbol: Some("USDC".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("0.5").unwrap(),
        status: "ACTIVE".to_string(),
    })
    .await
    .expect("Failed to insert test trade");

    // Check that the UUID exists
    let exists = db
        .trade_uuid_exists(test_uuid)
        .await
        .expect("Failed to check trade UUID");

    assert!(exists, "Trade UUID should exist after insertion");

    // Check that a different UUID doesn't exist
    let different_uuid = "different-uuid-67890";
    let not_exists = db
        .trade_uuid_exists(different_uuid)
        .await
        .expect("Failed to check different trade UUID");

    assert!(!not_exists, "Different trade UUID should not exist");
}
