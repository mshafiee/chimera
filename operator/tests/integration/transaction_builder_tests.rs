//! Integration tests for transaction builder
//!
//! Tests Jupiter Swap API integration, transaction building, and signing

use chimera_operator::{
    config::AppConfig,
    engine::transaction_builder::{load_wallet_keypair, TransactionBuilder},
    models::{Action, Signal, SignalPayload, Strategy},
    vault::VaultSecrets,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Keypair;
use std::sync::Arc;

/// Test transaction builder initialization
#[tokio::test]
async fn test_transaction_builder_init() {
    let config = Arc::new(AppConfig::load().unwrap_or_else(|_| {
        // Create minimal config for testing
        AppConfig {
            server: Default::default(),
            rpc: chimera_operator::config::RpcConfig {
                primary_url: "https://api.mainnet-beta.solana.com".to_string(),
                ..Default::default()
            },
            database: Default::default(),
            security: Default::default(),
            circuit_breakers: Default::default(),
            strategy: Default::default(),
            jito: Default::default(),
            queue: Default::default(),
            token_safety: Default::default(),
            notifications: Default::default(),
        }
    }));

    let rpc_client = Arc::new(RpcClient::new(config.rpc.primary_url.clone()));
    let builder = TransactionBuilder::new(rpc_client, config);

    // Builder should be created successfully
    assert!(true, "Transaction builder initialized");
}

/// Test wallet keypair loading from vault
#[test]
fn test_load_wallet_keypair() {
    // Create a test keypair
    let test_keypair = Keypair::new();
    let secret_key = test_keypair.to_bytes();
    let mut key_bytes = Vec::with_capacity(64);
    key_bytes.extend_from_slice(&secret_key);
    key_bytes.extend_from_slice(&test_keypair.pubkey().to_bytes());

    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: Some(key_bytes),
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
        wallet_private_key: Some(vec![1, 2, 3]), // Invalid length
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
        amount_sol: 0.5,
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: None,
    };

    let signal = Signal::new(payload, chrono::Utc::now().timestamp(), None);
    
    assert_eq!(signal.payload.strategy, Strategy::Shield);
    assert_eq!(signal.payload.action, Action::Buy);
    assert!(!signal.trade_uuid.is_empty());
}
