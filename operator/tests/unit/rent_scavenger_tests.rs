//! RentScavenger tests.
//!
//! Drives `reclaim_empty_accounts` against a local mock Solana JSON-RPC
//! server, covering the close pipeline, safety limits, re-verification, and
//! retry behavior. The scavenger uses a blocking `RpcClient`, so heavy RPC
//! work is offloaded with `spawn_blocking`.

use chimera_operator::engine::{RentScavenger, RentScavengerConfig};
use chimera_operator::metrics::RentScavengerMetrics;
use prometheus::Registry;
use serde_json::json;
use solana_sdk::signature::{Keypair, Signer};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

#[path = "../common/mock_rpc.rs"]
mod mock_rpc;

/// A parsed token account payload (UiAccountData::Json) with the fields the
/// scavenger's closability check reads.
fn token_account(
    pubkey: &str,
    amount: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut info = serde_json::Map::new();
    info.insert(
        "tokenAmount".to_string(),
        json!({"amount": amount, "decimals": 9, "uiAmount": null, "uiAmountString": amount}),
    );
    info.insert("isNative".to_string(), json!(false));
    info.insert(
        "mint".to_string(),
        json!("4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R"),
    );
    info.insert("owner".to_string(), json!("owner"));
    info.insert("state".to_string(), json!("initialized"));
    info.extend(extra);
    json!({
        "pubkey": pubkey,
        "account": {
            "data": {
                "parsed": {"info": info, "type": "account"},
                "program": "spl-token",
                "space": 165
            },
            "executable": false,
            "lamports": 2039280,
            "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            "rentEpoch": 0
        }
    })
}

fn closable(pubkey: &str, amount: &str) -> serde_json::Value {
    token_account(pubkey, amount, serde_json::Map::new())
}

/// Valid base58 wallet/pubkey addresses used as token account pubkeys.
const PUBKEYS: [&str; 8] = [
    "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "9HsFJKqobLFZ6QLT7xXhS3ggDfSGTJPUh2Rfug4VFGWh",
    "A6Wch1mJJ1PyooNSAUtctcNmQTxqtkcWManMBQPmKceM",
    "7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg",
    "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
    "8nx2iAtTuFVRL5bSo7hY7nCJExwFP6kpRq9SJfV9Z1Qk",
    "6m2cdhR9kY5Sx3JwQn4TvB8LqP2zNcW7HfE1aGpK4XyM",
];

const OWNER: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

fn default_config() -> RentScavengerConfig {
    RentScavengerConfig {
        enabled: true,
        interval_secs: 3600,
        max_batch_size: 10,
        max_rent_lamports: 1_000_000_000,
    }
}

fn scavenger(rpc_url: String, config: RentScavengerConfig) -> RentScavenger {
    let keypair = Arc::new(Keypair::new());
    RentScavenger::new(rpc_url, keypair, config, None)
}

fn scavenger_with_metrics(
    rpc_url: String,
    config: RentScavengerConfig,
) -> (RentScavenger, Arc<RentScavengerMetrics>) {
    let keypair = Arc::new(Keypair::new());
    let metrics = Arc::new(RentScavengerMetrics::new(&Registry::new()));
    (
        RentScavenger::new(rpc_url, keypair, config, Some(metrics.clone())),
        metrics,
    )
}

/// Base JSON-RPC mock handler that serves the RPC surface the scavenger
/// needs: getTokenAccountsByOwner (two fetches — the initial scan and the
/// re-verification snapshot), getMinimumBalanceForRentExemption,
/// getLatestBlockhash, sendTransaction, getSignatureStatuses.
fn close_flow_handler(
    accounts: Vec<serde_json::Value>,
    send_failures: usize,
    send_fail_count: Arc<AtomicUsize>,
) -> mock_rpc::RpcHandler {
    mock_rpc::rpc_handler(move |method, params| {
        let send_failures = send_failures;
        let send_fail_count = send_fail_count.clone();
        match method {
            "getTokenAccountsByOwner" => {
                // Only the legacy Token program scan returns accounts; the
                // Token-2022 scan returns none so accounts close exactly once.
                let is_token_2022 = params
                    .get(1)
                    .and_then(|f| f.get("programId"))
                    .and_then(|p| p.as_str())
                    .map(|p| p == "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
                    .unwrap_or(false);
                if is_token_2022 {
                    Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": []}))
                } else {
                    Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": accounts}))
                }
            }
            "getMinimumBalanceForRentExemption" => Some(json!(2039280u64)),
            "getLatestBlockhash" => Some(json!({
                "context": {"slot": 1},
                "value": {"blockhash": "11111111111111111111111111111111", "lastValidBlockHeight": 1}
            })),
            "sendTransaction" => {
                let failures = send_fail_count.load(Ordering::SeqCst);
                if failures < send_failures {
                    send_fail_count.fetch_add(1, Ordering::SeqCst);
                    Some(
                        json!({"error": {"code": -32005, "message": "Node is unhealthy: timeout"}}),
                    )
                } else {
                    mock_rpc::legacy_tx_signature_from_params(&params)
                        .map(|sig| json!(sig))
                        .or_else(|| Some(json!("5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2NygdhoC3LZf9YKkX4vN4w3hBqZ1kFZxYkqkU9kP2XKq")))
                }
            }
            "getSignatureStatuses" => Some(json!({
                "context": {"slot": 2},
                "value": [{"slot": 1, "confirmations": null, "err": null, "status": {"Ok": null}}]
            })),
            _ => None,
        }
    })
}

async fn run_scavenger(scav: &RentScavenger) {
    // The scavenger's RPC client is blocking; it briefly occupies a worker
    // thread, which is fine on the multi-thread test runtime.
    let _ = scav.reclaim_empty_accounts().await;
}

// ── Config validation ────────────────────────────────────────────────────────

#[test]
fn test_config_validate_clamps() {
    let mut config = RentScavengerConfig {
        enabled: true,
        interval_secs: 10,
        max_batch_size: 0,
        max_rent_lamports: 100,
    };
    config.validate();
    assert_eq!(config.interval_secs, 6 * 3600);
    assert_eq!(config.max_batch_size, 10);
    assert_eq!(config.max_rent_lamports, 1_000_000_000);

    let mut config2 = RentScavengerConfig {
        enabled: true,
        interval_secs: 7200,
        max_batch_size: 100,
        max_rent_lamports: 500_000_000,
    };
    config2.validate();
    assert_eq!(config2.interval_secs, 7200);
    assert_eq!(config2.max_batch_size, 10);
    assert_eq!(config2.max_rent_lamports, 500_000_000);
}

#[test]
fn test_config_validate_passes_valid() {
    let mut config = default_config();
    config.validate();
    assert_eq!(config.interval_secs, 3600);
    assert_eq!(config.max_batch_size, 10);
    assert_eq!(config.max_rent_lamports, 1_000_000_000);
}

#[test]
fn test_config_default_respects_env() {
    std::env::remove_var("RENT_SCAVENGER_ENABLED");
    assert!(!RentScavengerConfig::default().enabled);
    std::env::set_var("RENT_SCAVENGER_ENABLED", "true");
    assert!(RentScavengerConfig::default().enabled);
    std::env::set_var("RENT_SCAVENGER_ENABLED", "not-a-bool");
    assert!(!RentScavengerConfig::default().enabled);
    std::env::remove_var("RENT_SCAVENGER_ENABLED");
}

// ── Reclaim flows ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_no_accounts() {
    let (url, _server) = mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(|method, _| {
        if method == "getTokenAccountsByOwner" {
            Some(json!([]))
        } else {
            None
        }
    }))
    .await;
    let scav = scavenger(url, default_config());
    run_scavenger(&scav).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_skips_non_closable_accounts() {
    let keypair = Arc::new(Keypair::new());
    let owner_str = keypair.pubkey().to_string();
    let fail_count = Arc::new(AtomicUsize::new(0));
    let accounts = vec![
        // non-zero amount → not closable
        token_account(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "100",
            serde_json::Map::new(),
        ),
        // zero amount but delegate → not closable
        token_account(
            "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "0",
            serde_json::json!({"delegate": "9HsFJKqobLFZ6QLT7xXhS3ggDfSGTJPUh2Rfug4VFGWh"})
                .as_object()
                .unwrap()
                .clone(),
        ),
        // frozen → not closable
        token_account(
            "A6Wch1mJJ1PyooNSAUtctcNmQTxqtkcWManMBQPmKceM",
            "0",
            serde_json::json!({"state": "frozen"})
                .as_object()
                .unwrap()
                .clone(),
        ),
        // foreign close authority → not closable
        token_account(
            "7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg",
            "0",
            serde_json::json!({"closeAuthority": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin"})
                .as_object()
                .unwrap()
                .clone(),
        ),
        // Legacy (non-JSON) account data → skipped by the parsed-data branch
        json!({"pubkey": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin", "account": {
            "data": "base64encodeddata",
            "executable": false, "lamports": 2039280,
            "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", "rentEpoch": 0
        }}),
        // unparseable pubkey → skipped
        token_account("not-a-pubkey", "0", serde_json::Map::new()),
        // close authority = owner → closable, but space=0 falls back to the
        // 165-byte default rent lookup before closing.
        token_account(
            "8nx2iAtTuFVRL5bSo7hY7nCJExwFP6kpRq9SJfV9Z1Qk",
            "0",
            serde_json::json!({"closeAuthority": owner_str.clone()})
                .as_object()
                .unwrap()
                .clone(),
        ),
    ];
    let accounts_closure = accounts;
    let handler = mock_rpc::rpc_handler(move |method, params| match method {
        "getTokenAccountsByOwner" => {
            let is_token_2022 = params
                .get(1)
                .and_then(|f| f.get("programId"))
                .and_then(|p| p.as_str())
                .map(|p| p == "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
                .unwrap_or(false);
            if is_token_2022 {
                Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": []}))
            } else {
                Some(
                    json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": accounts_closure}),
                )
            }
        }
        "getMinimumBalanceForRentExemption" => Some(json!(2039280u64)),
        "getLatestBlockhash" => Some(json!({
            "context": {"slot": 1},
            "value": {"blockhash": "11111111111111111111111111111111", "lastValidBlockHeight": 1}
        })),
        "sendTransaction" => {
            mock_rpc::legacy_tx_signature_from_params(&params).map(|sig| json!(sig))
        }
        "getSignatureStatuses" => Some(json!({
            "context": {"slot": 2},
            "value": [{"slot": 1, "confirmations": null, "err": null, "status": {"Ok": null}}]
        })),
        _ => None,
    });
    let (url, _server) = mock_rpc::json_rpc_mock(handler).await;

    // Only the owner-closeAuthority account is closable; the space-0 fallback
    // still closes it (rent default 165). The rest are skipped.
    let metrics = Arc::new(RentScavengerMetrics::new(&Registry::new()));
    let scav = RentScavenger::new(url, keypair, default_config(), Some(metrics.clone()));
    run_scavenger(&scav).await;
    assert_eq!(
        metrics.accounts_closed_total.get(),
        1,
        "only the space-0 closable account closes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_space_zero_default_rent_lookup() {
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(
        vec![
            closable("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"),
            {
                let mut acct = token_account(
                    "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                    "0",
                    serde_json::Map::new(),
                );
                acct["account"]["data"]["space"] = serde_json::json!(0);
                acct
            },
        ],
        0,
        fail_count,
    ))
    .await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;
    assert_eq!(metrics.accounts_closed_total.get(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_closes_batch_with_metrics() {
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(
        vec![
            closable("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"),
            closable("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"),
        ],
        0,
        fail_count,
    ))
    .await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;

    // Both accounts closed in one batch.
    assert_eq!(metrics.accounts_closed_total.get(), 2);
    assert_eq!(metrics.rent_reclaimed_total.get(), 2 * 2039280);
    assert_eq!(metrics.errors_total.get(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_batch_failure_retries_per_account() {
    // First send fails transiently ("timeout" → retry_rpc retries once);
    // the batch close then falls back to per-account closes.
    let fail_count = Arc::new(AtomicUsize::new(1));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(
        vec![
            closable("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"),
            closable("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"),
        ],
        1,
        fail_count,
    ))
    .await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;

    assert_eq!(
        metrics.accounts_closed_total.get(),
        2,
        "per-account retry closes both"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_permanent_send_failure_closes_none() {
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(move |method, params| {
        let is_token_2022 = params
            .get(1)
            .and_then(|f| f.get("programId"))
            .and_then(|p| p.as_str())
            .map(|p| p == "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
            .unwrap_or(false);
        match method {
            "getTokenAccountsByOwner" if is_token_2022 => Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": []})),
            "getTokenAccountsByOwner" => Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": [closable("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0")]})),
            "getMinimumBalanceForRentExemption" => Some(json!(2039280u64)),
            "getLatestBlockhash" => Some(json!({
                "context": {"slot": 1},
                "value": {"blockhash": "11111111111111111111111111111111", "lastValidBlockHeight": 1}
            })),
            "sendTransaction" => {
                let _ = fail_count.load(Ordering::SeqCst);
                Some(json!({"error": {"code": -32602, "message": "invalid signature"}}))
            }
            _ => None,
        }
    }))
    .await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;
    assert_eq!(metrics.accounts_closed_total.get(), 0);
    assert_eq!(
        metrics.errors_total.get(),
        2,
        "batch + per-account errors counted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_rent_safety_limit_stops() {
    // Each account's rent (2,039,280 lamports) exceeds the 0.001 SOL limit.
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(
        vec![closable(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "0",
        )],
        0,
        fail_count,
    ))
    .await;
    let mut config = default_config();
    config.max_rent_lamports = 1_000_000;
    let scav = scavenger(url, config);
    run_scavenger(&scav).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_tx_too_large_falls_back_per_account() {
    // 20 accounts × ~90 bytes each exceeds the 1232-byte transaction limit,
    // so the batch send errors with a size guard and each account is closed
    // individually.
    let accounts: Vec<serde_json::Value> = PUBKEYS
        .iter()
        .cycle()
        .take(20)
        .map(|pk| closable(pk, "0"))
        .collect();
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(accounts, 0, fail_count)).await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;
    assert_eq!(
        metrics.accounts_closed_total.get(),
        20,
        "per-account fallback closes all 20"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_verification_drops_changed_accounts() {
    // First fetch returns two closable accounts; the re-verification snapshot
    // (second getTokenAccountsByOwner call) returns only one → the other is
    // dropped from the verified batch.
    let call = Arc::new(AtomicUsize::new(0));
    let fail_count = Arc::new(AtomicUsize::new(0));
    let call_clone = call.clone();
    let handler = mock_rpc::rpc_handler(move |method, params| match method {
        "getTokenAccountsByOwner" => {
            let is_token_2022 = params
                .get(1)
                .and_then(|f| f.get("programId"))
                .and_then(|p| p.as_str())
                .map(|p| p == "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
                .unwrap_or(false);
            if is_token_2022 {
                return Some(json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": []}));
            }
            let n = call_clone.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Some(
                    json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": [closable("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0"), closable("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0")]}),
                )
            } else {
                Some(
                    json!({"context": {"slot": 1, "apiVersion": "1.18.1"}, "value": [closable("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "0")]}),
                )
            }
        }
        "getMinimumBalanceForRentExemption" => Some(json!(2039280u64)),
        "getLatestBlockhash" => Some(json!({
            "context": {"slot": 1},
            "value": {"blockhash": "11111111111111111111111111111111", "lastValidBlockHeight": 1}
        })),
        "sendTransaction" => {
            mock_rpc::legacy_tx_signature_from_params(&params).map(|sig| json!(sig))
        }
        "getSignatureStatuses" => Some(json!({
            "context": {"slot": 2},
            "value": [{"slot": 1, "confirmations": null, "err": null, "status": {"Ok": null}}]
        })),
        _ => None,
    });
    let (url, _server) = mock_rpc::json_rpc_mock(handler).await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;
    assert_eq!(
        metrics.accounts_closed_total.get(),
        1,
        "only re-verified account closes"
    );
    assert!(call.load(Ordering::SeqCst) >= 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reclaim_rpc_error_increments_errors() {
    let (url, _server) = mock_rpc::json_rpc_mock(mock_rpc::rpc_handler(|method, _| {
        if method == "getTokenAccountsByOwner" {
            Some(json!({"error": {"code": -32005, "message": "Node unhealthy"}}))
        } else {
            None
        }
    }))
    .await;
    let (scav, metrics) = scavenger_with_metrics(url, default_config());
    run_scavenger(&scav).await;
    assert_eq!(metrics.errors_total.get(), 2, "one error per token program");
}

// ── start() ──────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_start_disabled_returns_ok() {
    let mut config = default_config();
    config.enabled = false;
    let scav = Arc::new(scavenger("http://127.0.0.1:1".to_string(), config));
    scav.start().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_runs_initial_cycle_and_ticks() {
    let fail_count = Arc::new(AtomicUsize::new(0));
    let (url, _server) = mock_rpc::json_rpc_mock(close_flow_handler(
        vec![closable(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "0",
        )],
        0,
        fail_count,
    ))
    .await;
    let mut config = default_config();
    config.interval_secs = 1;
    let scav = Arc::new(scavenger(url, config));
    scav.start().await.unwrap();
    // Initial run executes immediately; give the 1s tick a chance too.
    tokio::time::sleep(Duration::from_millis(2200)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_initial_run_failure_logs() {
    // RPC unreachable → the initial run fails inside the spawned task.
    let mut config = default_config();
    config.interval_secs = 1;
    let scav = Arc::new(scavenger("http://127.0.0.1:1".to_string(), config));
    scav.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
}
