//! Integration tests for transaction builder
//!
//! Tests the keypair loading formats (hex / base58 / Solana CLI JSON array)
//! through the PUBLIC `load_wallet_keypair` API. NOTE: these intentionally
//! mirror the in-module unit tests in `transaction_builder.rs` — the
//! duplication is kept because public-API coverage is the goal here.

use chimera_operator::{
    engine::transaction_builder::load_wallet_keypair,
    models::{Action, Signal, SignalPayload, Strategy},
    vault::VaultSecrets,
};
use rust_decimal::Decimal;
use solana_sdk::signature::{Keypair, Signer};
use std::str::FromStr;

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
        wallet_private_key: Some(hex_key),
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    let loaded = load_wallet_keypair(&secrets).unwrap();
    assert_eq!(loaded.pubkey(), test_keypair.pubkey());
}

/// Loading a keypair stored as base58 should round-trip to the same pubkey.
#[test]
fn test_load_wallet_keypair_base58() {
    let test_keypair = Keypair::new();
    let b58 = bs58::encode(test_keypair.to_bytes()).into_string();

    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: Some(b58),
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    let loaded = load_wallet_keypair(&secrets).unwrap();
    assert_eq!(loaded.pubkey(), test_keypair.pubkey());
}

/// Loading a keypair stored as a Solana CLI JSON byte-array (id.json) should
/// round-trip to the same pubkey.
#[test]
fn test_load_wallet_keypair_json_array() {
    let test_keypair = Keypair::new();
    let json = serde_json::to_string(&test_keypair.to_bytes().to_vec()).unwrap();

    let secrets = VaultSecrets {
        webhook_secret: "test".to_string(),
        webhook_secret_previous: None,
        wallet_private_key: Some(json),
        rpc_api_key: None,
        fallback_rpc_api_key: None,
    };

    let loaded = load_wallet_keypair(&secrets).unwrap();
    assert_eq!(loaded.pubkey(), test_keypair.pubkey());
}

/// Test wallet keypair loading fails with invalid key material
#[test]
fn test_load_wallet_keypair_invalid() {
    // Malformed hex, malformed base58, malformed JSON array, and a
    // correctly-encoded but wrong-length key must ALL be rejected.
    let malformed_inputs = vec![
        "not-valid-hex".to_string(),
        "!!!!not-base58!!!!".to_string(),
        "[1, 2, 3".to_string(), // truncated JSON array
        "{not json}".to_string(),
        hex::encode([7u8; 31]), // valid hex but wrong length (31 bytes)
    ];

    for input in malformed_inputs {
        let secrets = VaultSecrets {
            webhook_secret: "test".to_string(),
            webhook_secret_previous: None,
            wallet_private_key: Some(input.clone()),
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        };
        assert!(
            load_wallet_keypair(&secrets).is_err(),
            "key material must be rejected: {input}"
        );
    }
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
