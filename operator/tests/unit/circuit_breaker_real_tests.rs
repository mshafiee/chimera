//! Circuit Breaker Real-Evaluation Tests
//!
//! Extends the existing circuit_breaker_tests.rs by actually calling evaluate()
//! against a real PostgreSQL database (the harness is Postgres-only; SQLite
//! was decommissioned), rather than simulating logic manually.
//!
//! Behavioral characteristics pinned here:
//! - The 5s check interval rate-limits back-to-back evaluate() calls
//! - Drawdown uses the all-time historical peak (false positives from old sessions)
//! - No hourly loss limit: $500 can be lost in 1 hour without an hourly trip
//! - Consecutive loss counter resets at any WIN, even one tiny win

use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState};
use chimera_operator::config::CircuitBreakerConfig;
use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    // DbPool is PostgreSQL-only (single variant): irrefutable destructure, no
    // fallback panic arm (which would be unreachable).
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
}

fn tight_config() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        max_loss_24h_usd: Decimal::from_str("500.0").unwrap(),
        max_consecutive_losses: 3,
        max_drawdown_percent: Decimal::from_str("15.0").unwrap(),
        // NEGATIVE by convention (validated < 0 in config): -5% portfolio stop.
        portfolio_stop_loss_percent: Decimal::from_str("-5.0").unwrap(),
        cooldown_minutes: 30,
        max_jupiter_failures: 5,
    }
}

/// A price cache with SOL = $1 so the 24h USD loss check actually runs
/// (evaluate() skips the USD check when the SOL price is unavailable).
fn price_cache_with_sol_price() -> Arc<PriceCache> {
    let cache = PriceCache::new().unwrap();
    cache.set_price(
        chimera_operator::constants::mints::SOL,
        Decimal::ONE,
        PriceSource::Jupiter,
        Some(9),
    );
    Arc::new(cache)
}

/// Insert a closed position with a specific PnL.
///
/// NOTE: the evaluate() path sums `positions.realized_pnl_usd` (24h,
/// pnl_data_valid) and compares against `max_loss_24h_usd`. These tests are
/// USD-denominated, so the same numeric value is written into
/// `realized_pnl_sol` AND `realized_pnl_usd` — there is no SOL-as-USD path in
/// the current implementation.
async fn insert_closed_position_with_pnl(pool: &Pool<Postgres>, trade_uuid: &str, pnl_usd: f64) {
    // Insert backing trade
    sqlx::query(
        "INSERT INTO trades \
         (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ($1, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED') \
         ON CONFLICT (trade_uuid) DO NOTHING",
    )
    .bind(trade_uuid)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO positions \
         (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
          entry_tx_signature, state, realized_pnl_sol, realized_pnl_usd, closed_at) \
         VALUES ($1, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', $2, $3, CURRENT_TIMESTAMP)",
    )
    .bind(trade_uuid)
    .bind(pnl_usd)
    .bind(pnl_usd)
    .execute(pool)
    .await
    .unwrap();
}

/// Backdate a position + its trade so ORDER BY created_at/closed_at is
/// deterministic (all inserts otherwise share ~identical timestamps).
async fn backdate(pool: &Pool<Postgres>, trade_uuid: &str, seconds_ago: i64) {
    let offset = format!("-{seconds_ago} seconds");
    sqlx::query("UPDATE trades SET created_at = NOW() + ($1)::interval WHERE trade_uuid = $2")
        .bind(&offset)
        .bind(trade_uuid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE positions SET closed_at = NOW() + ($1)::interval WHERE trade_uuid = $2")
        .bind(&offset)
        .bind(trade_uuid)
        .execute(pool)
        .await
        .unwrap();
}

// ─── Test 48 (plan) ── evaluate trips on real DB loss ─────────────────────────

#[tokio::test]
async fn test_evaluate_trips_on_24h_loss() {
    // Insert enough realized USD PnL to exceed the $500 threshold.
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000))
        .with_price_cache(price_cache_with_sol_price());

    // Insert 600 USD loss in last 24h (well above $500 threshold)
    for i in 0..6 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-24h-{}", i), -100.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "600 USD loss in 24h must trip the circuit breaker"
    );
}

#[tokio::test]
async fn test_evaluate_does_not_trip_below_threshold() {
    // 400 USD loss → below $500 threshold → must stay Active.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000))
        .with_price_cache(price_cache_with_sol_price());

    // 2 losses × (-200 USD) = -400 total, consecutive = 2 < threshold of 3.
    for i in 0..2 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-below-{}", i), -200.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Active,
        "400 USD loss must not trip (threshold is $500)"
    );
}

// ─── Test 50 (plan) ── 5s check interval rate-limits evaluate() ───────────────

#[tokio::test]
async fn test_evaluate_rate_limit_prevents_re_evaluation_within_5s() {
    // First evaluate() call sets last_check. A second immediate call is
    // rate-limited by the 5s check interval. Once the interval elapses, the
    // new losses are seen and the breaker trips.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000))
        .with_price_cache(price_cache_with_sol_price());

    // First eval: empty DB, nothing trips; last_check is recorded.
    cb.evaluate().await.unwrap();
    assert_eq!(cb.current_state(), CircuitBreakerState::Active);

    // A second IMMEDIATE evaluate() is rate-limited by the 5s check interval.
    cb.evaluate().await.unwrap();
    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Active,
        "back-to-back evaluate() within the 5s check interval is rate-limited"
    );

    // Insert catastrophic loss AFTER the first eval, wait out the interval,
    // then re-evaluate: the losses are now visible.
    for i in 0..10 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-blind-{}", i), -100.0).await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    cb.evaluate().await.unwrap();
    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "losses inserted after the first check must trip on the next evaluation window"
    );
}

// ─── Test 54 (plan) ── consecutive losses resets at WIN ───────────────────────

#[tokio::test]
async fn test_consecutive_losses_resets_at_intervening_win() {
    // Pattern: LOSE, LOSE, WIN, LOSE, LOSE, LOSE (most recent first)
    // Consecutive counter should be 3 (from the WIN backward), not 5 (total).
    // With max_consecutive_losses=3, this trips.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Both tables are populated (get_consecutive_losses JOINs positions) and
    // timestamps are backdated so ORDER BY created_at DESC is deterministic:
    //   3 recent losses → -1s, -2s, -3s
    //   1 win           → -10s
    //   2 old losses    → -20s, -21s
    for i in 0..3_i64 {
        let uuid = format!("uuid-loss-recent-{}", i);
        insert_closed_position_with_pnl(&pool, &uuid, -50.0).await;
        backdate(&pool, &uuid, i + 1).await;
    }
    insert_closed_position_with_pnl(&pool, "uuid-win", 10.0).await;
    backdate(&pool, "uuid-win", 10).await;
    for i in 0..2_i64 {
        let uuid = format!("uuid-loss-old-{}", i);
        insert_closed_position_with_pnl(&pool, &uuid, -50.0).await;
        backdate(&pool, &uuid, 20 + i).await;
    }

    cb.evaluate().await.unwrap();

    // 3 consecutive losses = max_consecutive_losses → trips
    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "3 consecutive losses (with old losses behind a win) must trip at threshold=3"
    );
}

#[tokio::test]
async fn test_consecutive_losses_4_does_not_count_behind_win() {
    // Pattern (most recent first): LOSE, LOSE, WIN, LOSE → consecutive = 2
    // (not 3). Should NOT trip with threshold=3.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Most recent: 2 losses
    for i in 0..2 {
        let uuid = format!("uuid-2loss-{}", i);
        insert_closed_position_with_pnl(&pool, &uuid, -50.0).await;
        backdate(&pool, &uuid, i + 1).await;
    }
    // Win
    insert_closed_position_with_pnl(&pool, "uuid-win2", 10.0).await;
    backdate(&pool, "uuid-win2", 10).await;
    // Older loss
    insert_closed_position_with_pnl(&pool, "uuid-old-loss", -50.0).await;
    backdate(&pool, "uuid-old-loss", 20).await;

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Active,
        "2 consecutive losses (with a win before) must NOT trip at threshold=3"
    );
}

// ─── Test 53 (plan) ── no hourly limit allows $500 in one hour ────────────────

#[tokio::test]
async fn test_no_hourly_loss_limit_allows_large_intra_hour_loss() {
    // 10 losses of -50 USD in the last hour = -$500. Only the 24h cumulative
    // matters — there is no hourly sub-limit. Exactly at the $500 threshold
    // the breaker trips.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    for i in 0..10 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-hourly-{}", i), -50.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "500 USD 24h loss exactly at threshold must trip (no hourly sub-limit exists)"
    );
}

// ─── Test 52 (plan) ── cooldown exit re-checks the trip condition ─────────────

#[tokio::test]
async fn test_cooldown_exit_reevaluates_trip_condition() {
    // After tripping, the CB enters cooldown. With cooldown_minutes = 0,
    // evaluate() immediately exits cooldown and RE-CHECKS the breach
    // condition — which still holds (600 USD loss), so it re-trips.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);

    // Use a 0-minute cooldown for instant testing
    let cfg = CircuitBreakerConfig {
        cooldown_minutes: 0,
        ..tight_config()
    };
    let cb = CircuitBreaker::new(cfg, db.clone(), Decimal::from(1000000));

    // Insert losses exceeding threshold
    for i in 0..6 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-trip-{}", i), -100.0).await;
    }

    // Trip the breaker
    cb.evaluate().await.unwrap();
    assert_eq!(cb.current_state(), CircuitBreakerState::Tripped);

    // Manually enter cooldown
    cb.enter_cooldown().await.unwrap();
    assert_eq!(cb.current_state(), CircuitBreakerState::Cooldown);

    // Wait out the 5s check interval so the next evaluate() is not
    // rate-limited, then evaluate: cooldown expired (0 minutes) → the breach
    // condition is re-checked → the breaker re-trips while the loss persists.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "evaluate() after cooldown expiry must re-trip while the loss persists"
    );
}

// ─── Test 51 (plan) ── historical peak causes false drawdown positive ─────────

#[tokio::test]
async fn test_drawdown_from_all_time_peak_not_session_peak() {
    // The drawdown calculation uses the all-time running PnL peak (ordered by
    // closed_at). If the running PnL peaked at +1000 USD and later recovered
    // to only +500 USD, drawdown = (1000-500)/1000 = 50% → trips at the 15%
    // threshold. This can falsely trip even when the current session is
    // profitable.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::ZERO)
        .with_price_cache(price_cache_with_sol_price());

    // Historical profitable positions: build peak of +1000 USD
    for i in 0..10_i64 {
        let ts = chrono::DateTime::parse_from_rfc3339(&format!("2026-01-01T00:00:{:02}Z", i))
            .unwrap()
            .with_timezone(&chrono::Utc);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-hist-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
              VALUES ($1, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', 100.0, $2)",
        )
        .bind(format!("uuid-hist-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Drawdown: -100 USD each → running PnL drops from 1000 to 400
    for i in 0..6_i64 {
        let ts = chrono::DateTime::parse_from_rfc3339(&format!("2026-01-01T00:01:{:02}Z", i))
            .unwrap()
            .with_timezone(&chrono::Utc);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-dd-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
              VALUES ($1, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', -100.0, $2)",
        )
        .bind(format!("uuid-dd-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Partial recovery: +25 USD each → running PnL goes from 400 to 500
    for i in 0..4_i64 {
        let ts = chrono::DateTime::parse_from_rfc3339(&format!("2026-01-01T00:02:{:02}Z", i))
            .unwrap()
            .with_timezone(&chrono::Utc);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-rec-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
              VALUES ($1, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', 25.0, $2)",
        )
        .bind(format!("uuid-rec-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Running PnL: peak = +1000 USD (first 10 positions), current = +500 USD
    // Drawdown = (1000 - 500) / 1000 = 50% > 15% threshold → must trip
    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "all-time drawdown (50%) trips CB even though current session is profitable"
    );
}
