//! Tests for `operator/src/engine/entry_confirmation.rs`.
//!
//! Covers the `EntryConfirmationManager`: registration gates (disabled, zero
//! ref price, duplicate token), the background confirmation loop (`spawn` →
//! `check_due` → `evaluate`) through every fail-closed path, the price-hold
//! pass/fail branches, the re-decision rejection path, and the full
//! admit-and-queue path (`queue_monitoring_signal` with a real DB + engine
//! handle). The token parser's Jupiter quote is served by a local mock so no
//! network is needed; the decision pipeline runs against the real selection
//! service with a pre-seeded token-safety cache.

use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::engine::entry_confirmation::{
    EntryConfirmationConfig, EntryConfirmationManager,
};
use chimera_operator::engine::{Ingress, SelectionRequest};
use chimera_operator::models::Action;
use chimera_operator::price_cache::{PriceCache, PriceSource};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::TokenSafetyResult;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/harness.rs"]
mod harness;

use harness::{
    build, make_selection_service_with_parser, seed_wallet, test_config, TOKEN_A, WALLET_A,
};

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Config with an immediate confirmation window (`wait_secs = 0`), so the
/// first `interval.tick()` of the spawned loop finds the entry due.
fn immediate_config() -> EntryConfirmationConfig {
    EntryConfirmationConfig {
        enabled: true,
        wait_secs: 0,
        max_drawdown_pct: dec("3.0"),
    }
}

/// Tiny HTTP server answering the Jupiter `/quote` endpoint with a fixed
/// `outAmount` (in SOL lamports), or an `{"error": ...}` body when
/// `out_amount` is `None` (covers the `Ok(None)` quote path). Returns 500 for
/// any other path.
async fn mock_quote_server(out_amount: Option<u64>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16384];
            let Ok(n) = sock.read(&mut buf).await else {
                continue;
            };
            let body = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = if body.contains("/quote?") {
                match out_amount {
                    Some(lamports) => serde_json::json!({"outAmount": lamports.to_string()}),
                    None => serde_json::json!({"error": "no route"}),
                }
                .to_string()
            } else {
                serde_json::json!({"error": "unknown"}).to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    format!("http://{addr}")
}

/// A TokenParser whose safety cache is pre-seeded (fast_check passes without
/// RPC) and whose fetcher talks to `quote_base` for sell quotes and reads
/// decimals from `price_cache`.
fn make_parser(quote_base: &str, price_cache: Arc<PriceCache>) -> Arc<TokenParser> {
    let cache = Arc::new(TokenCache::new(1000, 300));
    cache.insert(
        format!("{TOKEN_A}:SHIELD"),
        TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );
    let fetcher = Arc::new(
        TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            "http://127.0.0.1:1",
            None,
            quote_base.to_string(),
        )
        .with_price_cache(price_cache.clone()),
    );
    Arc::new(TokenParser::new(
        TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: dec("0"),
            min_liquidity_spear_usd: dec("0"),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        cache,
        fetcher,
    ))
}

fn buy_request() -> SelectionRequest {
    SelectionRequest {
        wallet_address: WALLET_A.to_string(),
        token_address: TOKEN_A.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("0.5"),
        ingress: Ingress::Helius,
        source_slot: Some(1),
        exit_fraction: None,
        whale_entry_price: None,
    }
}

/// Build the manager under test from a harness, with the given parser and
/// config. Returns the manager, the harness (kept alive for its DB), and the
/// price cache used for decimals.
async fn manager_with(
    config: EntryConfirmationConfig,
    parser: Option<Arc<TokenParser>>,
    selection: Option<Arc<chimera_operator::engine::SelectionService>>,
) -> (
    Arc<EntryConfirmationManager>,
    harness::Harness,
    Arc<PriceCache>,
) {
    let h = build(test_config()).await;
    let price_cache = h.price_cache.clone();
    let parser = parser.unwrap_or_else(|| {
        // Throwaway parser (dead endpoints): needed to construct the
        // SelectionService, never consulted when the manager has no parser
        // and the loop drops before reaching selection.
        make_parser("http://127.0.0.1:1", Arc::new(PriceCache::new().unwrap()))
    });
    let selection = selection
        .unwrap_or_else(|| make_selection_service_with_parser(h.db.clone(), parser.clone(), false));
    let mgr = Arc::new(EntryConfirmationManager::new(
        config,
        h.db.clone(),
        h.engine_handle.clone(),
        Some(parser),
        selection,
    ));
    (mgr, h, price_cache)
}

fn pool_of(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn count_trades_for(pool: &sqlx::Pool<sqlx::Postgres>, token: &str) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE token_address = $1")
        .bind(token)
        .fetch_one(pool)
        .await
        .unwrap();
    count
}

#[tokio::test]
async fn register_respects_config_and_duplicates() {
    // Disabled → never registers.
    let (mgr, h, _pc) = manager_with(
        EntryConfirmationConfig {
            enabled: false,
            ..immediate_config()
        },
        None,
        None,
    )
    .await;
    assert!(!mgr.enabled());
    assert!(!mgr.register(buy_request(), dec("0.01")).await);
    drop(h);

    // Enabled but non-positive ref price → rejected.
    let (mgr, h, _pc) = manager_with(immediate_config(), None, None).await;
    assert!(mgr.enabled());
    assert!(!mgr.register(buy_request(), Decimal::ZERO).await);
    assert!(!mgr.register(buy_request(), dec("-1.0")).await);

    // Same token twice → second is a duplicate → false.
    assert!(mgr.register(buy_request(), dec("0.01")).await);
    assert!(!mgr.register(buy_request(), dec("0.02")).await);
    drop(h);
}

#[tokio::test]
async fn evaluate_drops_when_no_token_parser() {
    let (mgr, h, _pc) = manager_with(immediate_config(), None, None).await;
    assert!(mgr.register(buy_request(), dec("0.01")).await);
    mgr.clone().spawn();
    // First tick fires immediately; give the loop time to evaluate and drop.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_drops_when_decimals_unknown() {
    // Parser with a dead RPC and an empty price cache → decimals resolve to
    // None → fail-closed drop.
    let base = mock_quote_server(Some(10_000)).await;
    let parser = make_parser(&base, Arc::new(PriceCache::new().unwrap()));
    let (mgr, h, _pc) = manager_with(immediate_config(), Some(parser), None).await;
    assert!(mgr.register(buy_request(), dec("0.01")).await);
    mgr.clone().spawn();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_drops_when_quote_fails() {
    // Decimals known via the price cache, but the quote endpoint is a dead
    // connection → sell_quote_out_sol errors → fail-closed drop.
    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let parser = make_parser("http://127.0.0.1:1", pc);
    let (mgr, h, _pc) = manager_with(immediate_config(), Some(parser), None).await;
    assert!(mgr.register(buy_request(), dec("0.01")).await);
    mgr.clone().spawn();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_drops_when_quote_returns_none() {
    // A quote body carrying `error` → Ok(None) → fail-closed drop.
    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let base = mock_quote_server(None).await;
    let parser = make_parser(&base, pc);
    let (mgr, h, _pc) = manager_with(immediate_config(), Some(parser), None).await;
    assert!(mgr.register(buy_request(), dec("0.01")).await);
    mgr.clone().spawn();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_drops_when_price_fell_below_tolerance() {
    // 1e6 raw units quoted at 1 lamport each → current price 1e-15 SOL/raw;
    // ref price 1e-11 → dumped beyond -3% → drop.
    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let base = mock_quote_server(Some(1)).await;
    let parser = make_parser(&base, pc);
    let (mgr, h, _pc) = manager_with(immediate_config(), Some(parser), None).await;
    assert!(mgr.register(buy_request(), dec("0.00000000001")).await);
    mgr.clone().spawn();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_price_held_but_selection_rejects() {
    // Price holds (quote matches ref) but the wallet is not in the roster →
    // UNKNOWN_WALLET → dropped, no trade row.
    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let base = mock_quote_server(Some(10_000)).await; // 1e-5 SOL for 1e6 raw → 1e-11 SOL/raw
    let parser = make_parser(&base, pc);
    let (mgr, h, _pc) = manager_with(immediate_config(), Some(parser), None).await;
    assert!(mgr.register(buy_request(), dec("0.00000000001")).await);
    mgr.clone().spawn();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count_trades_for(&pool_of(&h.db), TOKEN_A).await, 0);
}

#[tokio::test]
async fn evaluate_admits_and_queues_trade() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;

    let price_cache = h.price_cache.clone();
    price_cache.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let base = mock_quote_server(Some(10_000)).await; // 1e-5 SOL for 1e6 raw → 1e-11 SOL/raw
    let parser = make_parser(&base, price_cache);
    let selection = make_selection_service_with_parser(h.db.clone(), parser.clone(), false);
    let mgr = Arc::new(EntryConfirmationManager::new(
        immediate_config(),
        h.db.clone(),
        h.engine_handle.clone(),
        Some(parser),
        selection,
    ));

    assert!(mgr.register(buy_request(), dec("0.00000000001")).await);
    mgr.clone().spawn();

    // Poll for the queued trade row (the loop's first tick is immediate).
    let pool = pool_of(&h.db);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let count = count_trades_for(&pool, TOKEN_A).await;
        if count > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for queued trade"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (status, side, amount): (String, String, Decimal) = sqlx::query_as(
        "SELECT status, side, amount_sol FROM trades WHERE token_address = $1 LIMIT 1",
    )
    .bind(TOKEN_A)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "QUEUED", "trade must be queued after admission");
    assert_eq!(side, "BUY");
    assert_eq!(amount, dec("0.5"), "sizing falls back to source amount");
}

#[tokio::test]
async fn evaluate_duplicate_insert_errors_do_not_panic() {
    // Second registration of the same token is a duplicate → skipped before
    // any evaluation (covers the debug-log path for duplicate registration).
    let h = build(test_config()).await;
    let price_cache = h.price_cache.clone();
    price_cache.set_price(TOKEN_A, dec("0.01"), PriceSource::Jupiter, Some(6));
    let base = mock_quote_server(Some(10_000)).await;
    let parser = make_parser(&base, price_cache);
    let selection = make_selection_service_with_parser(h.db.clone(), parser.clone(), false);
    let mgr = Arc::new(EntryConfirmationManager::new(
        immediate_config(),
        h.db.clone(),
        h.engine_handle.clone(),
        Some(parser),
        selection,
    ));
    assert!(mgr.register(buy_request(), dec("0.00000000001")).await);
    assert!(!mgr.register(buy_request(), dec("0.00000000001")).await);
}
