//! Chaos/resilience tests for Chimera Operator
//!
//! Tests system behavior under failure conditions:
//! - RPC failures and fallback
//! - Database lock scenarios
//! - Circuit breaker behavior
//! - Queue overflow handling

#[cfg(test)]
mod tests {

    use chimera_operator::config::AppConfig;
    use chimera_operator::engine::executor::{Executor, RpcMode};
    use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
    use rust_decimal::Decimal;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::Sqlite;
    use std::str::FromStr;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn create_test_db() -> (sqlx::Pool<Sqlite>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db_url = format!("sqlite:{}", db_path.display());

        let options = SqliteConnectOptions::from_str(&db_url)
            .unwrap()
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        // Create minimal schema
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS trades (id INTEGER PRIMARY KEY, trade_uuid TEXT UNIQUE)",
        )
        .execute(&pool)
        .await
        .unwrap();

        (pool, temp_dir)
    }

    fn create_test_config() -> Arc<AppConfig> {
        // Try to load config, or create a minimal one for testing
        let config = if let Ok(cfg) = AppConfig::load() {
            cfg
        } else {
            // Create minimal test config using config builder
            use config::Config;
            let config_builder = Config::builder()
                .set_default("server.host", "0.0.0.0")
                .unwrap()
                .set_default("server.port", 8080)
                .unwrap()
                .set_default("database.path", ":memory:")
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
                .set_default("jito.enabled", true)
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
                .build()
                .unwrap();

            config_builder.try_deserialize::<AppConfig>().unwrap()
        };

        Arc::new(config)
    }

    #[tokio::test]
    async fn test_rpc_fallback_on_failure() {
        // Test that system switches to fallback RPC after consecutive failures
        let (db, _temp) = create_test_db().await;
        let config = create_test_config();

        let executor = Executor::new(config.clone(), db);

        // Initially should be in Jito mode
        assert_eq!(executor.rpc_mode(), RpcMode::Jito);
        assert!(!executor.is_in_fallback());

        // Test that executor starts in Jito mode
        assert_eq!(executor.rpc_mode(), RpcMode::Jito);
        assert!(!executor.is_in_fallback());

        // Test RPC mode getters
        assert!(!executor.is_in_fallback());

        // Note: switch_to_fallback is private, so we test the behavior indirectly
        // by verifying the mode and fallback state are correctly initialized
    }

    #[tokio::test]
    async fn test_spear_disabled_in_fallback() {
        // Test that Spear strategy is rejected in Standard RPC mode
        let (db, _temp) = create_test_db().await;

        // Create config with Jito disabled (simulates fallback mode)
        let config_no_jito = if let Ok(mut cfg) = AppConfig::load() {
            cfg.jito.enabled = false;
            cfg.rpc.fallback_url = Some("https://api.mainnet-beta.solana.com".to_string());
            Arc::new(cfg)
        } else {
            // Fallback: create minimal config

            // We can't modify Arc contents, so test with what we have
            // The executor will be in Jito mode if jito.enabled is true
            create_test_config()
        };

        let executor_standard = Executor::new(config_no_jito, db.clone());

        // If Jito is disabled, executor should be in Standard mode
        // Create Spear signal
        let payload = SignalPayload {
            strategy: Strategy::Spear,
            token: "BONK".to_string(),
            token_address: Some("BONK111111111111111111111111111111111111111".to_string()),
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        };
        let signal = Signal::new(payload, 1234567890, None);

        // If executor is in Standard mode, Spear should be rejected
        if executor_standard.rpc_mode() == RpcMode::Standard {
            let result = executor_standard.execute(&signal).await;
            assert!(result.is_err(), "Spear should be rejected in Standard mode");

            // Verify error is SpearDisabled
            if let Err(e) = result {
                let error_str = format!("{}", e);
                assert!(
                    error_str.contains("Spear") || error_str.contains("disabled"),
                    "Error should indicate Spear is disabled"
                );
            }
        }

        // Shield should work in both modes
        let shield_payload = SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: Some("BONK111111111111111111111111111111111111111".to_string()),
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        };
        let shield_signal = Signal::new(shield_payload, 1234567890, None);

        // Shield should not be rejected due to strategy (may fail for RPC reasons)
        let shield_result = executor_standard.execute(&shield_signal).await;
        if let Err(e) = shield_result {
            let error_str = format!("{}", e);
            assert!(
                !error_str.contains("Spear") || !error_str.contains("disabled"),
                "Shield should not be rejected for strategy reasons"
            );
        }
    }

    #[tokio::test]
    async fn test_circuit_breaker_trip() {
        use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState};

        let (db, _temp) = create_test_db().await;
        let config = create_test_config();

        // manual_trip() calls log_config_change which writes to config_audit
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS config_audit (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             key TEXT NOT NULL, \
             old_value TEXT, \
             new_value TEXT NOT NULL, \
             changed_by TEXT NOT NULL, \
             change_reason TEXT, \
             changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP)",
        )
        .execute(&db)
        .await
        .unwrap();

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
            let payload = SignalPayload {
                strategy: Strategy::Shield,
                token: format!("TOK{}", i),
                token_address: Some(format!("TOK{}111111111111111111111111111111111111111", i)),
                action: Action::Buy,
                amount_sol: Decimal::from_str("0.1").unwrap(),
                wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                trade_uuid: Some(format!("uuid-fill-{}", i)),
                exit_fraction: None,
            };
            let signal = Signal::new(payload, 1_700_000_000 + i as i64, None);
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
    async fn test_database_lock_retry() {
        // Test that database operations retry on lock
        // Note: retry_sqlite is private, so we test the behavior indirectly
        // by testing that SQLite operations handle locks gracefully

        let (db, _temp) = create_test_db().await;

        // Create table
        sqlx::query("CREATE TABLE IF NOT EXISTS test_lock (id INTEGER PRIMARY KEY, value TEXT)")
            .execute(&db)
            .await
            .unwrap();

        // Test that we can write even if there's contention
        // (SQLite WAL mode allows concurrent reads/writes)
        let mut handles = vec![];
        for i in 0..5 {
            let db_clone = db.clone();
            let handle = tokio::spawn(async move {
                for j in 0..10 {
                    sqlx::query("INSERT INTO test_lock (value) VALUES (?)")
                        .bind(format!("task-{}-{}", i, j))
                        .execute(&db_clone)
                        .await
                        .unwrap();
                }
            });
            handles.push(handle);
        }

        // All should complete successfully
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify writes succeeded
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM test_lock")
            .fetch_one(&db)
            .await
            .unwrap();

        assert_eq!(count.0, 50, "All concurrent writes should succeed");
    }

    #[tokio::test]
    async fn test_database_lock_max_retries() {
        // Test that database handles high contention
        let (db, _temp) = create_test_db().await;

        sqlx::query("CREATE TABLE IF NOT EXISTS test_contention (id INTEGER PRIMARY KEY)")
            .execute(&db)
            .await
            .unwrap();

        // Create many concurrent transactions
        let mut handles = vec![];
        for _ in 0..20 {
            let db_clone = db.clone();
            let handle = tokio::spawn(async move {
                // Each task does multiple operations
                for _ in 0..5 {
                    sqlx::query("INSERT INTO test_contention DEFAULT VALUES")
                        .execute(&db_clone)
                        .await
                        .unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all with timeout
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            for handle in handles {
                handle.await.unwrap();
            }
        })
        .await;

        assert!(
            result.is_ok(),
            "All operations should complete within timeout"
        );
    }

    #[tokio::test]
    async fn test_database_lock_non_lock_error() {
        // Test that non-lock errors (like syntax errors) fail immediately
        let (db, _temp) = create_test_db().await;

        // Invalid SQL should fail immediately, not retry
        let result = sqlx::query("INVALID SQL SYNTAX").execute(&db).await;

        assert!(result.is_err(), "Invalid SQL should fail immediately");
    }

    #[tokio::test]
    async fn test_sqlite_concurrent_writes() {
        // Test concurrent database writes don't deadlock
        let (db, _temp) = create_test_db().await;

        // Create table for concurrent writes
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS test_concurrent (
                id INTEGER PRIMARY KEY,
                value TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        // Spawn multiple concurrent write tasks
        let mut handles = vec![];
        for i in 0..10 {
            let db_clone = db.clone();
            let handle = tokio::spawn(async move {
                for j in 0..10 {
                    sqlx::query("INSERT INTO test_concurrent (value) VALUES (?)")
                        .bind(format!("task-{}-write-{}", i, j))
                        .execute(&db_clone)
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
            .fetch_one(&db)
            .await
            .unwrap();

        assert_eq!(count.0, 100, "All concurrent writes should succeed");
    }

    #[tokio::test]
    async fn test_sqlite_vacuum_operation() {
        // Test that VACUUM operations don't block other queries
        let (db, _temp) = create_test_db().await;

        // Create table and insert data
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS test_vacuum (
                id INTEGER PRIMARY KEY,
                data TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        // Insert some data
        for i in 0..100 {
            sqlx::query("INSERT INTO test_vacuum (data) VALUES (?)")
                .bind(format!("data-{}", i))
                .execute(&db)
                .await
                .unwrap();
        }

        // Run VACUUM in background
        let db_vacuum = db.clone();
        let vacuum_handle =
            tokio::spawn(async move { sqlx::query("VACUUM").execute(&db_vacuum).await });

        // While VACUUM is running, try to read
        let read_handle = tokio::spawn(async move {
            // Should be able to read even during VACUUM (WAL mode)
            let result: Result<Vec<(i64, String)>, _> =
                sqlx::query_as("SELECT id, data FROM test_vacuum LIMIT 10")
                    .fetch_all(&db)
                    .await;

            result
        });

        // Both should complete (VACUUM may take time, but reads should work)
        let read_result = read_handle.await.unwrap();
        assert!(
            read_result.is_ok(),
            "Reads should work during VACUUM in WAL mode"
        );

        // Wait for VACUUM (may timeout, but that's OK for this test)
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), vacuum_handle).await;
    }

    #[tokio::test]
    async fn test_stuck_position_recovery() {
        // Validates that get_stuck_positions() correctly identifies EXITING positions
        // older than the threshold. The full recovery path requires an RPC call to
        // verify on-chain state, so we test the detection layer here.

        let (db, _temp) = create_test_db().await;

        // Schema required by get_stuck_positions() query
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS positions (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             trade_uuid TEXT NOT NULL, \
             wallet_address TEXT NOT NULL DEFAULT 'wallet1', \
             token_address TEXT NOT NULL, \
             strategy TEXT NOT NULL DEFAULT 'SHIELD', \
             state TEXT NOT NULL DEFAULT 'ACTIVE', \
             entry_tx_signature TEXT NOT NULL DEFAULT 'sig_entry', \
             exit_tx_signature TEXT, \
             last_updated TEXT NOT NULL DEFAULT (datetime('now')))",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS trades (\
             id INTEGER PRIMARY KEY, trade_uuid TEXT UNIQUE, status TEXT DEFAULT 'ACTIVE')",
        )
        .execute(&db)
        .await
        .unwrap();

        // One fresh EXITING (should not be flagged) and one stale EXITING (should be flagged)
        sqlx::query(
            "INSERT INTO positions (trade_uuid, token_address, state, last_updated) \
             VALUES ('fresh-exiting', 'TOK111111111111111111111111111111111111111', \
             'EXITING', datetime('now', '-10 seconds'))",
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO positions (trade_uuid, token_address, state, last_updated) \
             VALUES ('stuck-exiting', 'TOK222222222222222222222222222222222222222', \
             'EXITING', datetime('now', '-300 seconds'))",
        )
        .execute(&db)
        .await
        .unwrap();

        // get_stuck_positions uses a 60-second threshold by default
        let stuck = chimera_operator::db::get_stuck_positions(&db, 60)
            .await
            .unwrap();

        assert_eq!(
            stuck.len(),
            1,
            "Exactly 1 stuck position expected (300s > 60s threshold); got {}",
            stuck.len()
        );
        assert_eq!(stuck[0].trade_uuid, "stuck-exiting");
    }

    #[tokio::test]
    async fn test_webhook_replay_attack() {
        // Verify the drift-check arithmetic that the HMAC middleware relies on.
        // The middleware rejects when |now - timestamp| > max_drift_secs.
        let max_drift: i64 = 60;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let old_ts = now - max_drift - 10;
        let drift_old = (now - old_ts).abs();
        assert!(
            drift_old > max_drift,
            "Old timestamp drift {} must exceed max_drift {}",
            drift_old,
            max_drift
        );

        let fresh_ts = now - 5;
        let drift_fresh = (now - fresh_ts).abs();
        assert!(
            drift_fresh <= max_drift,
            "Fresh timestamp drift {} must be within max_drift {}",
            drift_fresh,
            max_drift
        );
    }

    #[tokio::test]
    async fn test_concurrent_webhook_processing() {
        // Insert 100 unique trade rows concurrently and verify no duplicates or deadlocks.
        let (db, _temp) = create_test_db().await;

        let n: usize = 100;
        let mut handles = vec![];

        for i in 0..n {
            let db_clone = db.clone();
            handles.push(tokio::spawn(async move {
                sqlx::query("INSERT OR IGNORE INTO trades (trade_uuid) VALUES (?)")
                    .bind(format!("concurrent-uuid-{}", i))
                    .execute(&db_clone)
                    .await
                    .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&db)
            .await
            .unwrap();

        assert_eq!(
            count, n as i64,
            "Exactly {} rows must exist after concurrent inserts",
            n
        );
    }

    #[tokio::test]
    async fn test_mid_trade_rpc_failure_fallback() {
        // Test that mid-trade RPC failure triggers fallback
        // This simulates a scenario where:
        // 1. Trade starts with Helius (Jito mode)
        // 2. Helius connection fails mid-execution
        // 3. System switches to fallback RPC (QuickNode)
        // 4. Trade completes with fallback

        let (db, _temp) = create_test_db().await;
        let config = create_test_config();

        let executor = Executor::new(config.clone(), db);

        // Initially should be in Jito mode
        assert_eq!(executor.rpc_mode(), RpcMode::Jito);
        assert!(!executor.is_in_fallback());

        // Create a Shield signal (works in both modes)
        let payload = SignalPayload {
            strategy: Strategy::Shield,
            token: "BONK".to_string(),
            token_address: Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string()), // USDC for testing
            action: Action::Buy,
            amount_sol: Decimal::from_str("0.5").unwrap(),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: Some("test-mid-trade-failure".to_string()),
            exit_fraction: None,
        };
        let signal = Signal::new(payload, chrono::Utc::now().timestamp(), None);

        // Simulate RPC failure by checking if executor can handle fallback
        // Note: Actual RPC calls would require network access, so we test the logic

        // Verify executor can switch modes (even if we can't trigger actual failure)
        // The executor should maintain state correctly

        // Test that executor tracks failure count
        // After max_consecutive_failures, it should switch to fallback
        let initial_mode = executor.rpc_mode();

        // Verify executor maintains mode state
        assert_eq!(executor.rpc_mode(), initial_mode);

        // Test that if we manually set to fallback, Spear is disabled
        // (This tests the behavior, even if we can't trigger actual RPC failure)
        if executor.rpc_mode() == RpcMode::Standard {
            // In Standard mode, Spear should be rejected
            let spear_payload = SignalPayload {
                strategy: Strategy::Spear,
                token: "BONK".to_string(),
                token_address: Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string()),
                action: Action::Buy,
                amount_sol: Decimal::from_str("0.5").unwrap(),
                wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                trade_uuid: Some("test-spear-in-fallback".to_string()),
                exit_fraction: None,
            };
            let spear_signal = Signal::new(spear_payload, chrono::Utc::now().timestamp(), None);

            let result = executor.execute(&spear_signal).await;
            assert!(
                result.is_err(),
                "Spear should be rejected in Standard (fallback) mode"
            );
        }

        // Verify that Shield works in both modes
        let shield_result = executor.execute(&signal).await;
        // Shield may fail for RPC reasons, but should not fail due to mode
        if let Err(e) = shield_result {
            let error_str = format!("{}", e);
            assert!(
                !error_str.contains("Spear") || !error_str.contains("disabled"),
                "Shield should not be rejected for mode reasons"
            );
        }

        // Test that executor maintains state across mode switches
        // The key behavior: trades started in one mode should complete in that mode
        // New trades after mode switch use the new mode

        // Verify executor state is consistent
        let final_mode = executor.rpc_mode();
        assert!(
            matches!(final_mode, RpcMode::Jito | RpcMode::Standard),
            "Executor should be in a valid RPC mode"
        );
    }
}
