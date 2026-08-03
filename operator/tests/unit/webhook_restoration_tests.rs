//! Webhook lifecycle tests (deterministic surface)
//!
//! The live Helius API surface (register/update/reconcile/health) is
//! network-dependent and must never run in the default suite — in particular,
//! a manager constructed with `helius_dry_run: false` could delete real
//! production webhooks, so NO test in this file constructs a non-dry-run
//! manager. Only deterministic paths are exercised here:
//!
//! - Safety default: `helius_dry_run` is true
//! - Solana address validation (pure)
//! - Registration of an invalid address is rejected before any network I/O
//! - Helius webhook payload parsing (pure)
//! - Reconciliation result structure (pure)

use chimera_operator::db_abstraction::Database;
use chimera_operator::monitoring::helius::{
    AccountData, HeliusClient, HeliusWebhookPayload, NativeTransfer, RawTokenAmount,
    TokenBalanceChange,
};
use chimera_operator::monitoring::rate_limiter::RateLimiter;
use chimera_operator::monitoring::transaction_parser::{
    parse_helius_webhook, SwapDirection,
};
use chimera_operator::monitoring::webhook_lifecycle::{
    is_valid_solana_address, WebhookLifecycleConfig, WebhookLifecycleManager,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
const TEST_WALLET: &str = "DakNYZdrGeFwF6BhD7ZhLU5qFPnGHXkAsLwq1w3SAJVc";

fn make_payload(account_data: Vec<AccountData>, native_transfers: Vec<NativeTransfer>) -> HeliusWebhookPayload {
    HeliusWebhookPayload {
        account_data,
        native_transfers,
        signature: "test_signature_123".to_string(),
        slot: 123456789,
        timestamp: chrono::Utc::now().timestamp(),
        transaction_error: None,
        transaction_type: "SWAP".to_string(),
    }
}

fn token_change(mint: &str, amount: &str, user_account: &str) -> TokenBalanceChange {
    TokenBalanceChange {
        mint: mint.to_string(),
        raw_token_amount: RawTokenAmount {
            token_amount: amount.to_string(),
            decimals: None,
        },
        token_account: format!("token_account_{user_account}"),
        user_account: user_account.to_string(),
    }
}

#[test]
fn test_config_default_dry_run_true() {
    // Safety default: dry_run must be true (serde default in config.rs
    // `default_helius_dry_run`) so a misconfigured deployment can never delete
    // Helius webhooks.
    let config: chimera_operator::config::WebhookLifecycleConfig =
        serde_json::from_str("{}").expect("empty config must deserialize with defaults");
    assert!(config.helius_dry_run, "Default helius_dry_run should be true for safety");
}

#[test]
fn test_is_valid_solana_address() {
    assert!(is_valid_solana_address(TEST_WALLET));
    assert!(is_valid_solana_address(SOL_MINT));
    assert!(!is_valid_solana_address(""));
    assert!(!is_valid_solana_address("not-a-pubkey"));
}

#[tokio::test]
async fn test_register_wallet_webhook_invalid_address_rejected() {
    // An invalid address must be rejected locally (validation runs before any
    // network call), so this test is deterministic and hermetic.
    let (db, _guard) = common::create_test_db().await;
    let helius_client = Arc::new(
        HeliusClient::new(
            "test_key_123".to_string(),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        )
        .expect("HeliusClient must construct"),
    );
    let rate_limiter = Arc::new(RateLimiter::new(40, 1));
    let manager = WebhookLifecycleManager::new(
        db,
        helius_client,
        rate_limiter,
        WebhookLifecycleConfig {
            auto_register_enabled: true,
            auto_cleanup_enabled: true,
            health_check_interval_secs: 3600,
            stale_threshold_days: 7,
            max_registration_retries: 3,
            webhook_url: "https://test.example.com/webhook".to_string(),
            helius_dry_run: true,
            auth_header: None,
        },
    );

    let result = manager
        .register_wallet_webhook("not-a-valid-solana-address")
        .await
        .expect("registration must complete without error");

    assert!(!result.success, "invalid address must be rejected");
    assert!(
        result.error_message.as_deref().unwrap_or("").contains("Invalid Solana address"),
        "error must explain the invalid address, got: {:?}",
        result.error_message
    );
    assert!(result.webhook_id.is_empty());
}

#[tokio::test]
async fn test_parse_helius_webhook_buy() {
    // SOL -> BONK: the BONK token delta is positive (received) so the swap is a BUY.
    let payload = make_payload(
        vec![AccountData {
            account: TEST_WALLET.to_string(),
            native_balance_change: Some(-500_000_000), // 0.5 SOL out
            token_balance_changes: Some(vec![token_change(
                "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                "1000000",
                TEST_WALLET,
            )]),
        }],
        vec![NativeTransfer {
            amount: 500_000_000,
            from_user_account: TEST_WALLET.to_string(),
            to_user_account: "DEX_PROGRAM_ACCOUNT".to_string(),
        }],
    );

    let parsed = parse_helius_webhook(&payload, None)
        .expect("parse must not error")
        .expect("payload must parse as a swap");

    assert_eq!(parsed.direction, SwapDirection::Buy);
    assert_eq!(parsed.token_in, SOL_MINT);
    assert_eq!(parsed.token_out, "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263");
    assert_eq!(parsed.amount_in, Decimal::from_str("0.5").unwrap());
    assert_eq!(parsed.amount_out, Decimal::from_str("1000000").unwrap());
}

#[tokio::test]
async fn test_parse_helius_webhook_multiple_payloads() {
    // Both payloads must parse — array/batch processing must not silently drop
    // one of them.
    let payloads = vec![
        make_payload(
            vec![AccountData {
                account: TEST_WALLET.to_string(),
                native_balance_change: Some(-500_000_000),
                token_balance_changes: Some(vec![token_change(
                    "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                    "1000000",
                    TEST_WALLET,
                )]),
            }],
            vec![NativeTransfer {
                amount: 500_000_000,
                from_user_account: TEST_WALLET.to_string(),
                to_user_account: "DEX_PROGRAM_ACCOUNT".to_string(),
            }],
        ),
        make_payload(
            vec![AccountData {
                account: TEST_WALLET.to_string(),
                native_balance_change: Some(500_000_000),
                token_balance_changes: Some(vec![token_change(
                    "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                    "-1000000",
                    TEST_WALLET,
                )]),
            }],
            vec![NativeTransfer {
                amount: 500_000_000,
                from_user_account: "DEX_PROGRAM_ACCOUNT".to_string(),
                to_user_account: TEST_WALLET.to_string(),
            }],
        ),
    ];

    let mut processed_count = 0;
    for payload in payloads {
        if parse_helius_webhook(&payload, None)
            .expect("parse must not error")
            .is_some()
        {
            processed_count += 1;
        }
    }

    assert_eq!(
        processed_count, 2,
        "both webhook payloads must parse into swaps"
    );
}

#[tokio::test]
async fn test_parse_helius_webhook_tracked_wallet_filter() {
    // With tracked_wallet set, only that wallet's deltas are aggregated: the
    // other wallet's token delta must be ignored, so the result is a SELL.
    let payload = make_payload(
        vec![
            AccountData {
                account: TEST_WALLET.to_string(),
                native_balance_change: Some(-500_000_000),
                token_balance_changes: Some(vec![token_change(
                    "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                    "1000000",
                    "OtherWallet1111111111111111111111111111111111",
                )]),
            },
            AccountData {
                account: TEST_WALLET.to_string(),
                native_balance_change: Some(-500_000_000),
                token_balance_changes: Some(vec![token_change(
                    "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                    "-2000000",
                    TEST_WALLET,
                )]),
            },
        ],
        vec![NativeTransfer {
            amount: 500_000_000,
            from_user_account: TEST_WALLET.to_string(),
            to_user_account: "DEX_PROGRAM_ACCOUNT".to_string(),
        }],
    );

    let parsed = parse_helius_webhook(&payload, Some(TEST_WALLET))
        .expect("parse must not error")
        .expect("tracked wallet must produce a swap");

    // Only the tracked wallet's delta (-2_000_000) counts.
    assert_eq!(parsed.direction, SwapDirection::Sell);
    assert_eq!(parsed.token_out, SOL_MINT);
    assert_eq!(parsed.amount_out, Decimal::from_str("2000000").unwrap());
}

#[tokio::test]
async fn test_reconciliation_result_structure() {
    // Pin the result-shape contract so callers can rely on the fields.
    let result = chimera_operator::monitoring::webhook_lifecycle::ReconciliationResult {
        registered: 1,
        orphaned: 1,
        updated: 0,
        failed: 0,
        would_delete: vec![("webhook_orphan".to_string(), "no matching DB record".to_string())],
        duration_ms: 5,
    };

    assert_eq!(result.registered, 1);
    assert_eq!(result.orphaned, 1);
    assert_eq!(result.updated, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.would_delete.len(), 1);
    assert_eq!(result.would_delete[0].0, "webhook_orphan");
}
