//! Smart-Money Cluster Tests (Phase 4 — 2026-08-07)
//!
//! Validates the profitable-wallet cluster signal:
//! - peek_profitable_cluster_count counts only statistically-profitable
//!   wallets (t-stat > threshold) with BUY signals within the cluster window
//! - a single-wallet signal from an UNPROVEN wallet is admitted (bypasses the
//!   consensus-OR-proven gate) when a smart-money cluster is present
//! - without a cluster, the same signal is rejected (SINGLE_WALLET_UNPROVEN)

use chimera_operator::config::PositionSizingConfig;
use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::engine::position_sizer::PositionSizer;
use chimera_operator::engine::selection::{
    Ingress, SelectionConfig, SelectionRequest, SelectionService,
};
use chimera_operator::engine::MarketRegimeDetector;
use chimera_operator::models::Action;
use chimera_operator::monitoring::helius::HeliusClient;
use chimera_operator::monitoring::signal_aggregator::{SignalAggregator, WalletTstatConfig};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal::Decimal;

#[path = "../common/mod.rs"]
mod common;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
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

/// Insert closed shadow positions/exits for a wallet, one per pnl_pct value,
/// so get_wallet_pnl_statistics has data to classify the wallet.
async fn insert_wallet_shadow(db: &Arc<dyn Database>, wallet: &str, pnl_pcts: &[&str]) {
    let pool = pg_pool(db);
    for (i, pct) in pnl_pcts.iter().enumerate() {
        let shadow_id = format!("cluster_wallet_{}_{}", &wallet[..8], i);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at, fully_closed)
             VALUES ($1, $2, 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v', false, 0.1, 'Webhook', NOW() - make_interval(hours => $3::int), true)",
        )
        .bind(&shadow_id)
        .bind(wallet)
        .bind(i as i32 + 2)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs)
             VALUES ($1, 'mirror_main', 1.0, 150.0, $2, 0.001, 'recovery_gate', 600)",
        )
        .bind(&shadow_id)
        .bind(dec(pct))
        .execute(&pool)
        .await
        .unwrap();
    }
}

const WALLET_PREFIX: &str = "CLU5TERWALLET000000000000000000000000000000000000000000000000";

fn build_selection_service(
    db: Arc<dyn Database>,
) -> (
    SelectionService,
    Arc<SignalAggregator>,
    Arc<TokenParser>,
    Arc<PositionSizer>,
) {
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
    let signal_aggregator = Arc::new(SignalAggregator::with_tstat_config(
        db.clone(),
        WalletTstatConfig {
            threshold: 1.645,
            min_samples: 10,
            window_days: 30,
        },
    ));
    let market_regime = Arc::new(MarketRegimeDetector::new(Arc::new(
        chimera_operator::price_cache::PriceCache::new().unwrap(),
    )));
    let helius = Arc::new(HeliusClient::new("test_key".to_string(), Default::default()).unwrap());
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
        min_token_age_proven_hours: 0.1,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven: true,
        min_proven_trades: 10,
        require_proven_positive_pnl: true,
        mirror_gate_enabled: true,
        mirror_gate_min_avg_pct: dec("1.5"),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
        mirror_gate_trial_min_samples: 0,
        wallet_tstat_enabled: true,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
        shadow_proven_enabled: false,
        shadow_proven_min_samples: 20,
        shadow_proven_min_total_pnl_sol: 2.0,
        token_velocity_gate_enabled: false,
        token_min_liquidity_velocity: 0.10,
        token_max_curve_completion: 0.85,
        cluster_gate_enabled: true,
        cluster_min_profitable_wallets: 3,
        averaging_down_enabled: false,
        averaging_down_window_hours: 12,
        averaging_down_min_buys: 2,
        averaging_down_min_drop_pct: rust_decimal::Decimal::new(3, 0),
        pump_chase_enabled: false,
        pump_chase_max_delta_pct: rust_decimal::Decimal::new(10, 0),
        stop_loss_cooldown_enabled: false,
        stop_loss_cooldown_hours: 12,
        stop_loss_cooldown_loss_pct: rust_decimal::Decimal::new(5, 0),
        pump_since_whale_guard_enabled: true,
        max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
        repeat_signal_gate_enabled: true,
        repeat_signal_min_prior: 1,
        entry_drift_guard_enabled: true,
        max_entry_drift_pct: rust_decimal::Decimal::new(30, 1),
        wqs_trial_enabled: false,
        wqs_trial_min_score: 10.0,
        proven_recency_trades: 0,
        token_age_trial_enabled: false,
        token_age_trial_max_size_sol: rust_decimal::Decimal::new(25, 2),
        wallet_loss_pause_enabled: false,
        wallet_loss_pause_window_hours: 24,
        wallet_loss_pause_max_loss_sol: rust_decimal::Decimal::new(15, 2),
        momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
        momentum_bypass_enabled: false,
        wqs_proven_waiver_enabled: true,
    };
    let service = SelectionService::new(
        db,
        token_parser.clone(),
        None, // portfolio_heat
        Some(signal_aggregator.clone()),
        Some(market_regime),
        Some(helius),
        Some(position_sizer.clone()),
        config,
    );
    (service, signal_aggregator, token_parser, position_sizer)
}

/// 10 varied pnl values, mean ≈ +4.7% → t-stat ≈ 26 (deeply profitable).
const PROFITABLE_PNLS: &[&str] = &[
    "4.0", "4.5", "5.0", "4.5", "5.5", "4.0", "5.0", "4.5", "5.5", "4.0",
];
/// 10 values centered on zero, mean ≈ 0.02 → t-stat ≈ 0.3 (not profitable).
const BREAKEVEN_PNLS: &[&str] = &[
    "0.0", "0.2", "-0.1", "0.1", "0.0", "-0.2", "0.1", "0.0", "-0.1", "0.2",
];

#[tokio::test]
async fn test_profitable_cluster_counts_only_proven_wallets() {
    let (db, _tmp) = create_test_db().await;
    let w1 = format!("W1{}", &WALLET_PREFIX[..28]);
    let w2 = format!("W2{}", &WALLET_PREFIX[..28]);
    let w3 = format!("W3{}", &WALLET_PREFIX[..28]);
    for w in [&w1, &w2, &w3] {
        insert_wallet(&db, w, Some(80.0)).await;
    }
    insert_wallet_shadow(&db, &w1, PROFITABLE_PNLS).await;
    insert_wallet_shadow(&db, &w2, PROFITABLE_PNLS).await;
    insert_wallet_shadow(&db, &w3, BREAKEVEN_PNLS).await;

    let (_, aggregator, _, _) = build_selection_service(db);
    let token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    aggregator.add_signal(&w1, token, "BUY", dec("0.5")).await;
    aggregator.add_signal(&w2, token, "BUY", dec("0.5")).await;
    aggregator.add_signal(&w3, token, "BUY", dec("0.5")).await;

    let count = aggregator.peek_profitable_cluster_count(token).await;
    assert_eq!(
        count, 2,
        "only t-stat-profitable wallets count toward the cluster (breakeven wallet excluded)"
    );
}

#[tokio::test]
async fn test_cluster_gate_admits_single_signal_from_unproven_wallet() {
    let (db, _tmp) = create_test_db().await;
    let w1 = format!("C1{}", &WALLET_PREFIX[..28]);
    let w2 = format!("C2{}", &WALLET_PREFIX[..28]);
    let w3 = format!("C3{}", &WALLET_PREFIX[..28]);
    for w in [&w1, &w2, &w3] {
        insert_wallet(&db, w, Some(80.0)).await;
        insert_wallet_shadow(&db, w, PROFITABLE_PNLS).await;
    }
    let (service, aggregator, _, _) = build_selection_service(db.clone());
    // Speculative token that passes token-safety (stablecoins are rejected as
    // NON_SPECULATIVE before the gates — same token as the existing gate tests).
    let token = "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq";

    // Three profitable wallets converge on the token (cluster).
    for w in [&w1, &w2, &w3] {
        aggregator.add_signal(w, token, "BUY", dec("0.5")).await;
    }
    assert_eq!(
        aggregator.peek_profitable_cluster_count(token).await,
        3,
        "precondition: cluster of 3 profitable wallets"
    );

    // A NEW (unproven) wallet signals the same token: the cluster bypass must
    // carry it past the consensus-OR-proven gate.
    let outsider = format!("O1{}", &WALLET_PREFIX[..28]);
    insert_wallet(&db, &outsider, Some(85.0)).await;
    let req = SelectionRequest {
        wallet_address: outsider,
        token_address: token.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        source_block_time: None,
        exit_fraction: None,
        whale_entry_price: None,
    };
    let decision = service.decide(&req).await;
    assert_ne!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "smart-money cluster must bypass the consensus-OR-proven gate"
    );
}

#[tokio::test]
async fn test_cluster_gate_rejects_without_cluster() {
    let (db, _tmp) = create_test_db().await;
    let outsider = format!("O2{}", &WALLET_PREFIX[..28]);
    insert_wallet(&db, &outsider, Some(85.0)).await;

    let (service, _, _, _) = build_selection_service(db);
    let req = SelectionRequest {
        wallet_address: outsider,
        token_address: "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        source_block_time: None,
        exit_fraction: None,
        whale_entry_price: None,
    };
    let decision = service.decide(&req).await;
    assert_eq!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "without a cluster, single unproven-wallet signal stays rejected"
    );
}
