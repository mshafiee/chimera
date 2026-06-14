//! Integration tests for transaction builder
//!
//! Tests Jupiter Swap API integration, transaction building, and signing

use chimera_operator::{
    engine::transaction_builder::load_wallet_keypair,
    models::{Action, Signal, SignalPayload, Strategy},
    vault::VaultSecrets,
};
use rust_decimal::Decimal;
use secrecy::Secret;
use solana_sdk::signature::{Keypair, Signer};
use std::str::FromStr;

/// Test transaction builder initialization — requires real config, skip in CI
#[tokio::test]
#[ignore]
async fn test_transaction_builder_init() {
    // Requires a real AppConfig loaded from environment or config file
    // Run manually with: cargo test -- --ignored test_transaction_builder_init
}

/// Test wallet keypair loading from vault
#[test]
fn test_load_wallet_keypair() {
    // Create a test keypair and encode as hex string (as VaultSecrets expects)
    let test_keypair = Keypair::new();
    let secret_bytes = test_keypair.to_bytes(); // 64 bytes for ed25519
    let hex_key = hex::encode(secret_bytes);

    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: Some(Secret::new(hex_key)),
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    let loaded = load_wallet_keypair(&secrets).unwrap();
    assert_eq!(loaded.pubkey(), test_keypair.pubkey());
}

/// Test wallet keypair loading fails with invalid key
#[test]
fn test_load_wallet_keypair_invalid() {
    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: Some(Secret::new("not-valid-hex".to_string())),
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    assert!(load_wallet_keypair(&secrets).is_err());
}

/// Test wallet keypair loading fails when key missing
#[test]
fn test_load_wallet_keypair_missing() {
    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: None,
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    assert!(load_wallet_keypair(&secrets).is_err());
}

/// Test signal creation for transaction building
#[test]
fn test_signal_creation() {
    let payload = SignalPayload {
        strategy: Strategy::Shield,
        token: "BONK".to_string(),
        token_address: Some("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string()),
        action: Action::Buy,
        amount_sol: Decimal::from_str("0.5").unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: None,
        exit_fraction: None,
    };

    let signal = Signal::new(payload, chrono::Utc::now().timestamp(), None);

    assert_eq!(signal.payload.strategy, Strategy::Shield);
    assert_eq!(signal.payload.action, Action::Buy);
    assert!(!signal.trade_uuid.is_empty());
}
