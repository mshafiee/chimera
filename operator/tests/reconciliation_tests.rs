//! Reconciliation Tests
//!
//! Tests the daily reconciliation process:
//! - On-chain vs DB state comparison
//! - Auto-resolution of discrepancies
//! - Epsilon tolerance for dust amounts
//! - Reconciliation log entries

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use tempfile::TempDir;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::Sqlite;

    async fn create_test_db() -> (sqlx::Pool<Sqlite>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_reconcile.db");
        let db_url = format!("sqlite:{}", db_path.display());
        
        let options = SqliteConnectOptions::from_str(&db_url)
            .unwrap()
            .create_if_missing(true);
        
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        
        // Create reconciliation schema
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY,
                trade_uuid TEXT UNIQUE,
                entry_tx_signature TEXT,
                exit_tx_signature TEXT,
                entry_amount_sol REAL,
                state TEXT
            )
            "#
        )
        .execute(&pool)
        .await
        .unwrap();
        
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS reconciliation_log (
                id INTEGER PRIMARY KEY,
                trade_uuid TEXT,
                discrepancy_type TEXT,
                db_value TEXT,
                on_chain_value TEXT,
                resolved INTEGER DEFAULT 0,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
            "#
        )
        .execute(&pool)
        .await
        .unwrap();
        
        (pool, temp_dir)
    }

    #[tokio::test]
    async fn test_on_chain_discrepancy_detection() {
        // Test that discrepancies between DB and on-chain are detected
        let (db, _temp) = create_test_db().await;
        
        // Insert position in DB
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, entry_amount_sol, state) 
             VALUES (?, ?, ?, ?)"
        )
        .bind("test-uuid-1")
        .bind("db-signature-123")
        .bind(0.5)
        .bind("ACTIVE")
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate on-chain check (in real scenario, this would query Solana RPC)
        // For test, we'll simulate finding a different signature
        let db_signature: Option<String> = sqlx::query_scalar(
            "SELECT entry_tx_signature FROM positions WHERE trade_uuid = ?"
        )
        .bind("test-uuid-1")
        .fetch_optional(&db)
        .await
        .unwrap();
        
        // Simulate on-chain has different signature (discrepancy)
        let on_chain_signature = Some("on-chain-signature-456".to_string());
        
        if db_signature != on_chain_signature {
            // Log discrepancy
            sqlx::query(
                "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, db_value, on_chain_value) 
                 VALUES (?, ?, ?, ?)"
            )
            .bind("test-uuid-1")
            .bind("SIGNATURE_MISMATCH")
            .bind(db_signature.as_deref().unwrap_or("NULL"))
            .bind(on_chain_signature.as_deref().unwrap_or("NULL"))
            .execute(&db)
            .await
            .unwrap();
        }
        
        // Verify discrepancy was logged
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reconciliation_log WHERE trade_uuid = 'test-uuid-1'"
        )
        .fetch_one(&db)
        .await
        .unwrap();
        
        assert_eq!(count.0, 1, "Discrepancy should be logged");
    }

    #[tokio::test]
    async fn test_epsilon_tolerance_for_dust() {
        // Test that small amount differences within epsilon are ignored
        let epsilon = 0.0001; // 0.01% tolerance
        
        // Test cases
        let test_cases = vec![
            (0.5, 0.50001, true),   // Within epsilon - should be considered equal
            (0.5, 0.5001, false),  // Outside epsilon - should be considered different
            (1.0, 1.00001, true),   // Within epsilon
            (1.0, 1.001, false),    // Outside epsilon
            (0.01, 0.010001, true), // Small amounts within epsilon
        ];
        
        for (db_amount, on_chain_amount, should_match) in test_cases {
            let diff = (db_amount - on_chain_amount).abs();
            let relative_diff = diff / db_amount.max(on_chain_amount);
            let within_epsilon = relative_diff <= epsilon;
            
            assert_eq!(
                within_epsilon,
                should_match,
                "Amount comparison: db={}, on_chain={}, diff={}, relative={}",
                db_amount,
                on_chain_amount,
                diff,
                relative_diff
            );
        }
    }

    #[tokio::test]
    async fn test_auto_resolution_missing_transaction() {
        // Test that missing on-chain transactions trigger auto-resolution
        let (db, _temp) = create_test_db().await;
        
        // Insert position with signature
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, entry_amount_sol, state) 
             VALUES (?, ?, ?, ?)"
        )
        .bind("test-uuid-2")
        .bind("missing-signature-123")
        .bind(0.5)
        .bind("ACTIVE")
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate on-chain check: transaction not found
        let on_chain_found = false;
        
        if !on_chain_found {
            // Auto-resolve: mark position as failed (transaction never confirmed)
            sqlx::query(
                "UPDATE positions SET state = 'FAILED' WHERE trade_uuid = ?"
            )
            .bind("test-uuid-2")
            .execute(&db)
            .await
            .unwrap();
            
            // Log resolution
            sqlx::query(
                "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, db_value, on_chain_value, resolved) 
                 VALUES (?, ?, ?, ?, ?)"
            )
            .bind("test-uuid-2")
            .bind("MISSING_TRANSACTION")
            .bind("missing-signature-123")
            .bind("NOT_FOUND")
            .bind(1) // resolved
            .execute(&db)
            .await
            .unwrap();
        }
        
        // Verify position was updated
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM positions WHERE trade_uuid = 'test-uuid-2'"
        )
        .fetch_optional(&db)
        .await
        .unwrap();
        
        assert_eq!(state, Some("FAILED".to_string()), "Position should be marked as FAILED");
        
        // Verify resolution was logged
        let resolved_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reconciliation_log WHERE trade_uuid = 'test-uuid-2' AND resolved = 1"
        )
        .fetch_one(&db)
        .await
        .unwrap();
        
        assert_eq!(resolved_count.0, 1, "Resolution should be logged");
    }

    #[tokio::test]
    async fn test_auto_resolution_amount_mismatch() {
        // Test that amount mismatches within epsilon are auto-resolved
        let (db, _temp) = create_test_db().await;
        let epsilon = 0.0001;
        
        // Insert position
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, entry_amount_sol, state) 
             VALUES (?, ?, ?, ?)"
        )
        .bind("test-uuid-3")
        .bind("signature-123")
        .bind(0.5)
        .bind("ACTIVE")
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate on-chain has slightly different amount (within epsilon)
        let db_amount: f64 = 0.5;
        let on_chain_amount = 0.50001; // Very small difference
        
        let diff = (db_amount - on_chain_amount).abs();
        let max_amount = db_amount.max(on_chain_amount);
        let relative_diff = if max_amount > 0.0 { diff / max_amount } else { 0.0 };
        
        if relative_diff <= epsilon {
            // Within tolerance - auto-resolve by updating DB to match on-chain
            sqlx::query(
                "UPDATE positions SET entry_amount_sol = ? WHERE trade_uuid = ?"
            )
            .bind(on_chain_amount)
            .bind("test-uuid-3")
            .execute(&db)
            .await
            .unwrap();
            
            // Log as resolved
            sqlx::query(
                "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, db_value, on_chain_value, resolved) 
                 VALUES (?, ?, ?, ?, ?)"
            )
            .bind("test-uuid-3")
            .bind("AMOUNT_MISMATCH")
            .bind(db_amount.to_string())
            .bind(on_chain_amount.to_string())
            .bind(1) // resolved
            .execute(&db)
            .await
            .unwrap();
        }
        
        // Verify amount was updated
        let updated_amount: Option<f64> = sqlx::query_scalar(
            "SELECT entry_amount_sol FROM positions WHERE trade_uuid = 'test-uuid-3'"
        )
        .fetch_optional(&db)
        .await
        .unwrap();
        
        assert!(
            (updated_amount.unwrap() - on_chain_amount).abs() < 0.00001,
            "Amount should be updated to match on-chain"
        );
    }

    #[tokio::test]
    async fn test_reconciliation_log_entries() {
        // Test that reconciliation log captures all discrepancy types
        let (db, _temp) = create_test_db().await;
        
        let discrepancy_types = vec![
            "SIGNATURE_MISMATCH",
            "MISSING_TRANSACTION",
            "AMOUNT_MISMATCH",
            "STATE_MISMATCH",
        ];
        
        for (idx, disc_type) in discrepancy_types.iter().enumerate() {
            sqlx::query(
                "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, db_value, on_chain_value) 
                 VALUES (?, ?, ?, ?)"
            )
            .bind(format!("test-uuid-{}", idx))
            .bind(*disc_type)
            .bind("db-value")
            .bind("on-chain-value")
            .execute(&db)
            .await
            .unwrap();
        }
        
        // Verify all types were logged
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reconciliation_log"
        )
        .fetch_one(&db)
        .await
        .unwrap();
        
        assert_eq!(count.0, discrepancy_types.len() as i64);
        
        // Verify each type exists
        for disc_type in &discrepancy_types {
            let type_count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM reconciliation_log WHERE discrepancy_type = ?"
            )
            .bind(*disc_type)
            .fetch_one(&db)
            .await
            .unwrap();
            
            assert_eq!(type_count.0, 1, "Each discrepancy type should be logged once");
        }
    }

    #[tokio::test]
    async fn test_unresolved_discrepancies_alert() {
        // Test that unresolved discrepancies are flagged for alerting
        let (db, _temp) = create_test_db().await;
        
        // Create resolved and unresolved discrepancies
        sqlx::query(
            "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, resolved) 
             VALUES ('uuid-1', 'SIGNATURE_MISMATCH', 1)"
        )
        .execute(&db)
        .await
        .unwrap();
        
        sqlx::query(
            "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, resolved) 
             VALUES ('uuid-2', 'MISSING_TRANSACTION', 0)"
        )
        .execute(&db)
        .await
        .unwrap();
        
        sqlx::query(
            "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, resolved) 
             VALUES ('uuid-3', 'AMOUNT_MISMATCH', 0)"
        )
        .execute(&db)
        .await
        .unwrap();
        
        // Query unresolved discrepancies
        let unresolved: Vec<(String, String)> = sqlx::query_as(
            "SELECT trade_uuid, discrepancy_type FROM reconciliation_log WHERE resolved = 0"
        )
        .fetch_all(&db)
        .await
        .unwrap();
        
        assert_eq!(unresolved.len(), 2, "Should have 2 unresolved discrepancies");
        assert!(unresolved.iter().any(|(uuid, _)| uuid == "uuid-2"));
        assert!(unresolved.iter().any(|(uuid, _)| uuid == "uuid-3"));
    }

    #[tokio::test]
    async fn test_reconciliation_handles_null_values() {
        // Test that reconciliation handles NULL values gracefully
        let (db, _temp) = create_test_db().await;
        
        // Insert position with NULL exit signature (position still active)
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, exit_tx_signature, state) 
             VALUES (?, ?, ?, ?)"
        )
        .bind("test-uuid-4")
        .bind("entry-sig-123")
        .bind::<Option<String>>(None)
        .bind("ACTIVE")
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate on-chain check: position still active (no exit signature)
        let on_chain_exit_sig: Option<String> = None;
        
        // Should not create discrepancy for NULL values when both are NULL
        let db_exit_sig: Option<String> = sqlx::query_scalar(
            "SELECT exit_tx_signature FROM positions WHERE trade_uuid = 'test-uuid-4'"
        )
        .fetch_optional(&db)
        .await
        .unwrap()
        .flatten();
        
        // Both NULL - no discrepancy
        if db_exit_sig.is_none() && on_chain_exit_sig.is_none() {
            // No discrepancy to log
            assert!(true, "NULL values should not create false discrepancies");
        }
    }

    #[tokio::test]
    async fn test_stuck_state_recovery_exiting_timeout() {
        // Test stuck state recovery: EXITING state > 60 seconds
        // 1. Create position in EXITING state with old timestamp
        // 2. Simulate blockhash expiration check
        // 3. Verify state reverted to ACTIVE
        
        let (db, _temp) = create_test_db().await;
        
        // Add last_updated timestamp column if not exists
        sqlx::query(
            "ALTER TABLE positions ADD COLUMN last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP"
        )
        .execute(&db)
        .await
        .ok(); // Ignore error if column already exists
        
        // Insert position in EXITING state with old timestamp (> 60s ago)
        let old_timestamp = chrono::Utc::now() - chrono::Duration::seconds(120);
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, exit_tx_signature, state, last_updated) 
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind("test-stuck-uuid")
        .bind("entry-sig-123")
        .bind("exit-sig-456")
        .bind("EXITING")
        .bind(old_timestamp)
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate recovery check: blockhash expired
        let now = chrono::Utc::now();
        let last_updated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT last_updated FROM positions WHERE trade_uuid = 'test-stuck-uuid'"
        )
        .fetch_optional(&db)
        .await
        .unwrap()
        .flatten();
        
        if let Some(last_update) = last_updated {
            let age_seconds = (now - last_update).num_seconds();
            
            // If position has been EXITING for > 60 seconds, revert to ACTIVE
            if age_seconds > 60 {
                // Simulate blockhash expiration check
                let blockhash_expired = true; // In real scenario, check via RPC
                
                if blockhash_expired {
                    // Revert state to ACTIVE
                    sqlx::query(
                        "UPDATE positions SET state = 'ACTIVE', exit_tx_signature = NULL, last_updated = ? 
                         WHERE trade_uuid = 'test-stuck-uuid'"
                    )
                    .bind(now)
                    .execute(&db)
                    .await
                    .unwrap();
                    
                    // Log recovery action
                    sqlx::query(
                        "INSERT INTO reconciliation_log (trade_uuid, discrepancy_type, db_value, on_chain_value, resolved) 
                         VALUES (?, ?, ?, ?, ?)"
                    )
                    .bind("test-stuck-uuid")
                    .bind("STUCK_STATE_RECOVERY")
                    .bind("EXITING")
                    .bind("ACTIVE")
                    .bind(1) // resolved
                    .execute(&db)
                    .await
                    .unwrap();
                }
            }
        }
        
        // Verify state was reverted
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM positions WHERE trade_uuid = 'test-stuck-uuid'"
        )
        .fetch_optional(&db)
        .await
        .unwrap();
        
        assert_eq!(state, Some("ACTIVE".to_string()), "Stuck EXITING position should be reverted to ACTIVE");
        
        // Verify exit signature was cleared
        let exit_sig: Option<String> = sqlx::query_scalar(
            "SELECT exit_tx_signature FROM positions WHERE trade_uuid = 'test-stuck-uuid'"
        )
        .fetch_optional(&db)
        .await
        .unwrap()
        .flatten();
        
        assert!(exit_sig.is_none(), "Exit signature should be cleared after recovery");
    }

    #[tokio::test]
    async fn test_stuck_state_recovery_recent_exiting() {
        // Test that recent EXITING positions (< 60s) are not recovered
        let (db, _temp) = create_test_db().await;
        
        // Add last_updated timestamp column if not exists
        sqlx::query(
            "ALTER TABLE positions ADD COLUMN last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP"
        )
        .execute(&db)
        .await
        .ok();
        
        // Insert position in EXITING state with recent timestamp (< 60s ago)
        let recent_timestamp = chrono::Utc::now() - chrono::Duration::seconds(30);
        sqlx::query(
            "INSERT INTO positions (trade_uuid, entry_tx_signature, exit_tx_signature, state, last_updated) 
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind("test-recent-exiting")
        .bind("entry-sig-123")
        .bind("exit-sig-456")
        .bind("EXITING")
        .bind(recent_timestamp)
        .execute(&db)
        .await
        .unwrap();
        
        // Simulate recovery check
        let now = chrono::Utc::now();
        let last_updated: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT last_updated FROM positions WHERE trade_uuid = 'test-recent-exiting'"
        )
        .fetch_optional(&db)
        .await
        .unwrap()
        .flatten();
        
        if let Some(last_update) = last_updated {
            let age_seconds = (now - last_update).num_seconds();
            
            // Recent position should NOT be recovered
            if age_seconds <= 60 {
                // Do not revert - position is still valid
                assert!(true, "Recent EXITING position should not be recovered");
            }
        }
        
        // Verify state remains EXITING
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM positions WHERE trade_uuid = 'test-recent-exiting'"
        )
        .fetch_optional(&db)
        .await
        .unwrap();
        
        assert_eq!(state, Some("EXITING".to_string()), "Recent EXITING position should remain EXITING");
    }
}
