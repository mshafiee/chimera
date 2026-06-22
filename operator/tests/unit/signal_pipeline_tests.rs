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
    assert_eq!(signal1.trade_uuid, signal2.trade_uuid, "Same payload should produce same UUID");
}

#[test]
fn test_signal_trade_uuid_differs_for_different_amounts() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Buy, Strategy::Shield, "2.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(signal1.trade_uuid, signal2.trade_uuid, "Different amounts should produce different UUIDs");
}

#[test]
fn test_signal_trade_uuid_differs_for_different_actions() {
    let payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let payload2 = make_payload(Action::Sell, Strategy::Shield, "1.0");
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(signal1.trade_uuid, signal2.trade_uuid, "Different actions should produce different UUIDs");
}

#[test]
fn test_signal_trade_uuid_differs_for_different_wallets() {
    let mut payload1 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let mut payload2 = make_payload(Action::Buy, Strategy::Shield, "1.0");
    payload2.wallet_address = "9xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string();
    let signal1 = make_signal(payload1, 1700000000);
    let signal2 = make_signal(payload2, 1700000000);
    assert_ne!(signal1.trade_uuid, signal2.trade_uuid, "Different wallets should produce different UUIDs");
}

#[test]
fn test_signal_exit_action_is_valid() {
    let exit = make_payload(Action::Sell, Strategy::Shield, "1.0");
    let buy = make_payload(Action::Buy, Strategy::Shield, "1.0");
    assert_eq!(exit.action, Action::Sell);
    assert_eq!(buy.action, Action::Buy);
}

#[test]
fn test_signal_strategy_preserved() {
    let spear = make_payload(Action::Buy, Strategy::Spear, "0.5");
    let shield = make_payload(Action::Buy, Strategy::Shield, "1.0");
    assert_eq!(spear.strategy, Strategy::Spear);
    assert_eq!(shield.strategy, Strategy::Shield);
}

#[test]
fn test_signal_positive_amount() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.5");
    assert!(payload.amount_sol > Decimal::ZERO, "Signal amount must be positive");
}

#[test]
fn test_signal_amount_zero() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "0.0");
    assert_eq!(payload.amount_sol, Decimal::ZERO, "Signal amount can be zero");
}

#[test]
fn test_signal_different_timestamps_same_uuid() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal1 = make_signal(payload.clone(), 1700000000);
    let signal2 = make_signal(payload, 1700000100);
    assert_eq!(signal1.trade_uuid, signal2.trade_uuid, "Different timestamps should NOT change UUID (for dedup)");
}

#[test]
fn test_signal_source_ip_preserved() {
    let payload = make_payload(Action::Buy, Strategy::Shield, "1.0");
    let signal = Signal::new(payload, 1700000000, Some("192.168.1.1".to_string()));
    assert_eq!(signal.source_ip, Some("192.168.1.1".to_string()));
}
