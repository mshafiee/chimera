use chimera_operator::config::{resolve_trade_mode, TradeMode};
use chimera_operator::engine::executor::lamports_per_base_to_sol_per_token;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

fn paper_sell_amount(token_amount: u64, exit_fraction: Decimal) -> u64 {
    (Decimal::from(token_amount) * exit_fraction)
        .to_u64()
        .unwrap_or(0)
}

#[test]
fn test_resolve_trade_mode_explicit_wins() {
    assert_eq!(
        resolve_trade_mode(
            Some(TradeMode::Live),
            TradeMode::Live,
            "https://api.devnet.solana.com"
        ),
        TradeMode::Live
    );
    assert_eq!(
        resolve_trade_mode(
            Some(TradeMode::Paper),
            TradeMode::Live,
            "https://api.mainnet-beta.solana.com"
        ),
        TradeMode::Paper
    );
    assert_eq!(
        resolve_trade_mode(
            Some(TradeMode::Devnet),
            TradeMode::Live,
            "https://api.mainnet-beta.solana.com"
        ),
        TradeMode::Devnet
    );
}

#[test]
fn test_resolve_trade_mode_config_mode_wins_over_auto_detect() {
    assert_eq!(
        resolve_trade_mode(
            None,
            TradeMode::Paper,
            "https://api.mainnet-beta.solana.com"
        ),
        TradeMode::Paper
    );
    assert_eq!(
        resolve_trade_mode(
            None,
            TradeMode::Devnet,
            "https://api.mainnet-beta.solana.com"
        ),
        TradeMode::Devnet
    );
}

#[test]
fn test_resolve_trade_mode_devnet_auto_detect() {
    assert_eq!(
        resolve_trade_mode(None, TradeMode::Live, "https://api.devnet.solana.com"),
        TradeMode::Devnet
    );
    assert_eq!(
        resolve_trade_mode(None, TradeMode::Live, "https://rpc.ankr.com/solana_devnet"),
        TradeMode::Devnet
    );
}

#[test]
fn test_resolve_trade_mode_plain_url_is_live() {
    assert_eq!(
        resolve_trade_mode(None, TradeMode::Live, "https://api.mainnet-beta.solana.com"),
        TradeMode::Live
    );
    assert_eq!(
        resolve_trade_mode(None, TradeMode::Live, "https://my-rpc.com"),
        TradeMode::Live
    );
}

#[test]
fn test_trade_mode_display() {
    assert_eq!(format!("{}", TradeMode::Devnet), "DEVNET");
    assert_eq!(format!("{}", TradeMode::Paper), "PAPER");
    assert_eq!(format!("{}", TradeMode::Live), "LIVE");
}

#[test]
fn test_trade_mode_default() {
    assert_eq!(TradeMode::default(), TradeMode::Live);
}

#[test]
fn test_lamports_per_base_9_decimal() {
    let lamports = Decimal::ONE;
    let result = lamports_per_base_to_sol_per_token(lamports, Some(9)).unwrap();
    assert_eq!(result, Decimal::ONE);
}

#[test]
fn test_lamports_per_base_6_decimal() {
    let lamports = Decimal::ONE;
    let result = lamports_per_base_to_sol_per_token(lamports, Some(6)).unwrap();
    let expected = dec!(0.001);
    assert_eq!(result, expected);
}

#[test]
fn test_lamports_per_base_none_decimals() {
    let result = lamports_per_base_to_sol_per_token(Decimal::ONE, None);
    assert!(result.is_none());
}

#[test]
fn test_paper_sell_amount_full() {
    assert_eq!(paper_sell_amount(1_000_000, Decimal::ONE), 1_000_000);
}

#[test]
fn test_paper_sell_amount_half() {
    assert_eq!(paper_sell_amount(1_000_001, dec!(0.5)), 500_000);
}

#[test]
fn test_paper_sell_amount_zero() {
    assert_eq!(paper_sell_amount(1_000_000, Decimal::ZERO), 0);
}

#[test]
fn test_paper_sell_amount_large() {
    let amount = u64::MAX;
    assert_eq!(paper_sell_amount(amount, Decimal::ONE), amount);
}

#[test]
fn test_paper_sell_amount_clamping() {
    assert_eq!(paper_sell_amount(1, dec!(0.9999999)), 0);
}
