//! Dune Bootstrap Tests (2026-08-07)
//!
//! Validates the `bootstrap_dune` evidence path end to end at the DB/gate
//! level:
//! - `get_wallet_pnl_statistics` unions `mirror_main` + `dune_wallet` rows
//!   (bootstrap evidence counts toward the wallet t-stat / cluster gates)
//! - the token mirror gate reads ONLY `mirror_main` — `dune_wallet` rows can
//!   never count as mirror-gate evidence (A/B hygiene)
//! - a wallet whose ONLY evidence is profitable `dune_wallet` bootstrap rows
//!   passes the t-stat gate and counts toward the smart-money cluster

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

/// Insert bootstrap-style shadow rows (`exit_strategy='dune_wallet'`,
/// `shadow_id` prefix `dune_`, `exit_reason='dune_bootstrap'`) — exactly what
/// the `bootstrap_dune` binary writes.
async fn insert_wallet_dune_shadow(db: &Arc<dyn Database>, wallet: &str, pnl_pcts: &[&str]) {
    let pool = pg_pool(db);
    for (i, pct) in pnl_pcts.iter().enumerate() {
        let shadow_id = format!("dune_{}_{}", &wallet[..8], i);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at, fully_closed)
             VALUES ($1, $2, 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v', false, 0.1, 'Dune', NOW() - INTERVAL '1 hour', true)",
        )
        .bind(&shadow_id)
        .bind(wallet)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol, exit_reason, hold_duration_secs, exited_at)
             VALUES ($1, 'dune_wallet', $2, $3, 'dune_bootstrap', 600, NOW() - INTERVAL '1 hour')",
        )
        .bind(&shadow_id)
        .bind(dec(pct))
        .bind(dec(pct) / dec("100") * dec("0.1"))
        .execute(&pool)
        .await
        .unwrap();
    }
}

/// Insert `mirror_main` shadow rows for a wallet (the shadow-trader path).
async fn insert_wallet_mirror_shadow(db: &Arc<dyn Database>, wallet: &str, pnl_pcts: &[&str]) {
    let pool = pg_pool(db);
    for (i, pct) in pnl_pcts.iter().enumerate() {
        let shadow_id = format!("mirror_{}_{}", &wallet[..8], i);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at, fully_closed)
             VALUES ($1, $2, 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v', false, 0.1, 'Webhook', NOW() - INTERVAL '1 hour', true)",
        )
        .bind(&shadow_id)
        .bind(wallet)
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

/// Insert `dune_wallet` rows for a TOKEN (mirror-gate hygiene test).
async fn insert_token_dune_shadow(db: &Arc<dyn Database>, token: &str, pnl_pcts: &[&str]) {
    let pool = pg_pool(db);
    for (i, pct) in pnl_pcts.iter().enumerate() {
        let shadow_id = format!("dune_token_{}_{}", &token[..8], i);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at, fully_closed)
             VALUES ($1, '11111111111111111111111111111111', $2, false, 0.1, 'Dune', NOW() - INTERVAL '1 hour', true)",
        )
        .bind(&shadow_id)
        .bind(token)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol, exit_reason, hold_duration_secs, exited_at)
             VALUES ($1, 'dune_wallet', $2, $3, 'dune_bootstrap', 600, NOW() - INTERVAL '1 hour')",
        )
        .bind(&shadow_id)
        .bind(dec(pct))
        .bind(dec(pct) / dec("100") * dec("0.1"))
        .execute(&pool)
        .await
        .unwrap();
    }
}

const WALLET_PREFIX: &str = "DUNEBOOTSTRAPWALLET000000000000000000000000000000000000000000000";

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
        wallet_tstat_enabled: true,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
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
        momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
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

#[tokio::test]
async fn test_wallet_pnl_statistics_unions_mirror_and_dune() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("U1{}", &WALLET_PREFIX[..28]);
    // 10 mirror_main rows at +5.0% and 10 dune_wallet rows at +7.0%:
    // the union must report n=20, mean=+6.0%.
    insert_wallet_mirror_shadow(&db, &wallet, &["5.0"; 10]).await;
    insert_wallet_dune_shadow(&db, &wallet, &["7.0"; 10]).await;

    let stats = db
        .get_wallet_pnl_statistics(&wallet, 30)
        .await
        .expect("wallet must have statistics")
        .expect("some(n)");
    let (n, mean, _stddev) = stats;
    assert_eq!(n, 20, "mirror_main + dune_wallet rows must be combined");
    assert!(
        (mean - dec("6.0")).abs() < dec("0.0001"),
        "union mean must average both strategies, got {mean}"
    );
}

#[tokio::test]
async fn test_mirror_gate_ignores_dune_wallet_rows() {
    let (db, _tmp) = create_test_db().await;
    let token = "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq";
    // 10 strongly-NEGATIVE dune_wallet rows for the token. If the mirror gate
    // counted them, it would report a negative average and reject the token;
    // the gate must instead report no mirror evidence at all.
    insert_token_dune_shadow(&db, token, &["-10.0"; 10]).await;

    let avg = db
        .get_token_mirror_avg_pnl(token, 48, 10)
        .await
        .expect("mirror query succeeds");
    assert!(
        avg.is_none(),
        "dune_wallet rows must never count as mirror-gate evidence"
    );

    // End-to-end: a proven wallet buying this token must NOT be rejected as
    // SHADOW_MIRROR_NEGATIVE — with no mirror_main evidence it is
    // SHADOW_MIRROR_INSUFFICIENT (or a later network-dependent gate).
    let wallet = format!("M1{}", &WALLET_PREFIX[..28]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    insert_wallet_mirror_shadow(&db, &wallet, &["5.0"; 10]).await; // proven wallet
    let (service, _, _, _) = build_selection_service(db);
    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: token.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
        whale_entry_price: None,
    };
    let decision = service.decide(&req).await;
    assert_ne!(
        decision.rejection_code,
        Some("SHADOW_MIRROR_NEGATIVE"),
        "dune_wallet rows must not drive the mirror-gate rejection"
    );
}

#[tokio::test]
async fn test_dune_only_wallet_passes_tstat_and_counts_toward_cluster() {
    let (db, _tmp) = create_test_db().await;
    // Three wallets whose ONLY evidence is profitable dune_wallet bootstrap
    // rows (the cold-start scenario the bootstrap exists for).
    let w1 = format!("D1{}", &WALLET_PREFIX[..28]);
    let w2 = format!("D2{}", &WALLET_PREFIX[..28]);
    let w3 = format!("D3{}", &WALLET_PREFIX[..28]);
    for w in [&w1, &w2, &w3] {
        insert_wallet(&db, w, Some(80.0)).await;
        insert_wallet_dune_shadow(&db, w, PROFITABLE_PNLS).await;
    }
    let (service, aggregator, _, _) = build_selection_service(db.clone());
    let token = "ZEUS1aR7aX8DFFJf5QjWj2ftDDdNTroMNGo8YoQm3Gq";

    // Cluster counting must include dune-only wallets (t-stat ≈ 26 each).
    for w in [&w1, &w2, &w3] {
        aggregator.add_signal(w, token, "BUY", dec("0.5")).await;
    }
    assert_eq!(
        aggregator.peek_profitable_cluster_count(token).await,
        3,
        "dune_wallet-only wallets must count toward the smart-money cluster"
    );

    // A NEW (unproven) wallet signals the same token: the dune-only cluster
    // must bypass the consensus-OR-proven gate for it.
    let outsider = format!("D9{}", &WALLET_PREFIX[..28]);
    insert_wallet(&db, &outsider, Some(85.0)).await;
    let req = SelectionRequest {
        wallet_address: outsider,
        token_address: token.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
        whale_entry_price: None,
    };
    let decision = service.decide(&req).await;
    assert_ne!(
        decision.rejection_code,
        Some("SINGLE_WALLET_UNPROVEN"),
        "dune-only smart-money cluster must bypass the consensus-OR-proven gate"
    );
}
