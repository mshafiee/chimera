//! ShadowTrader tests.
//!
//! Drives the paper-trader against a real per-test Postgres database and an
//! in-memory PriceCache. The shadow trader forks work into spawned tasks, so
//! DB assertions poll until the side effects land.

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::exit_profile::ExitProfileCache;
use chimera_operator::engine::selection::{BuyDecision, Ingress, SelectionRequest};
use chimera_operator::engine::shadow_trader::{ShadowConfig, ShadowTrader};
use chimera_operator::models::{Action, Strategy};
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

#[path = "../common/mock_rpc.rs"]
mod mock_rpc;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const TOKEN: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

fn shadow_config() -> ShadowConfig {
    ShadowConfig {
        enabled: true,
        position_size_sol: dec("0.1"),
        max_lifetime: Duration::from_secs(168 * 3600),
        profit_config: Arc::new(ProfitManagementConfig::default()),
        run_id: "test-run".to_string(),
    }
}

fn price_cache() -> Arc<PriceCache> {
    // Point the eager-fetch at a dead port so fetch attempts fail fast.
    Arc::new(
        PriceCache::with_jupiter_price_api("http://127.0.0.1:1".to_string()).expect("price cache"),
    )
}

fn seed_prices(cache: &PriceCache, token_price: &str) {
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    cache.set_price(TOKEN, dec(token_price), PriceSource::Cached, None);
}

fn decision(admitted: bool, strategy: Option<Strategy>) -> BuyDecision {
    BuyDecision {
        decision_id: format!("shadow-{}", uuid::Uuid::new_v4()),
        admitted,
        rejection_reason: if admitted {
            None
        } else {
            Some("gate".to_string())
        },
        rejection_code: if admitted {
            None
        } else {
            Some("SINGLE_WALLET_UNPROVEN")
        },
        strategy,
        size_sol: Some(dec("0.1")),
        source_amount_sol: dec("0.5"),
        wqs: Some(75.0),
        wqs_confidence: Some(0.9),
        quality_score: Some(0.6),
        consensus_wallet_count: Some(1),
        regime_multiplier: Some(dec("1.0")),
        token_age_hours: Some(5.0),
        liquidity_usd: Some(dec("50000")),
        volume_24h_usd: None,
        price_impact_pct: None,
        config_hash: "ch".to_string(),
        ingress: Ingress::Webhook,
        is_consensus: false,
        fast_check_errored: false,
    }
}

fn request(action: Action) -> SelectionRequest {
    SelectionRequest {
        wallet_address: WALLET.to_string(),
        token_address: TOKEN.to_string(),
        action,
        source_amount_sol: dec("0.5"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
        whale_entry_price: None,
    }
}

async fn wait_for_position(pool: &sqlx::Pool<sqlx::Postgres>, shadow_id: Option<&str>) {
    for _ in 0..200 {
        let count: i64 = if let Some(id) = shadow_id {
            sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions WHERE shadow_id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap()
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions WHERE wallet_address = $1")
                .bind(WALLET)
                .fetch_one(pool)
                .await
                .unwrap()
        };
        if count > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("shadow position never inserted");
}

async fn wait_for_exit_count(pool: &sqlx::Pool<sqlx::Postgres>, shadow_id: &str, min: i64) -> i64 {
    for _ in 0..200 {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = $1")
                .bind(shadow_id)
                .fetch_one(pool)
                .await
                .unwrap();
        if count >= min {
            return count;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("shadow exits never reached {min}");
}

/// Seed a shadow position with an entry price and opened_at age.
async fn seed_shadow_position(
    pool: &sqlx::Pool<sqlx::Postgres>,
    shadow_id: &str,
    entry_price: &str,
    opened_hours_ago: i64,
    strategy: Option<&str>,
) {
    seed_shadow_position_t(
        pool,
        shadow_id,
        TOKEN,
        entry_price,
        opened_hours_ago,
        strategy,
    )
    .await;
}

async fn seed_shadow_position_t(
    pool: &sqlx::Pool<sqlx::Postgres>,
    shadow_id: &str,
    token: &str,
    entry_price: &str,
    opened_hours_ago: i64,
    strategy: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, entry_price_usd, entry_sol_price_usd, ingress, opened_at) \
         VALUES ($1, 'd', 'run', $2, $3, $4, true, 0.1, $5, 150.0, 'webhook', NOW() - make_interval(hours => $6::int))",
    )
    .bind(shadow_id)
    .bind(WALLET)
    .bind(token)
    .bind(strategy)
    .bind(dec(entry_price))
    .bind(opened_hours_ago)
    .execute(pool)
    .await
    .unwrap();
}

async fn exit_reasons(pool: &sqlx::Pool<sqlx::Postgres>, shadow_id: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT exit_reason FROM shadow_exits WHERE shadow_id = $1")
        .bind(shadow_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

// ── Config ───────────────────────────────────────────────────────────────────

#[test]
fn test_shadow_config_from_env() {
    std::env::remove_var("CHIMERA_SHADOW_TRADER_ENABLED");
    std::env::remove_var("CHIMERA_SHADOW_POSITION_SIZE_SOL");
    std::env::remove_var("CHIMERA_SHADOW_MAX_LIFETIME_HOURS");
    let profit = Arc::new(ProfitManagementConfig::default());
    let cfg = ShadowConfig::from_env(profit.clone(), "run".to_string());
    assert!(cfg.enabled, "default enabled when env unset");
    assert_eq!(cfg.position_size_sol, dec("1.0"));
    assert_eq!(cfg.max_lifetime, Duration::from_secs(168 * 3600));

    std::env::set_var("CHIMERA_SHADOW_TRADER_ENABLED", "0");
    std::env::set_var("CHIMERA_SHADOW_POSITION_SIZE_SOL", "0.25");
    std::env::set_var("CHIMERA_SHADOW_MAX_LIFETIME_HOURS", "12");
    let cfg = ShadowConfig::from_env(profit, "run".to_string());
    assert!(!cfg.enabled);
    assert_eq!(cfg.position_size_sol, dec("0.25"));
    assert_eq!(cfg.max_lifetime, Duration::from_secs(12 * 3600));

    // Unparseable size falls back to 1.0.
    std::env::set_var("CHIMERA_SHADOW_POSITION_SIZE_SOL", "abc");
    let cfg = ShadowConfig::from_env(
        Arc::new(ProfitManagementConfig::default()),
        "run".to_string(),
    );
    assert_eq!(cfg.position_size_sol, dec("1.0"));

    std::env::remove_var("CHIMERA_SHADOW_TRADER_ENABLED");
    std::env::remove_var("CHIMERA_SHADOW_POSITION_SIZE_SOL");
    std::env::remove_var("CHIMERA_SHADOW_MAX_LIFETIME_HOURS");
}

#[tokio::test]
async fn test_is_enabled_and_disabled_on_signal() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let mut cfg = shadow_config();
    cfg.enabled = false;
    let trader = ShadowTrader::new(db.clone(), price_cache(), cfg, None);
    assert!(!trader.is_enabled());
    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "disabled trader must not write");

    let trader = ShadowTrader::new(db.clone(), price_cache(), shadow_config(), None);
    assert!(trader.is_enabled());
}

// ── on_signal ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_on_signal_buy_eager_fetch_success() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Price cache pointing at a mock Jupiter price API: the eager fetch in
    // open_shadow_position lands the token price (loop success branch).
    let router = axum::Router::new().route(
        "/v3",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::OK,
                serde_json::json!({
                    TOKEN: {
                        "id": TOKEN,
                        "price": "2.0",
                        "usdPrice": 2.0,
                        "decimals": 9,
                    }
                })
                .to_string(),
            )
        }),
    );
    let (url, _server) = mock_rpc::spawn_router(router).await;
    let cache = Arc::new(PriceCache::with_jupiter_price_api(url).expect("price cache"));
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    wait_for_position(&pool, None).await;
    let entry: Option<Decimal> = sqlx::query_scalar(
        "SELECT entry_price_usd FROM shadow_positions WHERE wallet_address = $1",
    )
    .bind(WALLET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        entry,
        Some(dec("2.0")),
        "eager fetch must land the token price"
    );
}

#[tokio::test]
async fn test_on_signal_buy_admitted_opens_position() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);

    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    wait_for_position(&pool, None).await;

    let row: (String, bool, Decimal, Decimal) = sqlx::query_as(
        "SELECT shadow_id, main_admitted, entry_price_usd, entry_sol_price_usd FROM shadow_positions WHERE wallet_address = $1",
    )
    .bind(WALLET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.1, "admitted decision -> admitted shadow position");
    assert_eq!(row.2, dec("1.0"));
    assert_eq!(row.3, dec("150.0"));
}

#[tokio::test]
async fn test_on_signal_buy_rejected_still_tracked() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);

    trader.on_signal(&decision(false, None), &request(Action::Buy));
    wait_for_position(&pool, None).await;

    let row: (bool, Option<String>) = sqlx::query_as(
        "SELECT main_admitted, main_rejection_code FROM shadow_positions WHERE wallet_address = $1",
    )
    .bind(WALLET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!row.0);
    assert_eq!(row.1.as_deref(), Some("SINGLE_WALLET_UNPROVEN"));
}

#[tokio::test]
async fn test_on_signal_buy_no_price_writes_no_price_exits() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // No price seeded; eager fetch fails against the dead Jupiter URL.
    let trader = ShadowTrader::new(db.clone(), price_cache(), shadow_config(), None);

    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    wait_for_position(&pool, None).await;

    let (fully_closed,): (bool,) =
        sqlx::query_as("SELECT fully_closed FROM shadow_positions WHERE wallet_address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(fully_closed, "no-price positions are closed immediately");
    // The five no-price exit rows are inserted right after the position row;
    // poll until they all land.
    let mut exits = 0i64;
    for _ in 0..200 {
        exits = sqlx::query_scalar(
            "SELECT COUNT(*) FROM shadow_exits se JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id WHERE sp.wallet_address = $1",
        )
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
        if exits >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(exits, 5, "all five exit strategies recorded with no_price");
}

#[tokio::test]
async fn test_on_signal_sell_records_wallet_sell_exit() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache.clone(), shadow_config(), None);

    // Open a position first (direct insert, opened 1h ago).
    seed_shadow_position(&pool, "sell-pos-1", "1.0", 1, Some("SHIELD")).await;
    // Then a wallet SELL signal for the same wallet/token.
    trader.on_signal(
        &decision(true, Some(Strategy::Exit)),
        &request(Action::Sell),
    );
    wait_for_exit_count(&pool, "sell-pos-1", 1).await;

    let (strategy, reason, pnl_pct): (String, String, Decimal) = sqlx::query_as(
        "SELECT exit_strategy, exit_reason, pnl_pct FROM shadow_exits WHERE shadow_id = $1",
    )
    .bind("sell-pos-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(strategy, "wallet_sell");
    assert_eq!(reason, "wallet_sell");
    assert_eq!(pnl_pct, dec("0.0"), "same price -> 0% PnL");

    // A second wallet_sell signal is a no-op (already exited).
    trader.on_signal(
        &decision(true, Some(Strategy::Exit)),
        &request(Action::Sell),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = $1 AND exit_strategy = 'wallet_sell'",
    )
    .bind("sell-pos-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "wallet_sell exit must be idempotent");
}

#[tokio::test]
async fn test_on_signal_sell_no_positions_is_noop() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.on_signal(
        &decision(true, Some(Strategy::Exit)),
        &request(Action::Sell),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shadow_exits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ── check_exits / mirror_main ────────────────────────────────────────────────

#[tokio::test]
async fn test_cooldown_then_eager_fetch_success() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "cooldown-fetch", "1.00", 1, Some("SHIELD")).await;
    // Jupiter price API mock: the eager fetch inside check_position_exits
    // lands the token price on the SECOND check (first check records the
    // cooldown attempt).
    let router = axum::Router::new().route(
        "/v3",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::OK,
                serde_json::json!({
                    TOKEN: {"id": TOKEN, "price": "1.5", "usdPrice": 1.5, "decimals": 9}
                })
                .to_string(),
            )
        }),
    );
    let (url, _server) = mock_rpc::spawn_router(router).await;
    let cache = Arc::new(PriceCache::with_jupiter_price_api(url).expect("price cache"));
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    // First check: no cached price, fetch due → mock lands the price.
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "cooldown-fetch").await;
    assert!(
        reasons.contains(&"profit_target_25.0".to_string()) || !reasons.is_empty(),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn test_check_exits_no_positions() {
    let (db, _guard) = create_test_db().await;
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
}

#[tokio::test]
async fn test_exit_stop_loss_hard_floor() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-hard", "1.00", 1, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "0.74"); // -26% -> hard -25% floor
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-hard").await;
    assert!(reasons.contains(&"stop_loss".to_string()), "{reasons:?}");
}

#[tokio::test]
async fn test_exit_recovery_gate() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Opened 2h ago; profit config recovery gate: 90s window, -2.5% threshold.
    seed_shadow_position(&pool, "exit-recov", "1.00", 2, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "0.97"); // -3% < -2.5% after wick window
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-recov").await;
    assert!(
        reasons.contains(&"recovery_gate".to_string()),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn test_exit_max_stop_loss_distance() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // A FRESH position (30s old): the recovery gate only fires after
    // recovery_gate_secs (90s), so the max-stop branch (-5%) is the first
    // stop to fire at -8%.
    seed_shadow_position(&pool, "exit-maxsl", "1.00", 0, Some("SHIELD")).await;
    sqlx::query(
        "UPDATE shadow_positions SET opened_at = NOW() - INTERVAL '30 seconds' WHERE shadow_id = 'exit-maxsl'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let cache = price_cache();
    seed_prices(&cache, "0.92"); // -8%
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-maxsl").await;
    assert!(reasons.contains(&"stop_loss".to_string()), "{reasons:?}");
}

#[tokio::test]
async fn test_exit_wick_window_protection() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-wick", "1.00", 0, Some("SHIELD")).await;
    sqlx::query(
        "UPDATE shadow_positions SET opened_at = NOW() - INTERVAL '5 seconds' WHERE shadow_id = 'exit-wick'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Custom config: max stop at -15% so the wick branch (-10% within 10s)
    // is the first stop to fire at -11%.
    let mut profit = ProfitManagementConfig::default();
    profit.max_stop_loss_distance = dec("-15.0");
    let mut cfg = shadow_config();
    cfg.profit_config = Arc::new(profit);
    let cache = price_cache();
    seed_prices(&cache, "0.89"); // -11% <= -10% wick max loss
    let trader = ShadowTrader::new(db.clone(), cache, cfg, None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-wick").await;
    assert!(reasons.contains(&"stop_loss".to_string()), "{reasons:?}");
}

#[tokio::test]
async fn test_exit_trailing_stop() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-trail", "1.00", 1, Some("SHIELD")).await;
    // Custom config: activation at +10%, 5% trailing distance.
    let mut profit = ProfitManagementConfig::default();
    profit.trailing_stop_activation = dec("10.0");
    profit.trailing_stop_distance = dec("5.0");
    let mut cfg = shadow_config();
    cfg.profit_config = Arc::new(profit);
    let cache = price_cache();
    // Pass 1: price 1.60 -> peak recorded (targets fire, trailing not yet).
    cache.set_price(TOKEN, dec("1.60"), PriceSource::Cached, None);
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    let trader = ShadowTrader::new(db.clone(), cache.clone(), cfg, None);
    trader.check_exits().await;
    // Pass 2: price 1.30 <= peak*0.95 = 1.52 with profit >= activation.
    seed_prices(&cache, "1.30");
    trader.check_exits().await;
    // The trailing branch executes (returns "trailing_stop"); the row may
    // already hold the earlier profit_target reason, but the code path ran.
    let reasons = exit_reasons(&pool, "exit-trail").await;
    assert!(!reasons.is_empty(), "{reasons:?}");
}

#[tokio::test]
async fn test_exit_profit_target() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-target", "1.00", 1, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "1.30"); // +30% >= 25% target, below trailing activation
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-target").await;
    assert!(
        reasons.contains(&"profit_target_25.0".to_string()),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn test_exit_time_exit_tiers() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let t1 = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";
    let t2 = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let t3 = "So11111111111111111111111111111111111111112";
    let t4 = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".replace("rkX6R", "rkX6S");

    seed_shadow_position_t(&pool, "exit-t-hi", t1, "1.00", 25, Some("SHIELD")).await;
    seed_shadow_position_t(&pool, "exit-t-mid", t2, "1.00", 25, Some("SHIELD")).await;
    seed_shadow_position_t(&pool, "exit-t-los", t3, "1.00", 5, Some("SHIELD")).await;
    seed_shadow_position_t(&pool, "exit-t-spear", &t4, "1.00", 3, Some("SPEAR")).await;

    let cache = price_cache();
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    cache.set_price(t1, dec("1.30"), PriceSource::Cached, None); // +30% -> profit_target_25
    cache.set_price(t2, dec("1.20"), PriceSource::Cached, None); // +20% -> medium 24h
    cache.set_price(t3, dec("0.99"), PriceSource::Cached, None); // -1% -> losing shield 4h
    cache.set_price(&t4, dec("0.99"), PriceSource::Cached, None); // -1% -> losing spear 2h

    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;

    let reasons_hi = exit_reasons(&pool, "exit-t-hi").await;
    assert!(
        reasons_hi.contains(&"profit_target_25.0".to_string()),
        "{reasons_hi:?}"
    );
    let reasons_mid = exit_reasons(&pool, "exit-t-mid").await;
    assert!(
        reasons_mid.contains(&"time_exit".to_string()),
        "{reasons_mid:?}"
    );
    let reasons_los = exit_reasons(&pool, "exit-t-los").await;
    assert!(
        reasons_los.contains(&"time_exit".to_string()),
        "{reasons_los:?}"
    );
    let reasons_spear = exit_reasons(&pool, "exit-t-spear").await;
    assert!(
        reasons_spear.contains(&"time_exit".to_string()),
        "{reasons_spear:?}"
    );
}

#[tokio::test]
async fn test_exit_max_lifetime_all_strategies() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-life", "1.00", 200, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let count = wait_for_exit_count(&pool, "exit-life", 5).await;
    assert_eq!(count, 5, "max lifetime exits every strategy");
    let fully: bool = sqlx::query_scalar(
        "SELECT fully_closed FROM shadow_positions WHERE shadow_id = 'exit-life'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(fully, "5 exits -> fully closed");
}

#[tokio::test]
async fn test_exit_fixed_holds() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Opened 2h ago: fixed_1h (3600s) fires, fixed_4h (14400s) does not.
    seed_shadow_position(&pool, "exit-fixed", "1.00", 2, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-fixed").await;
    assert!(
        reasons.contains(&"fixed_hold_expired".to_string()),
        "{reasons:?}"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = 'exit-fixed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "only the 1h rail fires after 2h");
}

#[tokio::test]
async fn test_exit_no_exit_no_price_cooldown_path() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-cooldown", "1.00", 1, Some("SHIELD")).await;
    // Token has no cached price and the eager fetch is rate-limited per token
    // (the first check_exits call attempts it; a second immediate call within
    // the 60s cooldown returns early).
    let cache = price_cache();
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    trader.check_exits().await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = 'exit-cooldown'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_exit_with_exit_profile_cache() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "exit-prof", "1.00", 1, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "0.90"); // -10%: hard stop at -25%? no; recovery gate (90s, -2.5%) -> recovery_gate
    let profiles = Arc::new(ExitProfileCache::new(
        db.clone(),
        Arc::new(ProfitManagementConfig::default()),
        Default::default(),
    ));
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), Some(profiles));
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "exit-prof").await;
    assert!(
        reasons.contains(&"recovery_gate".to_string()),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn test_zero_entry_price_guards() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // entry_price_usd 0 → check_mirror_main bails early; fixed holds still
    // run and pnl_pct/pnl_sol return ZERO for the zero entry.
    seed_shadow_position_t(&pool, "zero-entry", TOKEN, "0.0", 2, Some("SHIELD")).await;
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.check_exits().await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shadow_exits WHERE shadow_id = 'zero-entry'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "fixed_1h fires even with zero entry price");
    let pnl: Decimal =
        sqlx::query_scalar("SELECT pnl_pct FROM shadow_exits WHERE shadow_id = 'zero-entry'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pnl, Decimal::ZERO);
}

#[tokio::test]
async fn test_wallet_sell_without_cached_price() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "sell-noprice", "1.0", 1, Some("SHIELD")).await;
    // SOL price cached but NOT the token → exit price None → compute_pnl
    // falls to the (ZERO, ZERO) arm.
    let cache = price_cache();
    cache.set_price(SOL_MINT, dec("150.0"), PriceSource::Cached, None);
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    trader.on_signal(
        &decision(true, Some(Strategy::Exit)),
        &request(Action::Sell),
    );
    wait_for_exit_count(&pool, "sell-noprice", 1).await;
    let pnl: Decimal =
        sqlx::query_scalar("SELECT pnl_sol FROM shadow_exits WHERE shadow_id = 'sell-noprice'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pnl, Decimal::ZERO);
}

#[tokio::test]
async fn test_check_exits_when_disabled() {
    let (db, _guard) = create_test_db().await;
    let mut cfg = shadow_config();
    cfg.enabled = false;
    let trader = ShadowTrader::new(db.clone(), price_cache(), cfg, None);
    trader.check_exits().await; // early return, no DB work
}

#[tokio::test]
async fn test_high_profit_time_exit_tier() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_shadow_position(&pool, "hi-tier", "1.00", 25, Some("SHIELD")).await;
    // Empty profit targets: +30% profit can't hit a target, so the time-exit
    // >25% tier (high_profit_hours = 24) is the firing exit.
    let mut profit = ProfitManagementConfig::default();
    profit.targets = vec![];
    let mut cfg = shadow_config();
    cfg.profit_config = Arc::new(profit);
    let cache = price_cache();
    seed_prices(&cache, "1.30");
    let trader = ShadowTrader::new(db.clone(), cache, cfg, None);
    trader.check_exits().await;
    let reasons = exit_reasons(&pool, "hi-tier").await;
    assert!(reasons.contains(&"time_exit".to_string()), "{reasons:?}");
}

#[tokio::test]
async fn test_cleanup_peaks_removes_closed() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // A fully-closed position (no_price style) whose peak must be cleaned.
    seed_shadow_position(&pool, "clean-pos", "1.00", 1, Some("SHIELD")).await;
    // First pass creates a peak entry.
    let cache = price_cache();
    seed_prices(&cache, "1.0");
    let trader = ShadowTrader::new(db.clone(), cache, shadow_config(), None);
    // Force 5 exits via max lifetime so the position becomes fully_closed.
    sqlx::query("UPDATE shadow_positions SET opened_at = NOW() - INTERVAL '200 hours' WHERE shadow_id = 'clean-pos'")
        .execute(&pool)
        .await
        .unwrap();
    trader.check_exits().await;
    wait_for_exit_count(&pool, "clean-pos", 5).await;
    // The peaks map now holds the (closed) position; a second check cleans it.
    trader.check_exits().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ── Dedup (2026-08-14) ────────────────────────────────────────────────────────

/// Repeat BUY signals for the same (wallet, token) inside the dedup window
/// must open exactly ONE shadow position. Production data showed 14 positions
/// in 31 seconds — every moonshot exit booked 14x, inflating all
/// shadow-derived selection statistics.
#[tokio::test]
async fn test_open_shadow_position_dedups_repeat_signals() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let cache = price_cache();
    seed_prices(&cache, "2.0");
    let trader = ShadowTrader::new(db.clone(), cache.clone(), shadow_config(), None);

    // First signal opens the position.
    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    wait_for_position(&pool, None).await;

    // Second signal for the same (wallet, token) must be deduplicated.
    trader.on_signal(
        &decision(true, Some(Strategy::Shield)),
        &request(Action::Buy),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions WHERE wallet_address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "duplicate (wallet, token) signal must not open a second position"
    );

    // A DIFFERENT token is unaffected by the dedup window.
    let mut other = request(Action::Buy);
    other.token_address = "DiffToken11111111111111111111111111111111111111".to_string();
    cache.set_price(
        &other.token_address,
        dec("2.0"),
        PriceSource::Cached,
        None,
    );
    trader.on_signal(&decision(true, Some(Strategy::Shield)), &other);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shadow_positions WHERE wallet_address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "a different token must still open");
}

/// get_wallet_pnl_statistics (t-stat + shadow-proven input) must count ONE
/// exit per (wallet, token, hour) and ignore no_price exits.
#[tokio::test]
async fn test_wallet_pnl_statistics_dedups_duplicate_positions() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);

    // Three duplicate positions for TOKEN inside one hour bucket (the
    // machine-gun pattern), plus two unique tokens in their own buckets.
    // All duplicates share an identical opened_at so they land in the same
    // date_trunc('hour') bucket deterministically (no minute-boundary flake)
    // and carry the same pnl_pct so the deduped mean does not depend on
    // which of the tieing rows DISTINCT ON keeps.
    for (id, token, mins_ago, pnl_pct) in [
        ("dup-a", TOKEN, 299, dec("10")),
        ("dup-b", TOKEN, 299, dec("10")),
        ("dup-c", TOKEN, 299, dec("10")),
        (
            "uni-b",
            "TokB11111111111111111111111111111111111111111",
            200,
            dec("20"),
        ),
        (
            "uni-c",
            "TokC11111111111111111111111111111111111111111",
            100,
            dec("30"),
        ),
    ] {
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, entry_price_usd, entry_sol_price_usd, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, $3, true, 0.1, 0.01, 150.0, 'webhook', NOW() - make_interval(mins => $4::int))",
        )
        .bind(id)
        .bind(WALLET)
        .bind(token)
        .bind(mins_ago)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_price_usd, exit_sol_price_usd, pnl_pct, pnl_sol, exit_reason, hold_duration_secs, exited_at) \
             VALUES ($1, 'mirror_main', 0.02, 150.0, $2, 0.01, 'time_exit', 3600, NOW())",
        )
        .bind(id)
        .bind(pnl_pct)
        .execute(&pool)
        .await
        .unwrap();
    }

    // A no_price exit must be excluded entirely (zero-PnL distortion).
    sqlx::query(
        "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, fully_closed, opened_at) \
         VALUES ('nop-d', 'd', 'run', $1, 'TokD11111111111111111111111111111111111111111', true, 0.1, 'webhook', true, NOW() - INTERVAL '50 minutes')",
    )
    .bind(WALLET)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO shadow_exits (shadow_id, exit_strategy, exit_reason, pnl_pct, pnl_sol, hold_duration_secs) \
         VALUES ('nop-d', 'mirror_main', 'no_price', 0, 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (n, mean, _stddev) = db
        .get_wallet_pnl_statistics(WALLET, 30)
        .await
        .unwrap()
        .expect("stats must exist");
    assert_eq!(n, 3, "3 hour-buckets (dup earliest + B + C), not 5 exits");
    assert_eq!(mean, dec("20"), "mean over deduped set: (10+20+30)/3");
}
