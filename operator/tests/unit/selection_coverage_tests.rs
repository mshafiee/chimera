//! SelectionService coverage tests.
//!
//! Drives the full unified decision pipeline against a real per-test
//! Postgres database with a seeded token-safety cache (no RPC needed for the
//! fast path) and mocked Helius/Jupiter/RPC endpoints for the age, velocity,
//! shadow-fill, and attachment paths.

use chimera_operator::config::PositionSizingConfig;
use chimera_operator::config::RejectionMuteConfig;
use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::position_sizer::PositionSizer;
use chimera_operator::engine::rejection_mute::RejectionMuteDetector;
use chimera_operator::engine::selection::{
    Ingress, SelectionConfig, SelectionRequest, SelectionService,
};
use chimera_operator::engine::transaction_builder::TransactionBuilder;
use chimera_operator::engine::{DecisionRecorder, LatencyTracker, ShadowConfig, ShadowTrader};
use chimera_operator::engine::{MarketRegimeDetector, PortfolioHeat, SignalQuality};
use chimera_operator::experiment::ToxicFlowDetector;
use chimera_operator::models::{Action, Strategy};
use chimera_operator::monitoring::helius::HeliusClient;
use chimera_operator::monitoring::signal_aggregator::SignalAggregator;
use chimera_operator::monitoring::WalletPerformanceTracker;
use chimera_operator::price_cache::{PriceCache, PriceSource};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal::Decimal;
use serde_json::json;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

#[path = "../common/mock_rpc.rs"]
mod mock_rpc;

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const WALLET_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const WALLET_C: &str = "9HsFJKqobLFZ6QLT7xXhS3ggDfSGTJPUh2Rfug4VFGWh";
const TOKEN: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";
const TOKEN_B: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
const PUMP_TOKEN: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJospump";
const PUMP_TOKEN_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJospump";
const INVALID_BASE58: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsl"; // contains 'l'

fn base_config() -> SelectionConfig {
    SelectionConfig {
        total_capital_sol: dec("10.0"),
        max_position_sol: dec("5.0"),
        shield_signal_quality_threshold: 0.55,
        spear_signal_quality_threshold: 0.30,
        shield_percent: 60,
        spear_percent: 40,
        min_liquidity_shield_usd: dec("10000"),
        min_liquidity_spear_usd: dec("5000"),
        min_liquidity_pumpfun_usd: dec("25000"),
        allow_graduated_pumpfun: true,
        min_token_age_hours: 1.0,
        min_token_age_pumpfun_hours: 4.0,
        min_token_age_proven_hours: 0.1,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven: false,
        min_proven_trades: 10,
        require_proven_positive_pnl: true,
        mirror_gate_enabled: false,
        mirror_gate_min_avg_pct: dec("1.5"),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
        wallet_tstat_enabled: false,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
        token_velocity_gate_enabled: false,
        token_min_liquidity_velocity: 0.10,
        token_max_curve_completion: 0.85,
        cluster_gate_enabled: false,
        cluster_min_profitable_wallets: 3,
        averaging_down_enabled: false,
        averaging_down_window_hours: 12,
        averaging_down_min_buys: 2,
        averaging_down_min_drop_pct: dec("3.0"),
        pump_chase_enabled: false,
        pump_chase_max_delta_pct: dec("10.0"),
        stop_loss_cooldown_enabled: false,
        stop_loss_cooldown_hours: 12,
        stop_loss_cooldown_loss_pct: dec("5.0"),
        pump_since_whale_guard_enabled: true,
        max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
    }
}

/// TokenParser whose safety cache is seeded for `{token}:{strategy}` (both
/// strategy keys, since SPEAR wallets route to SPEAR).
fn seeded_parser(token: &str, strategy: &str, liquidity: &str, safe: bool) -> Arc<TokenParser> {
    let cache = Arc::new(TokenCache::new(1000, 300));
    for strat in ["SHIELD", "SPEAR"] {
        let key = if strat == strategy || strat == "SHIELD" {
            format!("{token}:{strat}")
        } else {
            format!("{token}:{strat}")
        };
        cache.insert(
            key,
            chimera_operator::token::TokenSafetyResult {
                safe,
                rejection_reason: if safe {
                    None
                } else {
                    Some("mock unsafe".to_string())
                },
                honeypot_checked: false,
                liquidity_checked: true,
                liquidity_usd: Some(dec(liquidity)),
            },
        );
    }
    let fetcher = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        "http://127.0.0.1:1",
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache,
        fetcher,
    ))
}

fn build_service(
    db: Arc<dyn Database>,
    parser: Arc<TokenParser>,
    config: SelectionConfig,
) -> SelectionService {
    SelectionService::new(db, parser, None, None, None, None, None, config)
}

#[allow(clippy::too_many_arguments)]
fn build_service_full(
    db: Arc<dyn Database>,
    parser: Arc<TokenParser>,
    config: SelectionConfig,
    heat: Option<Arc<PortfolioHeat>>,
    aggregator: Option<Arc<SignalAggregator>>,
    regime: Option<Arc<MarketRegimeDetector>>,
    helius: Option<Arc<HeliusClient>>,
    sizer: Option<Arc<PositionSizer>>,
) -> SelectionService {
    SelectionService::new(db, parser, heat, aggregator, regime, helius, sizer, config)
}

fn request(token: &str, action: Action) -> SelectionRequest {
    SelectionRequest {
        wallet_address: WALLET.to_string(),
        token_address: token.to_string(),
        action,
        source_amount_sol: dec("0.5"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
        whale_entry_price: None,
    }
}

async fn seed_wallet(db: &Arc<dyn Database>, address: &str, status: &str, wqs: f64) {
    let pool = pg_pool(db);
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, win_rate) \
         VALUES ($1, $2, $3, 0.9, 0.6) \
         ON CONFLICT (address) DO UPDATE SET status = EXCLUDED.status, wqs_score = EXCLUDED.wqs_score, wqs_confidence = EXCLUDED.wqs_confidence",
    )
    .bind(address)
    .bind(status)
    .bind(wqs)
    .execute(&pool)
    .await
    .unwrap();
}

/// Helius mock whose transactions endpoint returns the given transactions.
async fn helius_mock_with_txs(
    txs: Vec<serde_json::Value>,
) -> (mock_rpc::HeliusApiMock, Arc<HeliusClient>) {
    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = txs;
    let old = std::env::var("HELIUS_API_BASE_URL").ok();
    std::env::set_var("HELIUS_API_BASE_URL", &helius.url);
    let client = Arc::new(
        HeliusClient::new(
            "test-key".to_string(),
            Arc::new(parking_lot::RwLock::new(HashMap::new())),
        )
        .expect("helius client"),
    );
    match old {
        Some(v) => std::env::set_var("HELIUS_API_BASE_URL", v),
        None => std::env::remove_var("HELIUS_API_BASE_URL"),
    }
    (helius, client)
}

/// One "old" transaction for a mint → age = now - timestamp.
fn tx_with_timestamp(ts: i64) -> serde_json::Value {
    serde_json::json!({
        "signature": format!("sig-{ts}"),
        "timestamp": ts,
        "transactionError": null,
        "tokenTransfers": [],
        "nativeTransfers": []
    })
}

fn old_tx() -> serde_json::Value {
    tx_with_timestamp(chrono::Utc::now().timestamp() - 10 * 3600) // 10h old
}

fn young_tx() -> serde_json::Value {
    tx_with_timestamp(chrono::Utc::now().timestamp() - 300) // 5 min old
}

// ── Basic gates ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_invalid_token_address_rejected() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    );
    let d = service.decide(&request("not-a-token", Action::Buy)).await;
    assert!(!d.admitted);
    assert_eq!(d.rejection_code, Some("INVALID_TOKEN_ADDRESS"));
}

#[tokio::test]
async fn test_unknown_and_inactive_wallets() {
    let (db, _guard) = create_test_db().await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("UNKNOWN_WALLET"));

    seed_wallet(&db, WALLET, "CANDIDATE", 80.0).await;
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("WALLET_NOT_ACTIVE"));
}

#[tokio::test]
async fn test_wqs_too_low_and_strategy_routing() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 60.0).await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("WQS_TOO_LOW"));
}

#[tokio::test]
async fn test_toxic_wallet_rejected() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let toxic = Arc::new(ToxicFlowDetector::new(Default::default()));
    toxic
        .register_wallet_promotion(WALLET.to_string(), 0.5)
        .await
        .unwrap();
    toxic
        .record_entry(WALLET.to_string(), false, 0.1)
        .await
        .unwrap(); // roi drop -> toxic
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_toxic_detector(toxic);
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("TOXIC_WALLET"));
}

#[tokio::test]
async fn test_muted_wallet_rejected() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let mute = Arc::new(RejectionMuteDetector::new(RejectionMuteConfig {
        enabled: true,
        window_size: 10,
        min_window_samples: 5,
        hard_rejection_threshold: 0.80,
        mute_duration_hours: 6,
    }));
    // 8 hard rejections in a 10-window → muted.
    for _ in 0..8 {
        mute.record_decision(WALLET, false, Some("PUMPFUN_INSUFFICIENT_LIQUIDITY"))
            .await
            .unwrap();
    }
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_mute_detector(mute);
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("WALLET_MUTED"));
}

#[tokio::test]
async fn test_non_speculative_and_pumpfun_bonding_curve() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    );

    let d = service.decide(&request(SOL_MINT, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("NON_SPECULATIVE_TOKEN"));

    // pump.fun token with graduated pumpfun disabled.
    let mut cfg = base_config();
    cfg.allow_graduated_pumpfun = false;
    let service = build_service(
        db.clone(),
        seeded_parser(PUMP_TOKEN, "SHIELD", "100000", true),
        cfg,
    );
    let d = service.decide(&request(PUMP_TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("PUMPFUN_BONDING_CURVE"));
}

#[tokio::test]
async fn test_token_safety_gates() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", false),
        base_config(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("TOKEN_UNSAFE"));
}

#[tokio::test]
async fn test_token_age_gates() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    seed_wallet(&db, WALLET_B, "ACTIVE", 75.0).await;
    seed_wallet(&db, WALLET_C, "ACTIVE", 90.0).await;
    let parser = seeded_parser(TOKEN, "SHIELD", "100000", true);

    // Unknown age + SPEAR (wqs 75) → TOKEN_AGE_UNKNOWN.
    let (_m, helius) = helius_mock_with_txs(vec![]).await;
    let service = build_service_full(
        db.clone(),
        parser.clone(),
        base_config(),
        None,
        None,
        None,
        Some(helius),
        None,
    );
    let mut req = request(TOKEN, Action::Buy);
    req.wallet_address = WALLET_B.to_string();
    let d = service.decide(&req).await;
    assert_eq!(d.rejection_code, Some("TOKEN_AGE_UNKNOWN"));

    // Unknown age + SHIELD → warn-and-allow; then liquidity passes → admitted.
    let (_m2, helius2) = helius_mock_with_txs(vec![]).await;
    let service = build_service_full(
        db.clone(),
        parser.clone(),
        base_config(),
        None,
        None,
        None,
        Some(helius2),
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(
        d.admitted,
        "SHIELD warn-and-allow for unknown age: {:?}",
        d.rejection_code
    );

    // Too young + unproven wallet → TOKEN_TOO_NEW.
    let (_m3, helius3) = helius_mock_with_txs(vec![young_tx()]).await;
    let service = build_service_full(
        db.clone(),
        parser.clone(),
        base_config(),
        None,
        None,
        None,
        Some(helius3),
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("TOKEN_TOO_NEW"));

    // Old enough → admitted.
    let (_m4, helius4) = helius_mock_with_txs(vec![old_tx()]).await;
    let service = build_service_full(
        db.clone(),
        parser,
        base_config(),
        None,
        None,
        None,
        Some(helius4),
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "old token admits: {:?}", d.rejection_code);
    assert!(d.token_age_hours.is_some());
    assert_eq!(d.strategy, Some(Strategy::Shield));
}

#[tokio::test]
async fn test_liquidity_fetch_failure_fails_closed() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    // fast_check returns safe with NO liquidity for a random never-listed
    // mint → get_liquidity's DexScreener fetch returns $0 (or errors when
    // offline) → fail-closed $0 → liquidity floor rejects.
    use solana_sdk::signature::Signer;
    let unlisted = solana_sdk::signature::Keypair::new().pubkey().to_string();
    let cache = Arc::new(TokenCache::new(1000, 300));
    cache.insert(
        format!("{unlisted}:SHIELD"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: false,
            liquidity_usd: None,
        },
    );
    let fetcher = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        "http://127.0.0.1:1",
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    let parser = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache,
        fetcher,
    ));
    let service = build_service(db.clone(), parser, base_config());
    let d = service.decide(&request(&unlisted, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("LIQUIDITY_BELOW_MINIMUM"));
}

#[tokio::test]
async fn test_proven_wallet_age_waiver() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // T-stat-proven wallet: 10 mirror_main exits at +5% (stddev 0 → t=∞).
    for i in 0..10 {
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress) \
             VALUES ($1, 'd', 'run', $2, $3, 'SHIELD', true, 0.1, 'webhook')",
        )
        .bind(format!("waiver-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol) VALUES ($1, 'mirror_main', 5.0, 0.005)",
        )
        .bind(format!("waiver-{i}"))
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut cfg = base_config();
    cfg.wallet_tstat_enabled = true;
    // Token 20 minutes old: below the 1h global min but above the 0.1h
    // proven floor → the age waiver admits.
    let (_hel, helius) = helius_mock_with_txs(vec![tx_with_timestamp(
        chrono::Utc::now().timestamp() - 1200,
    )])
    .await;
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg,
        None,
        None,
        None,
        Some(helius),
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(
        d.admitted,
        "age waiver admits proven wallets: {:?}",
        d.rejection_code
    );
}

#[tokio::test]
async fn test_liquidity_gates() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // Below SHIELD floor.
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "1000", true),
        base_config(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("LIQUIDITY_BELOW_MINIMUM"));

    // pump.fun below its (higher) floor → dedicated code.
    let service = build_service(
        db.clone(),
        seeded_parser(PUMP_TOKEN, "SHIELD", "1000", true),
        base_config(),
    );
    let d = service.decide(&request(PUMP_TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("PUMPFUN_INSUFFICIENT_LIQUIDITY"));
}

// ── Consensus / cluster / proven gates ───────────────────────────────────────

#[tokio::test]
async fn test_consensus_or_proven_gate() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let mut cfg = base_config();
    cfg.require_consensus_or_proven = true;

    // Unproven single wallet → rejected.
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    // Too few closed trades → still unproven.
    for i in 0..5 {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, net_pnl_sol) \
             VALUES ($1, $2, $3, 'SHIELD', 'BUY', 0.5, 'CLOSED', 0.1)",
        )
        .bind(format!("proven-few-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pg_pool(&db))
        .await
        .unwrap();
    }
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    // Enough trades but non-positive PnL → still unproven.
    for i in 0..10 {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, net_pnl_sol) \
             VALUES ($1, $2, $3, 'SHIELD', 'BUY', 0.5, 'CLOSED', -0.1)",
        )
        .bind(format!("proven-neg-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pg_pool(&db))
        .await
        .unwrap();
    }
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    // Proven via closed-trade ledger (10 closed trades, positive net PnL).
    for i in 0..10 {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, net_pnl_sol) \
             VALUES ($1, $2, $3, 'SHIELD', 'BUY', 0.5, 'CLOSED', 0.1)",
        )
        .bind(format!("proven-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pg_pool(&db))
        .await
        .unwrap();
    }
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "proven wallet passes: {:?}", d.rejection_code);

    // bypass_consensus_proven → gate skipped even for unproven wallets.
    let (db2, _guard2) = create_test_db().await;
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(
        db2.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service
        .decide_with_options(&request(TOKEN, Action::Buy), true)
        .await;
    assert!(d.admitted, "bypass admits: {:?}", d.rejection_code);
}

#[tokio::test]
async fn test_tstat_gate_variants() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    let mut cfg = base_config();
    cfg.require_consensus_or_proven = true;
    cfg.wallet_tstat_enabled = true;

    // No shadow data → unproven.
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    async fn seed_shadow(
        pool: &sqlx::Pool<sqlx::Postgres>,
        wallet: &str,
        prefix: &str,
        pnl: &str,
        n: usize,
    ) {
        for i in 0..n {
            sqlx::query(
                "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress) \
                 VALUES ($1, 'd', 'run', $2, '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', 'SHIELD', true, 0.1, 'webhook')",
            )
            .bind(format!("{prefix}-{i}"))
            .bind(wallet)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol) VALUES ($1, 'mirror_main', $2, 0.0)",
            )
            .bind(format!("{prefix}-{i}"))
            .bind(dec(pnl))
            .execute(pool)
            .await
            .unwrap();
        }
    }

    // Insufficient samples (3 < 10) → unproven.
    seed_wallet(&db, WALLET_B, "ACTIVE", 80.0).await;
    seed_shadow(&pool, WALLET_B, "ts-few", "5.0", 3).await;
    let mut req = request(TOKEN, Action::Buy);
    req.wallet_address = WALLET_B.to_string();
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&req).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    // Enough samples but non-positive mean → unproven.
    seed_wallet(&db, WALLET_C, "ACTIVE", 80.0).await;
    seed_shadow(&pool, WALLET_C, "ts-neg", "-3.0", 10).await;
    let mut req = request(TOKEN, Action::Buy);
    req.wallet_address = WALLET_C.to_string();
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&req).await;
    assert_eq!(d.rejection_code, Some("SINGLE_WALLET_UNPROVEN"));

    // Positive mean with non-zero variance → t = mean/se > threshold.
    let w4 = "7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg";
    seed_wallet(&db, w4, "ACTIVE", 80.0).await;
    seed_shadow(&pool, w4, "ts-var", "5.0", 10).await;
    sqlx::query("UPDATE shadow_exits SET pnl_pct = 4.0 WHERE shadow_id LIKE 'ts-var-%' AND shadow_id != 'ts-var-0'")
        .execute(&pool)
        .await
        .unwrap();
    let mut req = request(TOKEN, Action::Buy);
    req.wallet_address = w4.to_string();
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg,
    );
    let d = service.decide(&req).await;
    assert!(
        d.admitted,
        "significant positive t-stat admits: {:?}",
        d.rejection_code
    );
}

#[tokio::test]
async fn test_consensus_and_cluster_detection() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    seed_wallet(&db, WALLET_B, "ACTIVE", 80.0).await;
    seed_wallet(&db, WALLET_C, "ACTIVE", 80.0).await;

    // Two wallets already BUYing the token in the aggregator window → consensus.
    let aggregator = Arc::new(SignalAggregator::new(db.clone()));
    aggregator
        .add_signal(WALLET, TOKEN, "BUY", dec("1.0"))
        .await;
    aggregator
        .add_signal(WALLET_B, TOKEN, "BUY", dec("1.0"))
        .await;
    let mut cfg = base_config();
    cfg.require_consensus_or_proven = true;
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
        None,
        Some(aggregator),
        None,
        None,
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "consensus admits: {:?}", d.rejection_code);
    assert!(d.is_consensus);

    // Cluster: 3 wallets with profitable t-stat shadow evidence on the token.
    let (db2, _guard2) = create_test_db().await;
    let pool2 = pg_pool(&db2);
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    for w in [WALLET, WALLET_B, WALLET_C] {
        for i in 0..10 {
            sqlx::query(
                "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress) \
                 VALUES ($1, 'd', 'run', $2, $3, 'SHIELD', true, 0.1, 'webhook')",
            )
            .bind(format!("cl-{w}-{i}"))
            .bind(w)
            .bind(TOKEN)
            .execute(&pool2)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol) \
                 VALUES ($1, 'mirror_main', 5.0, 0.005)",
            )
            .bind(format!("cl-{w}-{i}"))
            .execute(&pool2)
            .await
            .unwrap();
        }
    }
    let mut cfg2 = base_config();
    cfg2.require_consensus_or_proven = true;
    cfg2.wallet_tstat_enabled = true;
    cfg2.cluster_gate_enabled = true;
    cfg2.cluster_min_profitable_wallets = 3;
    let aggregator2 = Arc::new(SignalAggregator::new(db2.clone()));
    for w in [WALLET, WALLET_B, WALLET_C] {
        aggregator2.add_signal(w, TOKEN, "BUY", dec("1.0")).await;
    }
    let service = build_service_full(
        db2.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg2,
        None,
        Some(aggregator2),
        None,
        None,
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "cluster admits: {:?}", d.rejection_code);
}

// ── Mirror / velocity / cooldown / averaging / pump-chase gates ─────────────

#[tokio::test]
async fn test_mirror_gate_paths() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let mut cfg = base_config();
    cfg.mirror_gate_enabled = true;

    async fn seed_mirror(pool: &sqlx::Pool<sqlx::Postgres>, token: &str, pnl_pcts: &[&str]) {
        for (i, p) in pnl_pcts.iter().enumerate() {
            sqlx::query(
                "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress) \
                 VALUES ($1, 'd', 'run', '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', $2, 'SHIELD', true, 0.1, 'webhook')",
            )
            .bind(format!("mir-{token}-{i}"))
            .bind(token)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol) VALUES ($1, 'mirror_main', $2, 0.0)",
            )
            .bind(format!("mir-{token}-{i}"))
            .bind(dec(p))
            .execute(pool)
            .await
            .unwrap();
        }
    }

    // Negative average → SHADOW_MIRROR_NEGATIVE.
    seed_mirror(&pool, TOKEN, &["-2.0"; 10]).await;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SHADOW_MIRROR_NEGATIVE"));

    // Insufficient samples → SHADOW_MIRROR_INSUFFICIENT.
    let (db2, _guard2) = create_test_db().await;
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(
        db2.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SHADOW_MIRROR_INSUFFICIENT"));

    // Positive average → admitted.
    let (db3, _guard3) = create_test_db().await;
    let pool3 = pg_pool(&db3);
    seed_wallet(&db3, WALLET, "ACTIVE", 80.0).await;
    seed_mirror(&pool3, TOKEN, &["2.0"; 10]).await;
    let service = build_service(
        db3.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "positive mirror admits: {:?}", d.rejection_code);
}

#[tokio::test]
async fn test_velocity_gate_paths() {
    // Mock RPC serving a bonding-curve account for PUMP_TOKEN.
    fn curve_account(real_sol: u64, complete: bool) -> serde_json::Value {
        let mut data = vec![0u8; 49];
        data[8..16].copy_from_slice(&1_000_000_000_000u64.to_le_bytes());
        data[16..24].copy_from_slice(&30_000_000_000u64.to_le_bytes());
        data[24..32].copy_from_slice(&500_000_000_000u64.to_le_bytes());
        data[32..40].copy_from_slice(&real_sol.to_le_bytes());
        data[40..48].copy_from_slice(&1_000_000_000_000u64.to_le_bytes());
        data[48] = complete as u8;
        mock_rpc::base64_account(data, "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
    }
    let sigs: Vec<serde_json::Value> = (0..10)
        .map(|i| serde_json::json!({"signature": format!("sig-{i}"), "slot": 1, "err": null}))
        .collect();

    // Late-curve dump zone: real_sol 80e9 → 94% complete > 85%.
    let sigs1 = sigs.clone();
    let (url, _server) =
        mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(move |method, _| match method {
            "getAccountInfo" => Some(curve_account(80_000_000_000, false)),
            "getSignaturesForAddress" => Some(serde_json::json!(sigs1)),
            _ => None,
        }))
        .await;
    let fetcher = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        &url,
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    let cache = Arc::new(TokenCache::new(1000, 300));
    let parser = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache.clone(),
        fetcher,
    ));
    // Seed the fast-check cache for the pump token (shared cache Arc).
    cache.insert(
        format!("{PUMP_TOKEN}:SHIELD"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );

    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let mut cfg = base_config();
    cfg.token_velocity_gate_enabled = true;
    let service = build_service(db.clone(), parser.clone(), cfg.clone());
    let d = service.decide(&request(PUMP_TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("BONDING_CURVE_DUMP_ZONE"));

    // Slow velocity: real_sol 1e9 over 10 swaps = 0.1 SOL/trade? below 0.1? use 1e9/10 = 0.1 → equal; use 500_000_000 → 0.05 < 0.1.
    let sigs2 = sigs.clone();
    let (url2, _server2) =
        mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(move |method, _| match method {
            "getAccountInfo" => Some(curve_account(500_000_000, false)),
            "getSignaturesForAddress" => Some(serde_json::json!(sigs2)),
            _ => None,
        }))
        .await;
    let fetcher2 = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        &url2,
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    let cache2 = Arc::new(TokenCache::new(1000, 300));
    let parser2 = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache2.clone(),
        fetcher2,
    ));
    cache2.insert(
        format!("{PUMP_TOKEN_B}:SHIELD"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );
    let (db2, _guard2) = create_test_db().await;
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(db2.clone(), parser2.clone(), cfg.clone());
    let d = service.decide(&request(PUMP_TOKEN_B, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("LOW_LIQUIDITY_VELOCITY"));
}

#[tokio::test]
async fn test_velocity_gate_graduated_and_error_paths() {
    fn curve_account(real_sol: u64, complete: bool) -> serde_json::Value {
        let mut data = vec![0u8; 49];
        data[8..16].copy_from_slice(&1_000_000_000_000u64.to_le_bytes());
        data[16..24].copy_from_slice(&30_000_000_000u64.to_le_bytes());
        data[24..32].copy_from_slice(&500_000_000_000u64.to_le_bytes());
        data[32..40].copy_from_slice(&real_sol.to_le_bytes());
        data[40..48].copy_from_slice(&1_000_000_000_000u64.to_le_bytes());
        data[48] = complete as u8;
        mock_rpc::base64_account(data, "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
    }
    let sigs: Vec<serde_json::Value> = (0..10)
        .map(|i| serde_json::json!({"signature": format!("sig-{i}"), "slot": 1, "err": null}))
        .collect();

    // Graduated curve (complete=true) → velocity gate does not apply.
    let sigs1 = sigs.clone();
    let (url, _server) =
        mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(move |method, _| match method {
            "getAccountInfo" => Some(curve_account(85_000_000_000, true)),
            "getSignaturesForAddress" => Some(serde_json::json!(sigs1)),
            _ => None,
        }))
        .await;
    let cache = Arc::new(TokenCache::new(1000, 300));
    cache.insert(
        format!("{PUMP_TOKEN}:SHIELD"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );
    let parser = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache,
        Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            &url,
            None,
            "http://127.0.0.1:1".to_string(),
        )),
    ));
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let mut cfg = base_config();
    cfg.token_velocity_gate_enabled = true;
    let service = build_service(db.clone(), parser, cfg.clone());
    let d = service.decide(&request(PUMP_TOKEN, Action::Buy)).await;
    assert!(d.admitted, "graduated curve passes: {:?}", d.rejection_code);

    // Curve fetch error (account value null) → fail-open.
    let (url2, _server2) =
        mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(move |method, _| match method {
            "getAccountInfo" => {
                Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": null}))
            }
            "getSignaturesForAddress" => Some(serde_json::json!(sigs)),
            _ => None,
        }))
        .await;
    let cache2 = Arc::new(TokenCache::new(1000, 300));
    cache2.insert(
        format!("{PUMP_TOKEN_B}:SHIELD"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );
    let parser2 = Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache2,
        Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            &url2,
            None,
            "http://127.0.0.1:1".to_string(),
        )),
    ));
    let (db2, _guard2) = create_test_db().await;
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    let service = build_service(db2.clone(), parser2, cfg);
    let d = service.decide(&request(PUMP_TOKEN_B, Action::Buy)).await;
    assert!(
        d.admitted,
        "curve fetch error fails open: {:?}",
        d.rejection_code
    );
}

#[tokio::test]
async fn test_stop_loss_cooldown_gate() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // Closed position with realized loss ≥ 5% of entry → cooldown active.
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, created_at) \
         VALUES ('cool-trade', $1, $2, 'SHIELD', 'BUY', 1.0, 'CLOSED', NOW() - INTERVAL '1 hour')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_net_pnl_sol, closed_at) \
         VALUES ('cool-trade', $1, $2, 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', -0.06, NOW() - INTERVAL '1 hour')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();

    let mut cfg = base_config();
    cfg.stop_loss_cooldown_enabled = true;
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg.clone(),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("STOP_LOSS_COOLDOWN"));

    // No recent loss on a different token → cooldown check passes (Ok(false)).
    let other_token = "9HsFJKqobLFZ6QLT7xXhS3ggDfSGTJPUh2Rfug4VFGWh";
    let service = build_service(
        db.clone(),
        seeded_parser(other_token, "SHIELD", "100000", true),
        cfg,
    );
    let d = service.decide(&request(other_token, Action::Buy)).await;
    assert!(
        d.admitted,
        "no recent loss passes the cooldown: {:?}",
        d.rejection_code
    );
}

#[tokio::test]
async fn test_averaging_down_gate() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // Whale bought the token 3x, each lower (1.0 → 0.8 → 0.6): falling knife.
    for (i, price) in ["1.0", "0.8", "0.6"].iter().enumerate() {
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, entry_price_usd, ingress) \
             VALUES ($1, 'd', 'run', $2, $3, 'SHIELD', true, 0.1, $4, 'webhook')",
        )
        .bind(format!("avg-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .bind(dec(price))
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut cfg = base_config();
    cfg.averaging_down_enabled = true;
    cfg.averaging_down_min_buys = 2;
    cfg.averaging_down_min_drop_pct = dec("3.0");
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("WHALE_AVERAGING_DOWN"));
}

#[tokio::test]
async fn test_pump_chase_gate() {
    let (db, _guard) = create_test_db().await;
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    let cache =
        Arc::new(PriceCache::with_jupiter_price_api("http://127.0.0.1:1".to_string()).unwrap());
    // History: 20 minutes ago price 1.0, now 1.5 → +50% in 15m.
    cache.set_price_with_time(
        TOKEN,
        dec("1.0"),
        PriceSource::Cached,
        chrono::Utc::now() - chrono::Duration::minutes(20),
        None,
    );
    cache.set_price_with_time(
        TOKEN,
        dec("1.5"),
        PriceSource::Cached,
        chrono::Utc::now() - chrono::Duration::seconds(30),
        None,
    );

    let mut cfg = base_config();
    cfg.pump_chase_enabled = true;
    cfg.pump_chase_max_delta_pct = dec("10.0");
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg,
    )
    .with_price_cache(cache);
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("PUMP_CHASE"));

    // No history → fail-open → admitted.
    let (db2, _guard2) = create_test_db().await;
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    let cache2 =
        Arc::new(PriceCache::with_jupiter_price_api("http://127.0.0.1:1".to_string()).unwrap());
    let mut cfg2 = base_config();
    cfg2.pump_chase_enabled = true;
    let service = build_service(
        db2.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        cfg2,
    )
    .with_price_cache(cache2);
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(
        d.admitted,
        "no price history fails open: {:?}",
        d.rejection_code
    );
}

// ── Quality / sizing / heat ──────────────────────────────────────────────────

#[tokio::test]
async fn test_signal_quality_too_low() {
    let (db, _guard) = create_test_db().await;
    // SHIELD wallet: liquidity exactly at the floor (0.1 score) + a 2h-old
    // token (passes the 1h age gate; age score 0.3) → 0.32 + 0.10 + 0.03 =
    // 0.45 < 0.55 shield threshold.
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let (_hel, helius) = helius_mock_with_txs(vec![tx_with_timestamp(
        chrono::Utc::now().timestamp() - 2 * 3600,
    )])
    .await;
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "10000", true),
        base_config(),
        None,
        None,
        None,
        Some(helius),
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("SIGNAL_QUALITY_TOO_LOW"));
}

#[tokio::test]
async fn test_portfolio_heat_limits() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // Tiny capital + a big open position → portfolio heat full.
    let tiny_heat = Arc::new(PortfolioHeat::new(db.clone(), dec("0.5")));
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ('heat-trade', $1, $2, 'SHIELD', 'BUY', 1.0, 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state) \
         VALUES ('heat-trade', $1, $2, 'SHIELD', 1.0, 1.0, 'sig', 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
        Some(tiny_heat),
        None,
        None,
        None,
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("PORTFOLIO_HEAT_LIMIT"));

    // Total heat below the 20% cap but SHIELD allocation full: capital 100 →
    // max heat 20 SOL; SHIELD share = 20×0.6 = 12 SOL; a 15 SOL SHIELD
    // position (15% total heat) exceeds the allocation.
    let (db2, _guard2) = create_test_db().await;
    let pool2 = pg_pool(&db2);
    seed_wallet(&db2, WALLET, "ACTIVE", 80.0).await;
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ('heat-trade2', $1, $2, 'SHIELD', 'BUY', 15.0, 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool2)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state) \
         VALUES ('heat-trade2', $1, $2, 'SHIELD', 15.0, 1.0, 'sig', 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool2)
    .await
    .unwrap();
    let heat = Arc::new(PortfolioHeat::new(db2.clone(), dec("100.0")));
    let service = build_service_full(
        db2.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
        Some(heat),
        None,
        None,
        None,
        None,
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("STRATEGY_HEAT_LIMIT"));
}

#[tokio::test]
async fn test_wallet_performance_boost_path() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // 20 closed trades with positive copy PnL → BOOSTED wallet.
    for i in 0..20 {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, net_pnl_sol) \
             VALUES ($1, $2, $3, 'SHIELD', 'BUY', 0.5, 'CLOSED', 0.05)",
        )
        .bind(format!("boost-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pool)
        .await
        .unwrap();
    }
    let mut app = chimera_operator::config::AppConfig::default();
    let mut monitoring = chimera_operator::config::MonitoringConfig::default();
    monitoring.wallet_boost_enabled = true;
    app.monitoring = Some(monitoring);
    let perf = Arc::new(WalletPerformanceTracker::new_with_config(
        db.clone(),
        Arc::new(app),
    ));
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_wallet_performance(perf);
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "{:?}", d.rejection_code);
}

#[tokio::test]
async fn test_position_size_zero_rejected() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    // 15 closed LOSING trades → Kelly full_kelly_cap below min_size_sol →
    // the sizer returns zero → POSITION_SIZE_ZERO.
    for i in 0..15 {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, net_pnl_sol) \
             VALUES ($1, $2, $3, 'SHIELD', 'BUY', 0.5, 'CLOSED', -0.05)",
        )
        .bind(format!("neg-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .execute(&pool)
        .await
        .unwrap();
    }
    let mut sizing = PositionSizingConfig::default();
    sizing.use_kelly_sizing = true;
    let sizer = Arc::new(PositionSizer::new(db.clone(), Arc::new(sizing)));
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
        None,
        None,
        None,
        None,
        Some(sizer),
    );
    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert_eq!(d.rejection_code, Some("POSITION_SIZE_ZERO"));
}

#[tokio::test]
async fn test_admitted_buy_full_pipeline() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    seed_wallet(&db, WALLET_B, "ACTIVE", 75.0).await;

    let sizer = Arc::new(PositionSizer::new(
        db.clone(),
        Arc::new(PositionSizingConfig::default()),
    ));
    let aggregator = Arc::new(SignalAggregator::new(db.clone()));
    let regime = Arc::new(MarketRegimeDetector::new(Arc::new(
        PriceCache::with_jupiter_price_api("http://127.0.0.1:1".to_string()).unwrap(),
    )));
    let heat = Arc::new(PortfolioHeat::new(db.clone(), dec("1000")));
    // Helius mock returns an old transaction so the age gate passes for both
    // SHIELD and SPEAR wallets.
    let (_hel, helius) = helius_mock_with_txs(vec![old_tx()]).await;
    let service = build_service_full(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
        Some(heat),
        Some(aggregator),
        Some(regime),
        Some(helius),
        Some(sizer),
    );

    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "{:?}", d.rejection_code);
    assert_eq!(d.strategy, Some(Strategy::Shield));
    assert!(d.size_sol.unwrap() > dec("0"));
    assert!(d.quality_score.is_some());
    assert!(d.regime_multiplier.is_some());
    assert!(d.liquidity_usd.is_some());
    assert_eq!(d.config_hash, service.config_hash());

    // Signal aggregation was persisted.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM signal_aggregation")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Spear routing for wqs 75.
    let mut req = request(TOKEN, Action::Buy);
    req.wallet_address = WALLET_B.to_string();
    let d = service.decide(&req).await;
    assert!(d.admitted, "{:?}", d.rejection_code);
    assert_eq!(d.strategy, Some(Strategy::Spear));
}

#[tokio::test]
async fn test_admitted_with_all_attachments() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;

    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let mut config = chimera_operator::config::AppConfig::default();
    config.jupiter.api_url = jup_url;
    let quote_client = Arc::new(
        TransactionBuilder::new(
            Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
                "http://127.0.0.1:1".to_string(),
            )),
            Arc::new(config),
        )
        .expect("tx builder"),
    );
    let tracker = Arc::new(LatencyTracker::new(10));
    let recorder = Arc::new(DecisionRecorder::new(
        db.clone(),
        Arc::new(chimera_operator::engine::run_context::RunContext::new(
            "hash",
            &[WALLET.to_string()],
            chrono::Utc::now(),
        )),
    ));
    let shadow = Arc::new(ShadowTrader::new(
        db.clone(),
        Arc::new(PriceCache::with_jupiter_price_api("http://127.0.0.1:1".to_string()).unwrap()),
        ShadowConfig {
            enabled: true,
            position_size_sol: dec("0.1"),
            max_lifetime: Duration::from_secs(3600),
            profit_config: Arc::new(chimera_operator::config::ProfitManagementConfig::default()),
            run_id: "run".to_string(),
        },
        None,
    ));
    let wallet_perf = Arc::new(WalletPerformanceTracker::new(db.clone()));
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_decision_recorder(recorder.clone())
    .with_shadow_fill(quote_client.clone(), tracker)
    .with_shadow_trader(shadow)
    .with_wallet_performance(wallet_perf)
    .with_dexscreener(Arc::new(
        chimera_operator::monitoring::dexscreener::DexScreenerClient::new(
            Arc::new(chimera_operator::monitoring::rate_limiter::RateLimiter::new(40, 1)),
            Arc::new(chimera_operator::engine::VolumeCache::default()),
        ),
    ));

    let d = service.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d.admitted, "{:?}", d.rejection_code);

    // Decision row persisted (fire-and-forget) with the fill model attached.
    let mut persisted = false;
    for _ in 0..200 {
        let v: Option<Option<String>> = sqlx::query_scalar(
            "SELECT simulated_fill_model_version FROM decision_records WHERE decision_id = $1",
        )
        .bind(&d.decision_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
        if let Some(Some(_)) = v {
            persisted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        persisted,
        "shadow-fill model must be persisted for admitted decisions"
    );
    let mut shadow_count = 0i64;
    for _ in 0..200 {
        shadow_count = sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions")
            .fetch_one(&pool)
            .await
            .unwrap();
        if shadow_count >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(shadow_count, 1, "shadow trader must fork the signal");

    // with_shadow_fill_opt(Some) enables calibration; (None) leaves it off.
    let service2 = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_decision_recorder(recorder.clone())
    .with_shadow_fill_opt(
        Some(quote_client.clone()),
        Arc::new(LatencyTracker::new(10)),
    );
    let d2 = service2.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d2.admitted);
    let service3 = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    )
    .with_decision_recorder(recorder.clone())
    .with_shadow_fill_opt(None, Arc::new(LatencyTracker::new(10)));
    let d3 = service3.decide(&request(TOKEN, Action::Buy)).await;
    assert!(d3.admitted);
}

// ── SELL path ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_sell_paths() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let service = build_service(
        db.clone(),
        seeded_parser(TOKEN, "SHIELD", "100000", true),
        base_config(),
    );

    // Unknown wallet.
    let d = service.decide(&request(TOKEN, Action::Sell)).await;
    assert_eq!(d.rejection_code, Some("UNKNOWN_WALLET"));

    // Not active.
    seed_wallet(&db, WALLET, "CANDIDATE", 80.0).await;
    let d = service.decide(&request(TOKEN, Action::Sell)).await;
    assert_eq!(d.rejection_code, Some("WALLET_NOT_ACTIVE"));

    // Active but no position.
    seed_wallet(&db, WALLET, "ACTIVE", 80.0).await;
    let d = service.decide(&request(TOKEN, Action::Sell)).await;
    assert_eq!(d.rejection_code, Some("NO_ACTIVE_POSITION"));

    // Active + position → admitted SELL, capped by max_position_sol.
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ('sell-trade', $1, $2, 'SHIELD', 'BUY', 1.0, 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state) \
         VALUES ('sell-trade', $1, $2, 'SHIELD', 1.0, 1.0, 'sig', 'ACTIVE')",
    )
    .bind(WALLET)
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    let d = service.decide(&request(TOKEN, Action::Sell)).await;
    assert!(d.admitted, "{:?}", d.rejection_code);
    assert_eq!(d.strategy, Some(Strategy::Exit));
    assert_eq!(d.size_sol, Some(dec("0.5")));

    // Partial exit fraction + cap: source 20 SOL × 0.5 = 10 → capped to 5.
    let mut req = request(TOKEN, Action::Sell);
    req.source_amount_sol = dec("20.0");
    req.exit_fraction = Some(dec("0.5"));
    let d = service.decide(&req).await;
    assert!(d.admitted);
    assert_eq!(d.size_sol, Some(dec("5.0")), "capped at max_position_sol");
}

// ── Pure helpers ─────────────────────────────────────────────────────────────

#[test]
fn test_ingress_as_str() {
    assert_eq!(Ingress::Webhook.as_str(), "webhook");
    assert_eq!(Ingress::Helius.as_str(), "helius");
}

#[test]
fn test_averaging_down_public_fn() {
    assert!(!chimera_operator::engine::selection::is_averaging_down(
        &[],
        2,
        dec("3.0")
    ));
    assert!(chimera_operator::engine::selection::is_averaging_down(
        &[dec("1.0"), dec("0.9"), dec("0.8")],
        2,
        dec("3.0")
    ));
    assert!(!chimera_operator::engine::selection::is_averaging_down(
        &[dec("1.0"), dec("1.1")],
        2,
        dec("3.0")
    ));
    assert!(!chimera_operator::engine::selection::is_averaging_down(
        &[dec("0.0"), dec("0.1")],
        1,
        dec("3.0")
    ));
    assert!(!chimera_operator::engine::selection::is_averaging_down(
        &[dec("1.0"), dec("0.5")],
        2,
        dec("-1.0")
    ));
}

#[test]
fn test_config_hash_and_signal_quality() {
    let cfg = base_config();
    assert_eq!(cfg.hash().len(), 16);
    let q = SignalQuality::calculate(80.0, Some(1), dec("100000"), Some(12.0));
    assert!(q.score > 0.5);
    assert!(q.should_enter(0.55));
}
