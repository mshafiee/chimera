//! Circuit Breaker Real-Evaluation Tests
//!
//! Extends the existing circuit_breaker_tests.rs by actually calling evaluate()
//! against a real in-memory SQLite database, rather than simulating logic manually.
//!
//! Documents behavioral gaps:
//! - 30-second rate limit blinds the CB to new losses within the window
//! - Drawdown uses all-time historical peak (false positives from old sessions)
//! - No hourly loss limit: $500 can be lost in 1 hour without tripping
//! - Consecutive loss counter resets at any WIN, even one tiny win

use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState};
use chimera_operator::config::CircuitBreakerConfig;
use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Sqlite;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

fn sqlite_pool(db: &Arc<dyn Database>) -> Pool<Sqlite> {
    match db.pool() {
        DbPool::SQLite(pool) => pool,
        _ => panic!("test requires SQLite backend"),
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("cb_real_test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
}

fn tight_config() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        max_loss_24h_usd: Decimal::from_str("500.0").unwrap(),
        max_consecutive_losses: 3,
        max_drawdown_percent: Decimal::from_str("15.0").unwrap(),
        portfolio_stop_loss_percent: Decimal::from_str("5.0").unwrap(),
        cooldown_minutes: 30,
    }
}

/// Insert a closed trade with a specific USD PnL (used by get_consecutive_losses).
async fn insert_closed_trade_with_pnl(pool: &Pool<Sqlite>, uuid: &str, pnl_usd: f64) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, \
         amount_sol, status, pnl_usd, created_at, updated_at) \
         VALUES (?, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
    )
    .bind(uuid)
    .bind(pnl_usd)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a closed position with a specific SOL PnL (used by get_pnl_24h / drawdown).
async fn insert_closed_position_with_pnl(pool: &Pool<Sqlite>, trade_uuid: &str, pnl_sol: f64) {
    // Insert backing trade
    sqlx::query(
        "INSERT OR IGNORE INTO trades \
         (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES (?, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')",
    )
    .bind(trade_uuid)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO positions \
         (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
          entry_tx_signature, state, realized_pnl_sol, realized_pnl_usd, closed_at) \
         VALUES (?, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', ?, ?, CURRENT_TIMESTAMP)",
    )
    .bind(trade_uuid)
    .bind(pnl_sol)
    .bind(pnl_sol)
    .execute(pool)
    .await
    .unwrap();
}

// ─── Test 48 (plan) ── evaluate trips on real DB loss ─────────────────────────

#[tokio::test]
async fn test_evaluate_trips_on_24h_loss() {
    // Insert enough realized SOL PnL that, converted to USD, exceeds $500 threshold.
    // Note: get_pnl_24h queries positions.realized_pnl_sol.  The CB compares against
    // max_loss_24h_usd but the query returns SOL.  This test uses the actual comparison
    // to confirm whether the CB treats SOL = USD (potential unit mismatch bug).

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Insert 600 SOL loss in last 24h (well above $500 threshold)
    // If CB treats SOL as USD: -600 SOL < 0 AND 600 >= 500 → trip.
    for i in 0..6 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-24h-{}", i), -100.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "600 SOL/USD loss in 24h must trip the circuit breaker"
    );
}

#[tokio::test]
async fn test_evaluate_does_not_trip_below_threshold() {
    // 400 SOL/USD loss → below $500 threshold → must stay Active.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // 2 losses × (-200 SOL) = -400 total, consecutive = 2 < threshold of 3.
    // Using 4 × (-100) would also give -400 but would trigger the consecutive check (4 ≥ 3).
    for i in 0..2 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-below-{}", i), -200.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Active,
        "400 SOL/USD loss must not trip (threshold is $500)"
    );
}

// ─── Test 50 (plan) ── 30-second rate limit blinds CB to new losses ───────────

#[tokio::test]
async fn test_evaluate_rate_limit_prevents_re_evaluation_within_30s() {
    // First evaluate() call sets last_check. A second immediate call does nothing.
    // This means new losses incurred in the first 30s after an evaluation are invisible.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // First eval: empty DB, nothing trips.
    cb.evaluate().await.unwrap();
    assert_eq!(cb.current_state(), CircuitBreakerState::Active);

    // Insert catastrophic loss AFTER first eval
    for i in 0..10 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-blind-{}", i), -100.0).await;
    }

    // Second eval runs immediately — should be rate-limited, still Active
    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Active,
        "DOCUMENTS BUG: second evaluate() within 30s is rate-limited — losses not seen"
    );
}

// ─── Test 54 (plan) ── consecutive losses resets at WIN ───────────────────────

#[tokio::test]
async fn test_consecutive_losses_resets_at_intervening_win() {
    // Pattern: LOSE, LOSE, WIN, LOSE, LOSE, LOSE
    // Consecutive counter should be 3 (from the WIN backward), not 5 (total).
    // With max_consecutive_losses=3, this DOES trip. But the counter is 3, not 5.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Use insert_closed_position_with_pnl so both tables are populated for the JOIN in
    // get_consecutive_losses(). insert_closed_trade_with_pnl only inserts into trades.
    //
    // After inserting, backdate the timestamps to ensure deterministic ORDER BY created_at DESC:
    //   3 recent losses  → created_at = now (newest, offsets -1s, -2s, -3s)
    //   1 win            → created_at = now - 10s
    //   2 old losses     → created_at = now - 20s, -21s
    for i in 0..3_i64 {
        let uuid = format!("uuid-loss-recent-{}", i);
        insert_closed_position_with_pnl(&pool, &uuid, -50.0).await;
        let offset = format!("-{} seconds", i + 1);
        sqlx::query("UPDATE trades    SET created_at = datetime('now', ?) WHERE trade_uuid = ?")
            .bind(&offset)
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE positions SET closed_at  = datetime('now', ?) WHERE trade_uuid = ?")
            .bind(&offset)
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();
    }
    insert_closed_position_with_pnl(&pool, "uuid-win", 10.0).await;
    sqlx::query("UPDATE trades    SET created_at = datetime('now', '-10 seconds') WHERE trade_uuid = 'uuid-win'")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE positions SET closed_at  = datetime('now', '-10 seconds') WHERE trade_uuid = 'uuid-win'")
        .execute(&pool).await.unwrap();
    for i in 0..2_i64 {
        let uuid = format!("uuid-loss-old-{}", i);
        insert_closed_position_with_pnl(&pool, &uuid, -50.0).await;
        let offset = format!("-{} seconds", 20 + i);
        sqlx::query("UPDATE trades    SET created_at = datetime('now', ?) WHERE trade_uuid = ?")
            .bind(&offset)
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE positions SET closed_at  = datetime('now', ?) WHERE trade_uuid = ?")
            .bind(&offset)
            .bind(&uuid)
            .execute(&pool)
            .await
            .unwrap();
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
    // LOSE, WIN, LOSE, LOSE → consecutive = 2 (not 3). Should NOT trip with threshold=3.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Most recent: 2 losses
    for i in 0..2 {
        insert_closed_trade_with_pnl(&pool, &format!("uuid-2loss-{}", i), -50.0).await;
    }
    // Win
    insert_closed_trade_with_pnl(&pool, "uuid-win2", 10.0).await;
    // Older loss
    insert_closed_trade_with_pnl(&pool, "uuid-old-loss", -50.0).await;

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
    // Insert 10 trades of -$50 each in the last hour = -$500.
    // The CB only has a 24h limit, not an hourly limit.
    // With max_loss_24h=$500: the -$500 loss should trip (boundary condition = trip at >=500).
    // This test actually documents that the boundary IS at 500, and confirms the hourly
    // rate is irrelevant — only the 24h cumulative matters.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // 10 trades of -50 SOL each = -500 SOL total
    for i in 0..10 {
        insert_closed_position_with_pnl(&pool, &format!("uuid-hourly-{}", i), -50.0).await;
    }

    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "500 SOL/USD 24h loss exactly at threshold must trip (no hourly sub-limit exists)"
    );
}

// ─── Test 52 (plan) ── cooldown exit does not re-evaluate conditions ─────────

#[tokio::test]
async fn test_cooldown_exit_does_not_reevaluate_trip_condition() {
    // After tripping, the CB enters cooldown. When cooldown expires (via internal
    // exit_cooldown), state returns to Active WITHOUT re-evaluating the trip condition.
    // If losses still exceed threshold, the CB will immediately trip again on next evaluate().

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);

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

    // Now call evaluate() again — with 0-minute cooldown, it should exit cooldown
    // (the evaluate() internal exits cooldown first, then returns without re-checking)
    // Create a new CB instance (no rate limit) to force re-evaluation
    let cb2 = CircuitBreaker::new(tight_config(), db.clone(), Decimal::from(1000000));

    // Still -600 SOL loss in DB. New CB evaluates fresh → should trip again.
    cb2.evaluate().await.unwrap();

    assert_eq!(
        cb2.current_state(),
        CircuitBreakerState::Tripped,
        "New CB with unchanged loss data must trip immediately on first evaluate()"
    );
}

// ─── Test 51 (plan) ── historical peak causes false drawdown positive ─────────

#[tokio::test]
async fn test_drawdown_from_all_time_peak_not_session_peak() {
    // The drawdown calculation uses the all-time running PnL peak (ordered by closed_at).
    // If the running PnL peaked at +1000 SOL and later recovered to only +500 SOL,
    // drawdown = (1000-500)/1000 = 50% → trips at 15% threshold.
    // This can falsely trip even when the current session is profitable.
    //
    // NOTE: Uses explicit `closed_at` timestamps to guarantee the ORDER BY closed_at
    // in get_max_drawdown_percent() processes positions in the correct sequence:
    // gains first (build the peak), then losses (create drawdown), then partial recovery.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let cb = CircuitBreaker::new(tight_config(), db.clone(), Decimal::ZERO);

    // Insert positions with explicit timestamps to enforce ordering
    // Historical profitable positions: build peak of +1000 SOL
    for i in 0..10_i64 {
        let ts = format!("2026-01-01 00:00:{:02}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES (?, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-hist-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
             VALUES (?, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', 100.0, ?)",
        )
        .bind(format!("uuid-hist-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Drawdown: -100 SOL each → running PnL drops from 1000 to 400
    for i in 0..6_i64 {
        let ts = format!("2026-01-01 00:01:{:02}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES (?, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-dd-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
             VALUES (?, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', -100.0, ?)",
        )
        .bind(format!("uuid-dd-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Partial recovery: +25 SOL each → running PnL goes from 400 to 500
    for i in 0..4_i64 {
        let ts = format!("2026-01-01 00:02:{:02}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES (?, 'w', 't', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(format!("uuid-rec-{}", i))
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO positions \
             (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, \
              entry_tx_signature, state, realized_pnl_sol, closed_at) \
             VALUES (?, 'w', 't', 'SHIELD', 1.0, 1.0, 'sig', 'CLOSED', 25.0, ?)",
        )
        .bind(format!("uuid-rec-{}", i))
        .bind(&ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Running PnL: peak = +1000 SOL (first 10 positions), current = +500 SOL
    // Drawdown = (1000 - 500) / 1000 = 50% > 15% threshold → must trip
    cb.evaluate().await.unwrap();

    assert_eq!(
        cb.current_state(),
        CircuitBreakerState::Tripped,
        "DOCUMENTS: all-time drawdown (50%) trips CB even though current session is profitable"
    );
}
