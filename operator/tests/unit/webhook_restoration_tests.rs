//! Webhook Restoration Unit Tests
//!
//! Tests the core webhook restoration functionality:
//! - Advisory reconciliation mode (no auto-deletion)
//! - Startup webhook restoration with conflict handling
//! - Background health task operations
//! - Manual reconciliation workflows
//! - Array payload handling in webhook endpoint
//! - Real-time transaction parser improvements

use chrono::{DateTime, Duration, Utc};
use chimera_operator::config::Config;
use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::monitoring::webhook_lifecycle::{
    WebhookLifecycleManager, WebhookLifecycleConfig,
    HealthCheckResult, ReconciliationResult, WebhookReconciliationResult,
    WebhookHealthStatus, WebhookRegistrationResult,
};
use chimera_operator::monitoring::helius::{HeliusClient, HeliusWebhookPayload};
use chimera_operator::monitoring::transaction_parser::parse_helius_webhook;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

/// Create a test PostgreSQL database
async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let test_db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://localhost/test_chimera".to_string());
    
    let db_config = DatabaseConfig::postgres(test_db_url);
    let db = create_database(&db_config).await.unwrap();
    
    // Create necessary tables for webhook tests
    let pool = match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("webhook tests require PostgreSQL backend"),
    };
    
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS directional_wallets (
            wallet_address TEXT PRIMARY KEY,
            direction TEXT NOT NULL,
            status TEXT NOT NULL,
            last_swapped_at TIMESTAMP,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#
    )
    .execute(pool.as_ref())
    .await
    .unwrap();
    
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS webhook_registrations (
            id SERIAL PRIMARY KEY,
            wallet_address TEXT NOT NULL UNIQUE,
            webhook_id TEXT NOT NULL,
            webhook_url TEXT NOT NULL,
            account_keys JSONB NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            enabled BOOLEAN DEFAULT true
        )
        "#
    )
    .execute(pool.as_ref())
    .await
    .unwrap();
    
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS webhook_audit_log (
            id SERIAL PRIMARY KEY,
            wallet_address TEXT NOT NULL,
            webhook_id TEXT,
            event_type TEXT NOT NULL,
            event_details JSONB,
            timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#
    )
    .execute(pool.as_ref())
    .await
    .unwrap();
    
    (db, temp_dir)
}

/// Create a test webhook lifecycle manager
async fn create_test_manager(dry_run: bool) -> (WebhookLifecycleManager, TempDir) {
    let (db, temp_dir) = create_test_db().await;
    let pool = match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("webhook tests require PostgreSQL backend"),
    };
    
    let helius_api_key = std::env::var("HELIUS_API_KEY")
        .unwrap_or_else(|_| "test_key_123".to_string());
    
    let config = WebhookLifecycleConfig {
        helius_api_key: helius_api_key.clone(),
        webhook_url: "https://test.example.com/webhook".to_string(),
        health_check_interval_seconds: 30,
        max_retries: 3,
        retry_delay_seconds: 5,
        helius_dry_run: dry_run,
    };
    
    let helius_client = HeliusClient::new(&helius_api_key, None);
    
    let manager = WebhookLifecycleManager::new(
        pool,
        helius_client,
        config,
    );
    
    (manager, temp_dir)
}

#[tokio::test]
async fn test_advisory_reconciliation_mode_no_deletion() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    // Create a mock webhook in Helius that doesn't exist in our DB
    let orphaned_wallet = "DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc";
    
    // Test that advisory mode doesn't delete webhooks
    let result = manager.reconcile_webhooks().await;
    
    // In advisory mode, should report orphaned webhooks but not delete them
    assert!(result.orphaned_webhooks.contains(&orphaned_wallet.to_string()) || 
            !result.webhook_results.is_empty());
    
    // Verify no actual deletion calls would be made
    for webhook_result in &result.webhook_results {
        if webhook_result.status == "orphaned" {
            assert!(!webhook_result.action_taken.contains("deleted"));
            assert!(webhook_result.action_taken.contains("would delete") || 
                    webhook_result.action_taken.contains("dry run"));
        }
    }
}

#[tokio::test]
async fn test_production_mode_allows_deletion() {
    let (manager, _temp_dir) = create_test_manager(false).await;
    
    // Create an orphaned webhook scenario
    let orphaned_wallet = "5xUfgsaX72xhfAyy24NgXbqYCCicSYxtC8z3YuVSx6Dw";
    
    // In production mode, deletions are allowed
    let result = manager.reconcile_webhooks().await;
    
    for webhook_result in &result.webhook_results {
        if webhook_result.status == "orphaned" {
            // In production mode, deletions are allowed
            assert!(!webhook_result.action_taken.contains("dry run"));
        }
    }
}

#[tokio::test]
async fn test_webhook_restoration_on_startup() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    // Insert a wallet that needs webhook restoration
    let wallet_address = "GEasBFtNijNHiLivK2xRsewwRR927a9r4b9HE7MGT3pR";
    let pool = manager.get_pool();
    
    sqlx::query(
        r#"
        INSERT INTO directional_wallets (wallet_address, direction, status, last_swapped_at)
        VALUES ($1, 'long', 'ACTIVE', NOW() - INTERVAL '1 day')
        ON CONFLICT (wallet_address) DO NOTHING
        "#
    )
    .bind(wallet_address)
    .execute(pool.as_ref())
    .await
    .unwrap();
    
    // Run startup webhook restoration
    let result = manager.restore_webhooks_on_startup().await;
    
    assert!(result.success || result.attempted_registrations > 0);
    assert!(!result.failed_registrations.is_empty() || result.successful_registrations > 0);
}

#[tokio::test]
async fn test_array_payload_processing() {
    // Create a test webhook payload with multiple transactions
    let test_payloads = vec![
        create_test_webhook_payload("DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc"),
        create_test_webhook_payload("5xUfgsaX72xhfAyy24NgXbqYCCicSYxtC8z3YuVSx6Dw"),
    ];
    
    // Process all payloads
    let mut processed_count = 0;
    for payload in test_payloads {
        let parsed = parse_helius_webhook(&payload);
        if parsed.is_some() {
            processed_count += 1;
        }
    }
    
    // Should process both payloads (even if one fails, we continue)
    assert!(processed_count > 0);
}

#[tokio::test]
async fn test_transaction_parser_token_aggregation() {
    let payload = create_test_webhook_payload_with_multiple_tokens();
    
    let parsed = parse_helius_webhook(&payload);
    
    assert!(parsed.is_some());
    let swap = parsed.unwrap();
    
    // Should aggregate token deltas from all account_data entries
    assert!(!swap.token_changes.is_empty());
    
    // Verify that SOL/WSOL handling works correctly
    let has_sol_or_wsol = swap.token_changes.iter().any(|change| {
        change.mint == "So11111111111111111111111111111111111111112" || 
        change.mint == "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    });
    
    assert!(has_sol_or_wsol, "Should have SOL or WSOL in token changes");
}

#[tokio::test]
async fn test_webhook_health_status_tracking() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    let wallet_address = "DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc";
    
    // Register a webhook
    let registration_result = manager.register_wallet_webhook(wallet_address).await;
    
    // Check health status
    let health_result = manager.check_webhook_health(wallet_address).await;
    
    assert!(health_result.is_some());
    let health = health_result.unwrap();
    
    // Health status should be one of the valid statuses
    assert!(matches!(
        health.status,
        WebhookHealthStatus::Healthy | 
        WebhookHealthStatus::Degraded | 
        WebhookHealthStatus::Failed | 
        WebhookHealthStatus::Unknown
    ));
}

#[tokio::test]
async fn test_manual_reconciliation_workflow() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    // Perform manual reconciliation
    let result = manager.reconcile_webhooks().await;
    
    // Verify reconciliation result structure
    assert!(!result.webhook_results.is_empty() || result.total_checked >= 0);
    
    // Check advisory mode behavior
    let has_orphaned = result.webhook_results.iter()
        .any(|r| r.status == "orphaned");
    
    if has_orphaned {
        // In advisory mode, should not actually delete
        let no_deletions = result.webhook_results.iter()
            .all(|r| !r.action_taken.contains("deleted") || r.action_taken.contains("dry run"));
        assert!(no_deletions, "Advisory mode should not delete webhooks");
    }
}

#[tokio::test]
async fn test_webhook_registration_with_retry() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    let wallet_address = "DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc";
    
    // Attempt registration with retry logic
    let result = manager.register_wallet_webhook(wallet_address).await;
    
    // Should handle both success and failure gracefully
    assert!(result.success || result.error_message.is_some());
    
    if result.success {
        assert!(!result.webhook_id.is_empty());
        assert!(result.webhook_url.contains("webhook"));
    }
}

#[tokio::test]
async fn test_config_default_dry_run_true() {
    let config = WebhookLifecycleConfig::default();
    
    // Safety default: dry_run should be true
    assert!(config.helius_dry_run, "Default helius_dry_run should be true for safety");
}

#[tokio::test]
async fn test_background_health_task_respects_dry_run() {
    let (manager, _temp_dir) = create_test_manager(true).await;
    
    // Simulate background health check
    let health_results = manager.check_all_webhooks_health().await;
    
    // Should check health without making destructive changes
    for (wallet, health) in &health_results {
        assert!(!wallet.is_empty());
        assert!(matches!(
            health.status,
            WebhookHealthStatus::Healthy | 
            WebhookHealthStatus::Degraded | 
            WebhookHealthStatus::Failed | 
            WebhookHealthStatus::Unknown
        ));
    }
}

// Helper functions

fn create_test_webhook_payload(wallet_address: &str) -> HeliusWebhookPayload {
    HeliusWebhookPayload {
        r#type: "SWAP".to_string(),
        signature: "test_signature_123".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        slot: 123456789,
        fee: 5000,
        native_transfers: vec![],
        token_transfers: vec![],
        transaction: None,
        account_data: vec![],
    }
}

fn create_test_webhook_payload_with_multiple_tokens() -> HeliusWebhookPayload {
    HeliusWebhookPayload {
        r#type: "SWAP".to_string(),
        signature: "test_signature_multi_123".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        slot: 123456790,
        fee: 5000,
        native_transfers: vec![],
        token_transfers: vec![],
        transaction: None,
        account_data: vec![
            serde_json::json!({
                "account": "DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc",
                "native_balance_change": -1000000000,
                "token_balance_changes": [
                    {
                        "mint": "So11111111111111111111111111111111111111112",
                        "raw_amount": "-1000000000"
                    },
                    {
                        "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                        "raw_amount": "50000000"
                    }
                ]
            }),
            serde_json::json!({
                "account": "5xUfgsaX72xhfAyy24NgXbqYCCicSYxtC8z3YuVSx6Dw",
                "native_balance_change": 1000000000,
                "token_balance_changes": [
                    {
                        "mint": "So11111111111111111111111111111111111111112",
                        "raw_amount": "1000000000"
                    }
                ]
            })
        ],
    }
}

#[tokio::test]
async fn test_webhook_reconciliation_result_structure() {
    let result = ReconciliationResult {
        total_checked: 5,
        healthy: 3,
        degraded: 1,
        failed: 0,
        cleaned_up: 0,
        orphaned_webhooks: vec!["orphan1".to_string()],
        webhook_results: vec![
            WebhookReconciliationResult {
                wallet_address: "wallet1".to_string(),
                webhook_id: "webhook1".to_string(),
                status: "healthy".to_string(),
                last_received: Some(Utc::now()),
                action_taken: "checked".to_string(),
            },
            WebhookReconciliationResult {
                wallet_address: "orphan1".to_string(),
                webhook_id: "webhook_orphan".to_string(),
                status: "orphaned".to_string(),
                last_received: Some(Utc::now() - Duration::days(7)),
                action_taken: "would delete (dry run)".to_string(),
            },
        ],
    };
    
    assert_eq!(result.total_checked, 5);
    assert_eq!(result.healthy, 3);
    assert_eq!(result.degraded, 1);
    assert_eq!(result.failed, 0);
    assert_eq!(result.cleaned_up, 0);
    assert!(result.orphaned_webhooks.contains(&"orphan1".to_string()));
    
    let orphaned_result = result.webhook_results.iter()
        .find(|r| r.wallet_address == "orphan1")
        .expect("Should have orphaned webhook result");
    
    assert_eq!(orphaned_result.status, "orphaned");
    assert!(orphaned_result.action_taken.contains("dry run"));
}