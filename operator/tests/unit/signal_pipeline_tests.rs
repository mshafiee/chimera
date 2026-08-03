//! Signal Pipeline Unit Tests
//!
//! Tests signal data model integrity:
//! - Signal construction and validation
//! - Signal priority ordering (EXIT > SHIELD > SPEAR)
//! - Deterministic UUID generation

use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use rust_decimal::Decimal;
use std::str::FromStr;

fn make_payload(action: Action, strategy: Strategy, amount_sol: &str) -> SignalPayload {
    SignalPayload {
        strategy,
        token: "So11111111111111111111111111111111111111112".to_string(),
        token_address: Some("So11111111111111111111111111111111111111112".to_string()),
        action,
        amount_sol: Decimal::from_str(amount_sol).unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: None,
        exit_fraction: None,
    }
}

fn make_signal(payload: SignalPayload, timestamp: i64) -> Signal {
    Signal::new(payload, timestamp, Some("127.0.0.1".to_string()))
}

#[test]
fn test_signal_creation() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal = make_signal(payload, 1700000000);
    assert!(!signal.trade_uuid.is_empty());
    assert_eq!(signal.source_ip, Some("127.0.0.1".to_string()));
}

#[test]
fn test_signal_trade_uuid_deterministic() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_eq!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Same payload should produce same UUID"
    );
}

#[test]
fn test_signal_trade_uuid_differs_for_different_amounts() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Buy, Strategy::Shield, "2.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different amounts should produce different UUIDs"
    );
}

#[test]
fn test_signal_trade_uuid_differs_for_different_actions() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Sell, Strategy::Shield, "1.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different actions should produce different UUIDs"
    );
}

#[test]
fn test_signal_trade_uuid_differs_for_different_wallets() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let mut payload2 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    payload2.wallet_address = "9xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string();
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different wallets should produce different UUIDs"
    );
}

#[test]
fn test_signal_trade_uuid_differs_for_different_tokens() {
    // The dedup key includes the token symbol/address.
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let mut payload2 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    payload2.token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string();
    payload2.token_address = Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string());
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different tokens should produce different UUIDs"
    );
}

#[test]
fn test_signal_trade_uuid_differs_for_different_strategies() {
    // The dedup key includes the strategy, so a Shield vs Spear buy of the
    // same wallet/token/amount produces different UUIDs.
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Buy, Strategy::Spear, "1.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different strategies should produce different UUIDs"
    );
}

#[test]
fn test_signal_trade_uuid_preserves_provided_uuid() {
    // A provider-supplied trade_uuid is preserved verbatim.
    let mut payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    payload.trade_uuid = Some("custom-uuid-123".to_string());
    let signal = make_signal(payload, 1700000000);
    assert_eq!(signal.trade_uuid, "custom-uuid-123");
}

#[test]
fn test_exit_signal_validation() {
    // Exit strategy must have SELL action (models/signal.rs validate()).
    let exit = make_payload(Action::Sell, Strategy::Exit, "1.0");
    assert!(exit.validate().is_ok(), "SELL + Exit must validate");
    let invalid_exit = make_payload(Action::Buy, Strategy::Exit, "1.0");
    assert!(
        invalid_exit.validate().is_err(),
        "BUY + Exit must be rejected by validate()"
    );
}

#[test]
fn test_signal_amount_zero_rejected_by_validation() {
    // SignalPayload::validate() rejects amount_sol <= 0 — a zero amount is
    // NOT an acceptable signal.
    let payload = make_payload(Action::Buy, Strategy::Shield, "0.0");
    assert!(
        payload.validate().is_err(),
        "Zero amount must be rejected by validate()"
    );
}

#[test]
fn test_signal_positive_amount_validates() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.5");
    assert!(
        payload.validate().is_ok(),
        "Positive amount within limits must validate"
    );
}

#[test]
fn test_strategy_priority_ordering() {
    // EXIT > SHIELD > SPEAR (lower priority() value = higher priority)
    assert!(Strategy::Exit.priority() < Strategy::Shield.priority());
    assert!(Strategy::Shield.priority() < Strategy::Spear.priority());
}

#[test]
fn test_signal_different_timestamps_same_uuid() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal1 = make_signal(payload.clone(), 1700000000);
    let signal2 = make_signal(payload, 1700000100);
    assert_eq!(
        signal1.trade_uuid, signal2.trade_uuid,
        "Different timestamps should NOT change UUID (for dedup)"
    );
}

#[test]
fn test_signal_source_ip_preserved() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal = Signal::new(payload, 1700000000, Some("192.168.1.1".to_string()));
    assert_eq!(signal.source_ip, Some("192.168.1.1".to_string()));
}
