//! Selection Service Tests (B1)
//!
//! Validates the unified decision pipeline:
//! - WQS boundary: below 70 rejected (WQS_TOO_LOW); exactly 70 passes the WQS gate
//! - Both ingress paths (Webhook/Helius) produce identical decisions for the same inputs
//! - SELL with no active position is rejected
//!
//! NOTE: admitted-path behavior (70 ≤ WQS < 80 → SPEAR, WQS ≥ 80 → SHIELD, BUY
//! sizing via PositionSizer) cannot run in the default suite: the token-safety
//! and liquidity gates need live mainnet RPC data (and stablecoins are
//! deliberately rejected as non-speculative, so no whitelisted token can reach
//! the admitted path). Those flows are covered by the lib unit tests.

use chimera_operator::config::PositionSizingConfig;
use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::engine::position_sizer::PositionSizer;
use chimera_operator::engine::selection::{
    Ingress, SelectionConfig, SelectionRequest, SelectionService,
};
use chimera_operator::engine::MarketRegimeDetector;
use chimera_operator::monitoring::helius::HeliusClient;
use chimera_operator::monitoring::signal_aggregator::SignalAggregator;
use chimera_operator::models::{Action, Strategy};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal::Decimal;

#[path = "../common/mod.rs"]
mod common;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    // DbPool is PostgreSQL-only (single variant), so this is an irrefutable
    // destructure — no fallback panic arm (which would be unreachable).
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn insert_wallet(db: &Arc<dyn Database>, address: &str, wqs: Option<f64>) {
    let pool = pg_pool(db);
    let wqs_val = wqs.map(|v| dec(&format!("{:.3}", v)));
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, win_rate)
         VALUES ($1, 'ACTIVE', $2, 0.75, 0.6)
         ON CONFLICT (address) DO UPDATE SET status='ACTIVE', wqs_score=$2",
    )
    .bind(address)
    .bind(wqs_val)
    .execute(&pool)
    .await
    .unwrap();
}

/// Insert closed copy-trades for a wallet so the consensus-OR-proven gate can
/// recognize it as "proven" (the gate reads the live `trades` ledger).
async fn insert_closed_trades(
    db: &Arc<dyn Database>,
    address: &str,
    count: i32,
    net_pnl_per_trade: &str,
) {
    let pool = pg_pool(db);
    for i in 0..count {
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, token_symbol, strategy, side, amount_sol, status, net_pnl_sol)
             VALUES ($1, $2, 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v', 'TEST', 'SHIELD', 'BUY', 0.25, 'CLOSED', $3)",
        )
        .bind(format!("proven_test_{}_{}", address, i))
        .bind(address)
        .bind(dec(net_pnl_per_trade))
        .execute(&pool)
        .await
        .unwrap();
    }
}

fn build_selection_service(
    db: Arc<dyn Database>,
) -> (
    SelectionService,
    Arc<TokenParser>,
    Arc<PositionSizer>,
) {
    let DbPool::PostgreSQL(_pool) = db.pool();
    let token_parser = Arc::new({
        let config = TokenSafetyConfig::default();
        let cache = Arc::new(TokenCache::default_config());
        let fetcher = Arc::new(TokenMetadataFetcher::new(
            "https://api.mainnet-beta.solana.com",
        ));
        TokenParser::new(config, cache, fetcher)
    });
    let position_sizer = Arc::new(PositionSizer::new(
        db.clone(),
        Arc::new(PositionSizingConfig::default()),
    ));
    let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
    let market_regime = Arc::new(MarketRegimeDetector::new(Arc::new(
        chimera_operator::price_cache::PriceCache::new().unwrap(),
    )));
    let helius = Arc::new(
        HeliusClient::new("test_key".to_string(), Default::default()).unwrap(),
    );
    let config = SelectionConfig {
        total_capital_sol: dec("10.0"),
        max_position_sol: dec("5.0"),
        shield_signal_quality_threshold: 0.55,
        spear_signal_quality_threshold: 0.30,
        shield_percent: 60,
        spear_percent: 40,
        min_liquidity_shield_usd: dec("10000"),
        min_liquidity_spear_usd: dec("10000"),
        min_liquidity_pumpfun_usd: dec("25000"),
        allow_graduated_pumpfun: true,
        min_token_age_hours: 1.0,
        min_token_age_pumpfun_hours: 4.0,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven: true,
        min_proven_trades: 10,
        require_proven_positive_pnl: true,
    };
    let service = SelectionService::new(
        db,
        token_parser.clone(),
        None, // portfolio_heat
        Some(signal_aggregator),
        Some(market_regime),
        Some(helius),
        Some(position_sizer.clone()),
        config,
    );
    (service, token_parser, position_sizer)
}

const TOKEN: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC (known safe)
const WALLET_PREFIX: &str = "11111111111111111111111111111111"; // placeholder

#[tokio::test]
async fn test_wqs_below_70_buy_rejected() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("WQS6{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(65.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted, "WQS 65 must be rejected");
    assert_eq!(decision.rejection_code, Some("WQS_TOO_LOW"));
}

#[tokio::test]
async fn test_wqs_boundary_just_below_70_rejected() {
    // 69.99 is still below the 70.0 gate (no rounding can promote it).
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("WQS7{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(69.99)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted, "WQS 69.99 must be rejected");
    assert_eq!(decision.rejection_code, Some("WQS_TOO_LOW"));
}

#[tokio::test]
async fn test_wqs_exactly_70_passes_wqs_gate() {
    // WQS 70.0 exactly must pass the WQS gate (>= comparison). USDC is then
    // rejected as NON_SPECULATIVE_TOKEN — reaching that gate proves the WQS
    // boundary is inclusive at exactly 70.
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("WQS8{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(70.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted);
    assert_eq!(
        decision.rejection_code,
        Some("NON_SPECULATIVE_TOKEN"),
        "WQS 70.0 must pass the WQS gate and only fail on the token check"
    );
}

#[tokio::test]
async fn test_sell_no_position_rejected() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("SE1{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Sell,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted, "SELL with no position must be rejected");
    assert_eq!(decision.rejection_code, Some("NO_ACTIVE_POSITION"));
}

#[tokio::test]
async fn test_both_ingresses_produce_identical_rejection() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("SE2{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(40.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req_webhook = SelectionRequest {
        wallet_address: wallet.clone(),
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let req_helius = SelectionRequest {
        ingress: Ingress::Helius,
        ..req_webhook.clone()
    };

    let d1 = service.decide(&req_webhook).await;
    let d2 = service.decide(&req_helius).await;

    // Both rejected for the same reason (WQS too low).
    assert!(!d1.admitted);
    assert!(!d2.admitted);
    assert_eq!(d1.rejection_code, d2.rejection_code);
    // The only difference is the ingress field.
    assert_eq!(d1.ingress, Ingress::Webhook);
    assert_eq!(d2.ingress, Ingress::Helius);
}

// ── Consensus-OR-proven gate (profitability fix 2026-08-06) ────────────────
// The gate sits after the non-speculative/pump.fun checks and before the
// token fast-check, so the single-wallet rejection path is network-free
// (the fast-check network dependency is never reached).

#[tokio::test]
async fn test_single_wallet_unproven_buy_rejected_by_gate() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("G1{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        // Non-stablecoin, non-pump.fun address: passes the non-speculative and
        // pump.fun gates, then hits the consensus-OR-proven gate before any
        // network-dependent fast-check.
        token_address: "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted);
    assert_eq!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "single-wallet signal from an unproven wallet must be rejected by the gate"
    );
}

#[tokio::test]
async fn test_proven_wallet_single_signal_passes_gate() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("G2{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    // 10 closed copy-trades with positive 30d PnL → proven branch admits.
    insert_closed_trades(&db, &wallet, 10, "0.05").await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    // Must NOT be rejected by the consensus-OR-proven gate. The subsequent
    // token fast-check is network-dependent (may reject for other reasons),
    // so only the gate's absence is asserted here.
    assert_ne!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "proven wallet must pass the consensus-OR-proven gate"
    );
}

#[tokio::test]
async fn test_unproven_wallet_with_negative_pnl_rejected_by_gate() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("G3{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    // Enough trades, but 30d copy PnL is negative → not proven.
    insert_closed_trades(&db, &wallet, 10, "-0.05").await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert_eq!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "unproven wallet (negative 30d PnL) must be rejected by the gate"
    );
}

#[tokio::test]
async fn test_consensus_gate_bypass_allows_price_hold_confirmation() {
    // Entry confirmation re-evaluates a deferred signal with the gate
    // bypassed (the price-hold acts as the admission criterion). The decision
    // must NOT be rejected by the consensus-OR-proven gate in that mode; the
    // subsequent network-dependent fast-check may reject for other reasons.
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("G4{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };

    // Same inputs: the gate rejects when active...
    let gated = service.decide(&req).await;
    assert_eq!(
        gated.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "gate must reject without the bypass"
    );

    // ...and is skipped with the bypass (price-hold confirmation path).
    let bypassed = service.decide_with_options(&req, true).await;
    assert_ne!(
        bypassed.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "gate bypass must skip the consensus-OR-proven rejection"
    );
}
