//! Chaos/resilience tests for Chimera Operator
//!
//! Tests system behavior under failure conditions:
//! - RPC failure and fallback mode handling
//! - Database lock scenarios (advisory-lock serialization)
//! - Circuit breaker behavior
//! - Queue overflow handling
//! - Stuck-position detection
//!
//! Every test is deterministic and hermetic (no live network calls). Tests
//! that previously invoked `Executor::execute` against live mainnet RPCs or
//! merely mutated local structs were removed — they provided false confidence.

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{Database, InsertTrade};
use chimera_operator::engine::executor::{Executor, RpcMode};
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

#[path = "common/mod.rs"]
mod common;

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

/// Build a deterministic config (never falls back to the ambient repo
/// config file, whose contents may differ between environments).
fn create_test_config(jito_enabled: bool, trade_mode: &str) -> Arc<AppConfig> {
    use config::Config;
    let config_builder = Config::builder()
        .set_default("server.host", "0.0.0.0")
        .unwrap()
        .set_default("server.port", 8080)
        .unwrap()
        .set_default("database.max_connections", 1)
        .unwrap()
        .set_default("rpc.primary_provider", "helius")
        .unwrap()
        .set_default("rpc.primary_url", "https://api.mainnet-beta.solana.com")
        .unwrap()
        .set_default("rpc.fallback_url", "https://api.mainnet-beta.solana.com")
        .unwrap()
        .set_default("rpc.rate_limit_per_second", 40)
        .unwrap()
        .set_default("rpc.timeout_ms", 2000)
        .unwrap()
        .set_default("rpc.max_consecutive_failures", 3)
        .unwrap()
        .set_default("jito.enabled", jito_enabled)
        .unwrap()
        .set_default("jito.tip_floor_sol", 0.001)
        .unwrap()
        .set_default("jito.tip_ceiling_sol", 0.01)
        .unwrap()
        .set_default("jito.tip_percentile", 50)
        .unwrap()
        .set_default("jito.tip_percent_max", 0.10)
        .unwrap()
        .set_default("strategy.shield_percent", 70)
        .unwrap()
        .set_default("strategy.spear_percent", 30)
        .unwrap()
        .set_default("strategy.max_position_sol", 1.0)
        .unwrap()
        .set_default("strategy.min_position_sol", 0.01)
        .unwrap()
        .set_default("queue.capacity", 1000)
        .unwrap()
        .set_default("queue.load_shed_threshold_percent", 80)
        .unwrap()
        .set_default("security.max_timestamp_drift_secs", 60)
        .unwrap()
        .set_default("security.webhook_rate_limit", 100)
        .unwrap()
        .set_default("security.webhook_burst_size", 150)
        .unwrap()
        .set_default("circuit_breakers.max_loss_24h_usd", 500.0)
        .unwrap()
        .set_default("circuit_breakers.max_consecutive_losses", 5)
        .unwrap()
        .set_default("circuit_breakers.max_drawdown_percent", 15.0)
        .unwrap()
        .set_default("circuit_breakers.cooldown_minutes", 30)
        .unwrap()
        .set_default("trade_mode", trade_mode)
        .unwrap()
        .build()
        .unwrap();

    Arc::new(config_builder.try_deserialize::<AppConfig>().unwrap())
}

fn shield_signal(trade_uuid: &str, token: &str) -> Signal {
    let payload = SignalPayload {
        strategy: Strategy::Shield,
        token: token.to_string(),
        token_address: Some(token.to_string()),
        action: Action::Buy,
        amount_sol: Decimal::from_str("0.5").unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: Some(trade_uuid.to_string()),
        exit_fraction: None,
    };
    Signal::new(payload, 1_700_000_000, None)
}

#[tokio::test]
async fn test_rpc_mode_initialization() {
    // With jito enabled the executor must start in Jito (primary) mode and
    // not in fallback. The fallback TRANSITION (after N consecutive RPC
    // failures) requires injecting real RPC failures, which needs a mock
    // RPC provider — a test hook that does not exist yet, so the
    // transition itself is not exercised here.
    let (db, _guard) = create_test_db().await;
    let config = create_test_config(true, "paper");

    let executor = Executor::new(config, db);

    assert_eq!(executor.rpc_mode(), RpcMode::Jito);
    assert!(!executor.is_in_fallback());

    // Jito-disabled config must start in Standard mode.
    let (db, _guard) = create_test_db().await;
    let config = create_test_config(false, "paper");
    let executor = Executor::new(config, db);
    assert_eq!(executor.rpc_mode(), RpcMode::Standard);
}

#[tokio::test]
async fn test_spear_disabled_in_fallback() {
    // Deterministic and hermetic: with Jito disabled the executor is in
    // Standard mode, and with trade_mode=live the Spear gate fires BEFORE
    // any RPC call, so no network access is required.
    let (db, _guard) = create_test_db().await;
    let config = create_test_config(false, "live");

    let executor = Executor::new(config, db);
    assert_eq!(executor.rpc_mode(), RpcMode::Standard);

    let signal = shield_signal(
        "uuid-spear-rejected",
        "SPEAR111111111111111111111111111111111111111",
    );
    let signal = Signal {
        payload: SignalPayload {
            strategy: Strategy::Spear,
            ..signal.payload
        },
        ..signal
    };

    let result = executor.execute(&signal).await;
    let err = result.expect_err("Spear must be rejected in Standard (live) mode");
    let error_str = format!("{}", err);
    assert!(
        error_str.contains("Spear") || error_str.contains("disabled"),
        "Error should indicate Spear is disabled, got: {error_str}"
    );
}

#[tokio::test]
async fn test_circuit_breaker_trip() {
    use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState};

    let (db, _guard) = create_test_db().await;
    let config = create_test_config(true, "paper");

    let breaker = CircuitBreaker::new(
        config.circuit_breakers.clone(),
        db.clone(),
        config.position_sizing.total_capital_sol,
    );

    // Starts in Active (un-tripped) state
    assert!(
        breaker.is_trading_allowed(),
        "Circuit breaker must start un-tripped"
    );
    assert_eq!(breaker.current_state(), CircuitBreakerState::Active);

    // Trip manually to simulate a threshold breach (unit-testing evaluate() would
    // require inserting many DB loss records; manual_trip covers the state transition)
    breaker
        .manual_trip(
            "test-admin",
            "consecutive losses exceeded threshold".to_string(),
        )
        .await
        .unwrap();

    assert!(
        !breaker.is_trading_allowed(),
        "Circuit breaker must block trading after trip"
    );
    assert_ne!(breaker.current_state(), CircuitBreakerState::Active);
}

#[tokio::test]
async fn test_queue_load_shedding() {
    use chimera_operator::PriorityQueue;

    let capacity = 100usize;
    let shed_threshold = 80u32; // percent
    let queue = PriorityQueue::new(capacity, shed_threshold);

    // Fill past the 80% threshold using Shield signals (they are not shed)
    let fill_to = (capacity * shed_threshold as usize) / 100 + 1;
    for i in 0..fill_to {
        let signal = shield_signal(
            &format!("uuid-fill-{}", i),
            &format!("TOK{}111111111111111111111111111111111111111", i),
        );
        let _ = queue.push(signal, None).await;
    }

    // A Spear signal submitted while queue > 80% must be shed (Err returned)
    let spear_payload = SignalPayload {
        strategy: Strategy::Spear,
        token: "SPEAR".to_string(),
        token_address: Some("SPEAR111111111111111111111111111111111111111".to_string()),
        action: Action::Buy,
        amount_sol: Decimal::from_str("0.5").unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: Some("uuid-spear-shed".to_string()),
        exit_fraction: None,
    };
    let spear_signal = Signal::new(spear_payload, 1_700_001_000_i64, None);

    let result = queue.push(spear_signal, Some(50.0)).await;
    assert!(
        result.is_err(),
        "Spear signal must be shed when queue > 80%"
    );
}

#[tokio::test]
async fn test_concurrent_writes() {
    // Test concurrent database writes don't deadlock (PostgreSQL)
    let (db, _guard) = create_test_db().await;
    let pool = common::pg_pool(&db);

    // Create table for concurrent writes
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS test_concurrent (
            id SERIAL PRIMARY KEY,
            value TEXT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Spawn multiple concurrent write tasks
    let mut handles = vec![];
    for i in 0..10 {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            for j in 0..10 {
                sqlx::query("INSERT INTO test_concurrent (value) VALUES ($1)")
                    .bind(format!("task-{}-write-{}", i, j))
                    .execute(&pool_clone)
                    .await
                    .unwrap();
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all writes succeeded
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM test_concurrent")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(count.0, 100, "All concurrent writes should succeed");

    sqlx::query("DROP TABLE IF EXISTS test_concurrent")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_database_advisory_lock_serializes_opens() {
    // The real per-key advisory lock (pg_advisory_xact_lock in
    // activate_trade_and_open_position) must serialize concurrent position
    // opens for the same token: exactly one may win, the other must be
    // rejected by the duplicate-position check.
    let (db, _guard) = create_test_db().await;
    let token = "LOCKTOKEN111111111111111111111111111111111111";
    let wallet = "lock-wallet-address-000000000000000000000000";

    for uuid in ["lock-open-1", "lock-open-2"] {
        db.insert_trade(&InsertTrade {
            trade_uuid: uuid.to_string(),
            wallet_address: wallet.to_string(),
            token_address: token.to_string(),
            token_symbol: Some("LOCK".to_string()),
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: Decimal::from_str("1.0").unwrap(),
            status: "PENDING".to_string(),
        })
        .await
        .unwrap();
    }

    let db1 = db.clone();
    let db2 = db.clone();
    let (r1, r2) = tokio::join!(
        db1.activate_trade_and_open_position(
            "lock-open-1",
            wallet,
            token,
            None,
            "SHIELD",
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("10.0").unwrap(),
            "sig-1",
            None,
            None,
        ),
        db2.activate_trade_and_open_position(
            "lock-open-2",
            wallet,
            token,
            None,
            "SHIELD",
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("10.0").unwrap(),
            "sig-2",
            None,
            None,
        ),
    );

    let wins = [r1.is_ok(), r2.is_ok()].iter().filter(|ok| **ok).count();
    assert_eq!(
        wins, 1,
        "exactly one concurrent open must win the advisory lock"
    );

    let active = db.get_active_positions().await.unwrap();
    assert_eq!(active.len(), 1, "exactly one ACTIVE position may exist");
}

#[tokio::test]
async fn test_database_lock_non_lock_error() {
    // Test that non-lock errors (like syntax errors) fail immediately
    let (db, _guard) = create_test_db().await;
    let pool = common::pg_pool(&db);

    // Invalid SQL should fail immediately, not retry
    let result = sqlx::query("INVALID SQL SYNTAX").execute(&pool).await;

    assert!(result.is_err(), "Invalid SQL should fail immediately");
}

#[tokio::test]
async fn test_vacuum_operation() {
    // Test that VACUUM operations don't block other queries (PostgreSQL
    // MVCC: plain VACUUM never blocks readers/writers)
    let (db, _guard) = create_test_db().await;
    let pool = common::pg_pool(&db);

    // Create table and insert data
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS test_vacuum (
            id SERIAL PRIMARY KEY,
            data TEXT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Insert some data
    for i in 0..100 {
        sqlx::query("INSERT INTO test_vacuum (data) VALUES ($1)")
            .bind(format!("data-{}", i))
            .execute(&pool)
            .await
            .unwrap();
    }

    // Run VACUUM in background (no binds -> simple query protocol, so it
    // runs in autocommit, outside any transaction)
    let pool_vacuum = pool.clone();
    let vacuum_handle = tokio::spawn(async move {
        sqlx::query("VACUUM test_vacuum")
            .execute(&pool_vacuum)
            .await
    });

    // While VACUUM is running, try to read
    let pool_read = pool.clone();
    let read_handle = tokio::spawn(async move {
        // MVCC guarantees readers are never blocked by VACUUM
        sqlx::query_as::<_, (i32, String)>("SELECT id, data FROM test_vacuum LIMIT 10")
            .fetch_all(&pool_read)
            .await
    });

    // Both should complete
    let read_result = read_handle.await.unwrap();
    assert!(
        read_result.is_ok(),
        "Reads must work during VACUUM (PostgreSQL MVCC)"
    );

    // The VACUUM must actually succeed — a silently ignored failure would
    // let this test pass for the wrong reason.
    tokio::time::timeout(std::time::Duration::from_secs(10), vacuum_handle)
        .await
        .expect("VACUUM must finish within 10s")
        .expect("VACUUM task must not panic")
        .expect("VACUUM must succeed (simple query protocol outside a transaction)");

    sqlx::query("DROP TABLE IF EXISTS test_vacuum")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_stuck_position_recovery() {
    // Validates that get_stuck_positions() correctly identifies EXITING positions
    // older than the threshold. The full recovery path requires an RPC call to
    // verify on-chain state, so we test the detection layer here.

    let (db, _guard) = create_test_db().await;
    let pool = common::pg_pool(&db);

    // Seed two EXITING positions through the real schema.
    for (uuid, sig) in [
        ("fresh-exiting", "sig_fresh_entry"),
        ("stuck-exiting", "sig_stuck_entry"),
    ] {
        db.insert_trade(&InsertTrade {
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet1".to_string(),
            token_address: format!("TOK{uuid}"),
            token_symbol: Some("TST".to_string()),
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: Decimal::ONE,
            status: "EXITING".to_string(),
        })
        .await
        .unwrap();
        db.insert_position(&chimera_operator::db_abstraction::types::InsertPosition {
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet1".to_string(),
            token_address: format!("TOK{uuid}"),
            token_symbol: Some("TST".to_string()),
            strategy: "SHIELD".to_string(),
            entry_amount_sol: Decimal::ONE,
            entry_price: Decimal::from(10),
            entry_tx_signature: sig.to_string(),
        })
        .await
        .unwrap();
        db.update_position(&chimera_operator::db_abstraction::types::UpdatePosition {
            trade_uuid: uuid.to_string(),
            current_price: Some(Decimal::from(20)),
            unrealized_pnl_sol: None,
            unrealized_pnl_percent: None,
            state: Some("EXITING".to_string()),
            exit_price: Some(Decimal::from(20)),
            exit_tx_signature: Some(format!("sig_exit_{uuid}")),
            realized_pnl_sol: None,
            realized_pnl_usd: None,
        })
        .await
        .unwrap();
    }

    // Backdate last_updated: the positions_updated_at trigger force-resets it on
    // UPDATE, so disable it around the timestamp write (then re-enable).
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("ALTER TABLE positions DISABLE TRIGGER positions_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE positions SET last_updated = $1 WHERE trade_uuid = $2")
        .bind(chrono::Utc::now() - chrono::Duration::seconds(300))
        .bind("stuck-exiting")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE positions ENABLE TRIGGER positions_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // get_stuck_positions uses a 60-second threshold by default
    let stuck = db.get_stuck_positions(60).await.unwrap();

    assert_eq!(
        stuck.len(),
        1,
        "Exactly 1 stuck position expected (300s > 60s threshold); got {}",
        stuck.len()
    );
    assert_eq!(stuck[0].trade_uuid, "stuck-exiting");
}

#[tokio::test]
async fn test_concurrent_webhook_processing() {
    // Insert 100 unique trade rows concurrently and verify no duplicates or deadlocks.
    let (db, _guard) = create_test_db().await;
    let pool = common::pg_pool(&db);

    let n: usize = 100;
    let mut handles = vec![];

    for i in 0..n {
        let pool_clone = pool.clone();
        handles.push(tokio::spawn(async move {
            // Real schema requires wallet_address/token_address (NOT NULL).
            sqlx::query(
                "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
                 VALUES ($1, 'wallet1', 'token1', 'SHIELD', 'BUY', 1.0, 'PENDING') \
                 ON CONFLICT (trade_uuid) DO NOTHING",
            )
            .bind(format!("concurrent-uuid-{}", i))
            .execute(&pool_clone)
            .await
            .unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        count, n as i64,
        "Exactly {} rows must exist after concurrent inserts",
        n
    );
}
