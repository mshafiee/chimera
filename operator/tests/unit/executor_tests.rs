//! Executor coverage tests (executor.rs) — paper mode, helpers, health checks,
//! fallback/recovery state machine, Jito health tracking, tip calculation.
//!
//! Live-mode submission paths (JSON-RPC/Jito/Helius mocks) live in
//! `executor_live_tests.rs`.

use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{Database, DbPool, InsertPosition, InsertTrade};
use chimera_operator::engine::executor::{
    convert_fill_price, derive_token_amount, enforce_price_impact_cap,
    executed_output_sol_for, lamports_per_base_to_sol_per_token, max_price_impact_pct,
    ExecutionOutcome, Executor, ExecutorError, JitoError, RpcMode,
};
use chimera_operator::engine::transaction_builder::BuiltTransaction;
use chimera_operator::engine::TipManager;
use chimera_operator::metrics::MetricsState;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use chimera_operator::notifications::{CompositeNotifier, NotificationEvent};
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

#[path = "../common/mock_rpc.rs"]
mod mock_rpc;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

/// Paper-mode config: RPC at a dead port (fails fast), jito disabled,
/// generous cost limits so the paper BUY path succeeds.
fn base_config(jupiter_url: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config.trade_mode = chimera_operator::config::TradeMode::Paper;
    config.rpc.primary_url = "http://127.0.0.1:1".to_string();
    config.rpc.timeout_ms = 500;
    config.jupiter.api_url = jupiter_url.to_string();
    config.jupiter.multi_dex_comparison = false;
    config.jupiter.use_swap_v2 = false;
    config.jito.enabled = false;
    config.jito.searcher_endpoint = None;
    config.jito.helius_fallback = false;
    config.jito.helius_staked_exits = false;
    config.strategy.min_position_sol = dec("0.01");
    config.strategy.max_position_sol = dec("2.0");
    config.strategy.dex_fee_rate = dec("0.001");
    config.strategy.shield_max_total_cost_percent = dec("0.05");
    config.strategy.spear_max_total_cost_percent = dec("0.08");
    config.position_sizing.min_live_position_sol = dec("0.05");
    config
}

fn make_signal(action: Action, strategy: Strategy, amount_sol: &str) -> Signal {
    Signal {
        trade_uuid: format!("t-{}", uuid::Uuid::new_v4()),
        payload: SignalPayload {
            strategy,
            token: "TEST".to_string(),
            token_address: Some("4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string()),
            action,
            amount_sol: dec(amount_sol),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        },
        timestamp: chrono::Utc::now().timestamp(),
        source_ip: Some("127.0.0.1".to_string()),
        liquidity_usd: Some(dec("100000")),
        force_slow_path: false,
        token_decimals: Some(9),
    }
}

fn default_quote(in_amount: u64, out_amount: u64, impact_pct: Option<&str>) -> serde_json::Value {
    let mut q = serde_json::json!({
        "inputMint": "So11111111111111111111111111111111111111112",
        "outputMint": "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
        "inAmount": in_amount.to_string(),
        "outAmount": out_amount.to_string(),
        "slippageBps": 100,
    });
    if let Some(p) = impact_pct {
        q["priceImpactPct"] = serde_json::json!(p);
    }
    q
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure helper coverage
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lamports_per_base_conversion() {
    // 9-decimal token: factor 1e9/1e9 = 1
    assert_eq!(
        lamports_per_base_to_sol_per_token(dec("2"), Some(9)),
        Some(dec("2"))
    );
    // 6-decimal token: factor 1e6/1e9 = 0.001
    assert_eq!(
        lamports_per_base_to_sol_per_token(dec("2000"), Some(6)),
        Some(dec("2"))
    );
    // Unknown decimals -> None
    assert_eq!(lamports_per_base_to_sol_per_token(dec("2"), None), None);
}

#[test]
fn test_convert_fill_price() {
    let uuid = "uuid-1";
    assert_eq!(
        convert_fill_price(Some(dec("2")), Some(9), uuid),
        Some(dec("2"))
    );
    assert_eq!(convert_fill_price(None, Some(9), uuid), None);
    assert_eq!(convert_fill_price(Some(dec("2")), None, uuid), None);
    assert_eq!(convert_fill_price(None, None, uuid), None);
}

#[test]
fn test_derive_token_amount_edges() {
    assert_eq!(derive_token_amount(dec("0.25"), Some(dec("0.001")), Some(9)), Some(250_000_000_000));
    assert_eq!(derive_token_amount(dec("1"), Some(dec("0.5")), Some(6)), Some(2_000_000));
    assert_eq!(derive_token_amount(dec("0.25"), None, Some(9)), None);
    assert_eq!(derive_token_amount(dec("0.25"), Some(dec("0")), Some(9)), None);
    assert_eq!(derive_token_amount(dec("0.25"), Some(dec("0.001")), None), None);
}

#[test]
fn test_enforce_price_impact_cap() {
    let signal = make_signal(Action::Buy, Strategy::Shield, "1.0");
    // BUY with impact above cap -> rejected pre-submission
    let err = enforce_price_impact_cap(&signal, Some(dec("3.5"))).unwrap_err();
    assert!(matches!(err, ExecutorError::TransactionFailed(_)));
    assert!(err.to_string().contains("thin liquidity"));
    // BUY with impact within cap -> ok
    assert!(enforce_price_impact_cap(&signal, Some(dec("1.5"))).is_ok());
    assert!(enforce_price_impact_cap(&signal, None).is_ok());
    // SELL exempt regardless of impact
    let sell = make_signal(Action::Sell, Strategy::Exit, "1.0");
    assert!(enforce_price_impact_cap(&sell, Some(dec("99.0"))).is_ok());
}

#[test]
fn test_max_price_impact_cap_value() {
    assert_eq!(max_price_impact_pct(), dec("2"));
}

#[test]
fn test_executed_output_sol_for() {
    let sell = make_signal(Action::Sell, Strategy::Exit, "1.0");
    let buy = make_signal(Action::Buy, Strategy::Shield, "1.0");
    let legacy = BuiltTransaction::Legacy {
        transaction: solana_sdk::transaction::Transaction::new_unsigned(
            solana_sdk::message::Message::default(),
        ),
        blockhash: solana_sdk::hash::Hash::default(),
        price_impact_pct: None,
        fill_price_lamports_per_base: None,
        route_fee_sol: None,
        out_amount: Some(5_000_000_000),
    };
    assert_eq!(
        executed_output_sol_for(&legacy, &sell),
        Some(dec("5"))
    );
    // Zero out amount -> None
    let zero_out = BuiltTransaction::Legacy {
        transaction: solana_sdk::transaction::Transaction::new_unsigned(
            solana_sdk::message::Message::default(),
        ),
        blockhash: solana_sdk::hash::Hash::default(),
        price_impact_pct: None,
        fill_price_lamports_per_base: None,
        route_fee_sol: None,
        out_amount: Some(0),
    };
    assert_eq!(executed_output_sol_for(&zero_out, &sell), None);
    // BUY -> None
    assert_eq!(executed_output_sol_for(&legacy, &buy), None);
    // Missing out amount -> None
    let no_out = BuiltTransaction::Versioned {
        transaction_bytes: vec![],
        blockhash: solana_sdk::hash::Hash::default(),
        price_impact_pct: None,
        fill_price_lamports_per_base: None,
        route_fee_sol: None,
        out_amount: None,
    };
    assert_eq!(executed_output_sol_for(&no_out, &sell), None);
}

#[test]
fn test_execution_outcome_live() {
    let outcome = ExecutionOutcome::live(
        "sig".to_string(),
        true,
        Some(dec("1.5")),
        Some(dec("0.5")),
        Some(dec("0.001")),
        Some(dec("2")),
    );
    assert_eq!(outcome.signature, "sig");
    assert!(outcome.confirmed);
    assert_eq!(outcome.fill_price_sol_per_token, Some(dec("1.5")));
    assert_eq!(outcome.price_impact_pct, Some(dec("0.5")));
    assert_eq!(outcome.route_fee_sol, Some(dec("0.001")));
    assert_eq!(outcome.executed_output_sol, Some(dec("2")));
    assert_eq!(outcome.token_amount, None);
    assert_eq!(outcome.estimated_fee_sol, None);
}

#[test]
fn test_calculate_retry_backoff_bounds() {
    // attempt 0..4: base 2^min(attempt,4) seconds, ±25% jitter, cap 30s
    for attempt in 0..5 {
        let d = Executor::calculate_retry_backoff(attempt);
        let ms = d.as_millis() as u64;
        assert!(ms >= 500, "attempt {attempt}: {ms}ms too low");
        assert!(ms <= 30_000, "attempt {attempt}: {ms}ms too high");
        // base floor: 2^attempt seconds (min 1s for attempt 0)
        let floor = (2u64.pow(attempt.min(4)) as f64 * 0.75 * 1000.0) as u64;
        assert!(ms >= floor);
    }
    // Cap at 30s even for huge attempts
    assert!(Executor::calculate_retry_backoff(100).as_millis() <= 30_000);
}

#[test]
fn test_is_rate_limit_error() {
    assert!(Executor::is_rate_limit_error("rate limit exceeded"));
    assert!(Executor::is_rate_limit_error("HTTP 429"));
    assert!(Executor::is_rate_limit_error("too many requests"));
    assert!(Executor::is_rate_limit_error("ratelimit"));
    assert!(Executor::is_rate_limit_error("rate-limit"));
    assert!(Executor::is_rate_limit_error("RATE LIMIT"));
    assert!(!Executor::is_rate_limit_error("blockhash not found"));
    assert!(!Executor::is_rate_limit_error(""));
}

#[test]
fn test_validate_transaction_size() {
    let (db, _guard) = futures::executor::block_on(create_test_db());
    let config = Arc::new(base_config("http://127.0.0.1:1"));
    let executor = Executor::new(config, db);
    // Small tx passes
    assert!(executor.validate_transaction_size(&[0u8; 100]).is_ok());
    // Exactly at limit passes
    assert!(executor.validate_transaction_size(&[0u8; 1232]).is_ok());
    // Over the limit fails
    assert!(executor.validate_transaction_size(&[0u8; 1233]).is_err());
}