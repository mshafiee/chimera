//! Smart RPC polling fallback for transaction monitoring
//!
//! Used when webhooks fail or for validation. Implements signature caching
//! and prioritized polling to minimize credit usage.

use crate::db_abstraction::Database;
use crate::monitoring::rate_limiter::{RateLimiter, RequestPriority, RpcMethodCategory};
use crate::monitoring::transaction_parser;
use crate::token::is_non_speculative;
use anyhow::{Context, Result};
use lru::LruCache;
use rust_decimal::Decimal;
use serde_json::Value;
use solana_client::client_error::ClientError;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Transaction information from polling
#[derive(Debug, Clone)]
pub struct WalletTransaction {
    pub wallet_address: String,
    pub signature: String,
    pub token_address: Option<String>,
    pub direction: Option<String>, // BUY or SELL
    pub amount_sol: Option<Decimal>,
    pub timestamp: i64,
}

pub struct RpcPollingState {
    // Changed from HashSet to LruCache
    seen_signatures: Arc<tokio::sync::RwLock<LruCache<String, ()>>>,
    last_poll: Arc<tokio::sync::RwLock<std::collections::HashMap<String, SystemTime>>>,
}

impl Default for RpcPollingState {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcPollingState {
    pub fn new() -> Self {
        Self {
            // Cap at 10,000 signatures
            seen_signatures: Arc::new(tokio::sync::RwLock::new(LruCache::new(
                NonZeroUsize::new(10000).expect("Cache capacity must be > 0"),
            ))),
            last_poll: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    pub async fn has_seen(&self, signature: &str) -> bool {
        let seen = self.seen_signatures.read().await;
        seen.contains(signature)
    }

    pub async fn mark_seen(&self, signature: String) {
        let mut seen = self.seen_signatures.write().await;
        seen.put(signature, ());
        // No manual cleanup needed; LruCache handles it automatically
    }

    /// Update last poll time for wallet
    pub async fn update_last_poll(&self, wallet: &str) {
        let mut last_poll = self.last_poll.write().await;
        last_poll.insert(wallet.to_string(), SystemTime::now());
    }

    /// Check if wallet needs polling (based on interval)
    pub async fn should_poll(&self, wallet: &str, interval_secs: u64) -> bool {
        let last_poll = self.last_poll.read().await;
        if let Some(&last) = last_poll.get(wallet) {
            if let Ok(elapsed) = last.elapsed() {
                return elapsed.as_secs() >= interval_secs;
            }
        }
        true // Never polled, should poll
    }
}

/// Check if an RPC error is the "filter transaction not found" error (-32020)
///
/// This error occurs in Solana 4.0+ when using getSignaturesForAddress with a
/// `before` or `until` parameter that references a signature not found in the
/// transaction history (expired, too old, or invalid).
fn is_filter_transaction_not_found_error(error: &ClientError) -> bool {
    // Check the error message for the specific error code
    // This approach is version-agnostic and works across Solana versions
    error.to_string().contains("-32020")
}

/// Poll wallet transactions using RPC
/// # Arguments
/// * `rpc_client` - Solana RPC client
/// * `wallet_address` - Wallet to poll
/// * `last_signature` - Last known signature (to get new transactions)
/// * `rate_limiter` - Rate limiter
/// * `db` - Database pool (optional, for updating last signature)
pub async fn poll_wallet_transactions(
    rpc_client: &RpcClient,
    wallet_address: &str,
    last_signature: Option<&str>,
    rate_limiter: Arc<RateLimiter>,
    db: Option<&dyn Database>,
) -> Result<Vec<WalletTransaction>> {
    // Rate limit before polling (account query for signature lookup)
    rate_limiter
        .acquire_rpc(RpcMethodCategory::AccountQuery, RequestPriority::Polling)
        .await;

    // Paginated fetch: signatures are returned newest-first. Walk pages of 25 until
    // the anchor (last_signature) is found or we exhaust 50 pages (1250 signatures max).
    const PAGE_SIZE: usize = 25;
    const MAX_PAGES: usize = 50;

    let pubkey = wallet_address.parse().context("Invalid wallet address")?;

    let mut new_signatures: Vec<String> = Vec::new();
    let mut anchor_found = false;
    let mut before_sig: Option<String> = None;

    'pages: for _page in 0..MAX_PAGES {
        // Rate limit for getSignaturesForAddress (account query)
        rate_limiter
            .acquire_rpc(RpcMethodCategory::AccountQuery, RequestPriority::Polling)
            .await;

        let config = solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config {
            before: before_sig
                .as_deref()
                .and_then(|s| s.parse::<solana_sdk::signature::Signature>().ok()),
            limit: Some(PAGE_SIZE),
            ..Default::default()
        };

        // Handle Solana 4.0 RPC error -32020 (signature not found)
        let page = match rpc_client
            .get_signatures_for_address_with_config(&pubkey, config)
            .await
        {
            Ok(page) => page,
            Err(e) => {
                // In Solana 4.0, if the before/until signature is not found, we get error -32020
                // Treat this as "no more signatures" (equivalent to previous empty array behavior)
                if is_filter_transaction_not_found_error(&e) {
                    tracing::debug!(
                        wallet = %wallet_address,
                        before = ?before_sig,
                        "Filter signature not found (Solana 4.0 RPC error -32020), treating as no more signatures"
                    );
                    break;
                }
                // For other errors, propagate them
                return Err(anyhow::Error::from(e).context("Failed to get signatures"));
            }
        };

        if page.is_empty() {
            break;
        }

        for sig_info in &page {
            if let Some(last) = last_signature {
                if sig_info.signature == last {
                    anchor_found = true;
                    break 'pages; // Everything collected so far is newer than anchor
                }
            }
            new_signatures.push(sig_info.signature.clone());
        }

        // Set the cursor to the oldest signature in this page for the next iteration
        before_sig = page.last().map(|s| s.signature.clone());

        if page.len() < PAGE_SIZE {
            // No more pages available
            break;
        }
    }

    if last_signature.is_some() && !anchor_found && !new_signatures.is_empty() {
        tracing::warn!(
            wallet = wallet_address,
            count = new_signatures.len(),
            "Anchor signature not found in {} transactions, possible gap in signal detection",
            new_signatures.len()
        );
    }

    // Parse transactions (limited to save credits)
    let mut transactions = Vec::new();
    let mut latest_signature: Option<String> = None;

    for sig_str in new_signatures.iter().take(15) {
        // Limit to 15 transactions per poll (was 5)
        // Rate limit for getTransaction (transaction fetch - heavy operation)
        // Use Polling priority since this is background operation, not time-sensitive
        rate_limiter
            .acquire_rpc(
                RpcMethodCategory::TransactionFetch,
                RequestPriority::Polling,
            )
            .await;

        // Parse signature string to Signature type
        if let Ok(sig) = sig_str.parse::<solana_sdk::signature::Signature>() {
            let tx_config = solana_client::rpc_config::RpcTransactionConfig {
                encoding: Some(solana_transaction_status::UiTransactionEncoding::Json),
                max_supported_transaction_version: Some(0),
                ..Default::default()
            };
            let tx_result = crate::metrics::timed_rpc(
                "polling",
                "getTransaction",
                rpc_client.get_transaction_with_config(&sig, tx_config),
            )
            .await;
            match tx_result {
                Ok(tx) => {
                    // Convert UiTransaction to JSON Value for parser
                    let tx_json: Value = serde_json::to_value(&tx)
                        .context("Failed to serialize transaction to JSON")?;

                    // Parse transaction to extract swap info using transaction_parser
                    match transaction_parser::parse_transaction(&tx_json, wallet_address) {
                        Ok(tx_info) => {
                            if let Some(swap) = tx_info.parsed_swap {
                                // Extract token address and direction from parsed swap
                                let token_address =
                                    if swap.direction == transaction_parser::SwapDirection::Buy {
                                        Some(swap.token_out.clone())
                                    } else {
                                        Some(swap.token_in.clone())
                                    };

                                let direction = match swap.direction {
                                    transaction_parser::SwapDirection::Buy => {
                                        Some("BUY".to_string())
                                    }
                                    transaction_parser::SwapDirection::Sell => {
                                        Some("SELL".to_string())
                                    }
                                };

                                // Calculate SOL amount (amount_in for BUY, amount_out for SELL)
                                let sol_mint = "So11111111111111111111111111111111111111112";
                                let amount_sol =
                                    if swap.direction == transaction_parser::SwapDirection::Buy {
                                        // Buying: amount_in is SOL
                                        if swap.token_in == sol_mint {
                                            Some(swap.amount_in)
                                        } else {
                                            Some(swap.amount_out) // Fallback
                                        }
                                    } else {
                                        // Selling: amount_out is SOL
                                        if swap.token_out == sol_mint {
                                            Some(swap.amount_out)
                                        } else {
                                            Some(swap.amount_in) // Fallback
                                        }
                                    };

                                tracing::debug!(
                                    wallet = wallet_address,
                                    signature = sig_str,
                                    direction = ?direction,
                                    token = ?token_address,
                                    amount_sol = ?amount_sol,
                                    "Parsed swap transaction from RPC polling"
                                );

                                // Record speculative activity for inactivity tracking
                                if let Some(db_ref) = db {
                                    if let Some(token_str) = token_address.as_deref() {
                                        if !token_str.is_empty() && !is_non_speculative(token_str) {
                                            if let Err(e) = db_ref
                                                .update_last_speculative_signal(
                                                    wallet_address,
                                                    chrono::Utc::now(),
                                                )
                                                .await
                                            {
                                                tracing::warn!(
                                                    wallet = %wallet_address,
                                                    token = %token_str,
                                                    error = %e,
                                                    "Failed to update last speculative signal timestamp"
                                                );
                                            }
                                        }
                                    }
                                }

                                transactions.push(WalletTransaction {
                                    wallet_address: wallet_address.to_string(),
                                    signature: sig_str.clone(),
                                    token_address,
                                    direction,
                                    amount_sol,
                                    timestamp: tx.block_time.unwrap_or(0),
                                });
                            } else {
                                // Not a swap transaction, skip
                                tracing::trace!(
                                    wallet = wallet_address,
                                    signature = sig_str,
                                    "Transaction is not a swap, skipping"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                wallet = wallet_address,
                                signature = sig_str,
                                error = %e,
                                "Failed to parse transaction"
                            );
                        }
                    }

                    // Track latest signature for database update
                    if latest_signature.is_none() {
                        latest_signature = Some(sig_str.clone());
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        wallet = wallet_address,
                        signature = sig_str,
                        error = %e,
                        "Failed to fetch transaction during polling"
                    );
                }
            }
        }
    }

    // Update last signature in database if we have new transactions and database access
    if let (Some(latest_sig), Some(db_pool)) = (latest_signature, db) {
        if let Err(e) = db_pool
            .update_wallet_monitoring_signature(wallet_address, &latest_sig)
            .await
        {
            tracing::warn!(
                wallet = wallet_address,
                error = %e,
                "Failed to update last transaction signature in database"
            );
        }
    }

    Ok(transactions)
}

/// Batch poll multiple wallets with spacing
pub async fn poll_wallets_batch(
    rpc_client: &RpcClient,
    wallets: &[String],
    interval_secs: u64,
    batch_size: usize,
    rate_limiter: Arc<RateLimiter>,
    polling_state: Arc<RpcPollingState>,
    db: Option<&dyn Database>,
) -> Result<Vec<WalletTransaction>> {
    let mut all_transactions = Vec::new();

    for chunk in wallets.chunks(batch_size) {
        let mut chunk_transactions = Vec::new();

        for wallet in chunk {
            // Check if we should poll this wallet
            if !polling_state.should_poll(wallet, interval_secs).await {
                continue;
            }

            // Get last signature from database if available
            // Store in a variable to extend lifetime
            let last_sig_opt = if let Some(db_pool) = db {
                match db_pool.get_wallet_monitoring(wallet).await {
                    Ok(Some(monitoring)) => monitoring.last_transaction_signature.clone(),
                    _ => None,
                }
            } else {
                None
            };
            let last_signature = last_sig_opt.as_deref();

            // Poll wallet
            if let Ok(txs) = poll_wallet_transactions(
                rpc_client,
                wallet,
                last_signature,
                rate_limiter.clone(),
                db,
            )
            .await
            {
                // Filter out already-seen transactions
                for tx in txs {
                    if !polling_state.has_seen(&tx.signature).await {
                        polling_state.mark_seen(tx.signature.clone()).await;
                        chunk_transactions.push(tx);
                    }
                }

                polling_state.update_last_poll(wallet).await;
            }

            // Small delay between wallets in batch
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        all_transactions.extend(chunk_transactions);

        // Delay between batches
        if wallets.len() > batch_size {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    Ok(all_transactions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn rate_limiter() -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(1000, 1))
    }

    // ==========================================================================
    // RPC POLLING STATE
    // ==========================================================================

    #[tokio::test]
    async fn test_polling_state_seen_signatures() {
        let state = RpcPollingState::new();
        assert!(!state.has_seen("sig-1").await);
        state.mark_seen("sig-1".to_string()).await;
        assert!(state.has_seen("sig-1").await);
        // LruCache caps at 10k entries; repeated marks are idempotent.
        for i in 0..100 {
            state.mark_seen(format!("sig-{i}")).await;
        }
        assert!(state.has_seen("sig-99").await);
        assert!(state.has_seen("sig-1").await);
    }

    #[tokio::test]
    async fn test_polling_state_should_poll() {
        let state = RpcPollingState::new();
        // Never polled → should poll.
        assert!(state.should_poll("wallet-a", 60).await);

        // Polled just now → NOT due for 60s.
        state.update_last_poll("wallet-a").await;
        assert!(!state.should_poll("wallet-a", 60).await);

        // A short interval is due immediately (elapsed >= 0 seconds).
        assert!(state.should_poll("wallet-a", 0).await);

        // Different wallet unaffected.
        assert!(state.should_poll("wallet-b", 60).await);
    }

    #[test]
    fn test_filter_transaction_not_found_error() {
        use solana_client::client_error::{ClientError, ClientErrorKind};

        fn err(msg: &str) -> ClientError {
            ClientError {
                request: None,
                kind: Box::new(ClientErrorKind::Custom(msg.to_string())),
            }
        }

        let not_found = err("-32020: Transaction history is empty or the signature was not found");
        assert!(is_filter_transaction_not_found_error(&not_found));

        let other = err("-32009: Something else");
        assert!(!is_filter_transaction_not_found_error(&other));

        let io_err = err("connection refused");
        assert!(!is_filter_transaction_not_found_error(&io_err));
    }

    // ==========================================================================
    // MOCK JSON-RPC SERVER
    // ==========================================================================

    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
    const TOKEN_MINT: &str = "Token1111111111111111111111111111111111111";
    const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    const JUPITER_PROGRAM: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

    /// A valid 64-byte base58 signature.
    fn sig(n: u8) -> String {
        bs58::encode([n; 64]).into_string()
    }

    /// getSignaturesForAddress page with `count` signatures.
    fn signatures_body(count: usize) -> String {
        let sigs: Vec<serde_json::Value> = (0..count)
            .map(|i| {
                serde_json::json!({
                    "signature": sig(i as u8 + 1),
                    "slot": 100 + i,
                    "err": null,
                    "memo": null,
                    "blockTime": 1700000000 + i as i64,
                })
            })
            .collect();
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": sigs}).to_string()
    }

    /// A Jupiter swap transaction (BUY: spend 1 SOL, receive 100 TOKEN) that
    /// `parse_transaction` recognizes.
    fn tx_body() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "slot": 100,
                "transaction": {
                    "signatures": [sig(1)],
                    "message": {
                        "header": {
                            "numRequiredSignatures": 1,
                            "numReadonlySignedAccounts": 0,
                            "numReadonlyUnsignedAccounts": 0
                        },
                        "accountKeys": [
                            {"pubkey": "wallet", "writable": true, "signer": true},
                            {"pubkey": "token-ata", "writable": true, "signer": false}
                        ],
                        "recentBlockhash": "11111111111111111111111111111111",
                        "instructions": [
                            {"program": "jupiter", "programId": JUPITER_PROGRAM, "parsed": {}, "stackHeight": null}
                        ]
                    }
                },
                "meta": {
                    "err": null,
                    "status": {"Ok": null},
                    "fee": 5000,
                    "preBalances": [10000000000_i64, 0],
                    "postBalances": [9000000000_i64, 0],
                    "preTokenBalances": [
                        {"accountIndex": 0, "mint": SOL_MINT, "uiTokenAmount": {"uiAmount": 10.0, "uiAmountString": "10.0", "decimals": 9, "amount": "10000000000"}},
                        {"accountIndex": 1, "mint": TOKEN_MINT, "uiTokenAmount": {"uiAmount": 0.0, "uiAmountString": "0", "decimals": 9, "amount": "0"}}
                    ],
                    "postTokenBalances": [
                        {"accountIndex": 0, "mint": SOL_MINT, "uiTokenAmount": {"uiAmount": 9.0, "uiAmountString": "9.0", "decimals": 9, "amount": "9000000000"}},
                        {"accountIndex": 1, "mint": TOKEN_MINT, "uiTokenAmount": {"uiAmount": 100.0, "uiAmountString": "100.0", "decimals": 9, "amount": "100000000000"}}
                    ],
                    "innerInstructions": [],
                    "logMessages": [],
                    "rewards": []
                },
                "blockTime": 1700000123
            }
        })
        .to_string()
    }

    fn error_body(code: i64) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": code, "message": "mock error"}
        })
        .to_string()
    }

    /// Raw TCP server that answers JSON-RPC by method name.
    async fn mock_rpc<F>(handler: F) -> String
    where
        F: FnMut(&str) -> String + Send + 'static,
    {
        use std::sync::Mutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(Mutex::new(handler));
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 32768];
                let Ok(n) = sock.read(&mut buf).await else {
                    continue;
                };
                let body = String::from_utf8_lossy(&buf[..n]).to_string();
                let response = handler.lock().unwrap()(&body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_parses_buy() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(1)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));

        let txs = poll_wallet_transactions(&client, WALLET, None, rate_limiter(), None)
            .await
            .expect("poll succeeds");

        assert_eq!(txs.len(), 1);
        let tx = &txs[0];
        assert_eq!(tx.wallet_address, WALLET);
        assert_eq!(tx.direction.as_deref(), Some("BUY"));
        assert_eq!(tx.token_address.as_deref(), Some(TOKEN_MINT));
        // BUY with token_in == SOL → amount_in (1.0 SOL).
        assert_eq!(tx.amount_sol, Some(Decimal::ONE));
        assert_eq!(tx.timestamp, 1700000123);
        assert_eq!(tx.signature, sig(1));
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_anchor_stops_pagination() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(3)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));

        // Anchor at sig(2): everything collected before it (sig1) is newer.
        let txs = poll_wallet_transactions(&client, WALLET, Some(&sig(2)), rate_limiter(), None)
            .await
            .expect("poll succeeds");
        assert_eq!(txs.len(), 1, "only signatures newer than the anchor");
        assert_eq!(txs[0].signature, sig(1));
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_filter_error_breaks() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                error_body(-32020) // Solana 4.0 "signature not found"
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));

        let txs = poll_wallet_transactions(&client, WALLET, Some("old-sig"), rate_limiter(), None)
            .await
            .expect("-32020 is treated as end of history");
        assert!(txs.is_empty());
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_generic_rpc_error_propagates() {
        let url = mock_rpc(|_body| error_body(-32009)).await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));

        let result = poll_wallet_transactions(&client, WALLET, None, rate_limiter(), None).await;
        assert!(result.is_err(), "non-filter RPC errors propagate");
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_invalid_address() {
        let url = mock_rpc(|_body| signatures_body(1)).await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let result =
            poll_wallet_transactions(&client, "not-a-pubkey", None, rate_limiter(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_get_transaction_failure_skips() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(2)
            } else if body.contains("getTransaction") {
                error_body(-32009)
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));

        // Failed getTransaction calls are logged and skipped, not fatal.
        let txs = poll_wallet_transactions(&client, WALLET, None, rate_limiter(), None)
            .await
            .expect("poll succeeds");
        assert!(txs.is_empty());
    }

    #[tokio::test]
    async fn test_polling_state_default() {
        let state = RpcPollingState::default();
        assert!(!state.has_seen("x").await);
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_empty_page_breaks() {
        let url = mock_rpc(|_body| {
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": []}).to_string()
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let txs = poll_wallet_transactions(&client, WALLET, None, rate_limiter(), None)
            .await
            .expect("empty page is not an error");
        assert!(txs.is_empty());
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_anchor_not_found_warns() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(2)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        // Anchor signature that never appears → gap warning path, all
        // collected signatures still returned.
        let txs =
            poll_wallet_transactions(&client, WALLET, Some("not-in-page"), rate_limiter(), None)
                .await
                .unwrap();
        assert_eq!(txs.len(), 2);
    }

    /// SELL swap body: token → SOL, amount_out = SOL received.
    fn sell_tx_body() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "slot": 100,
                "transaction": {
                    "signatures": [sig(1)],
                    "message": {
                        "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
                        "accountKeys": [
                            {"pubkey": WALLET, "writable": true, "signer": true},
                            {"pubkey": "token-ata", "writable": true, "signer": false}
                        ],
                        "recentBlockhash": "11111111111111111111111111111111",
                        "instructions": [{"program": "jupiter", "programId": JUPITER_PROGRAM, "parsed": {}, "stackHeight": null}]
                    }
                },
                "meta": {
                    "err": null, "status": {"Ok": null}, "fee": 5000,
                    "preBalances": [10000000000_i64, 0], "postBalances": [11000000000_i64, 0],
                    "preTokenBalances": [
                        {"accountIndex": 0, "mint": SOL_MINT, "uiTokenAmount": {"uiAmount": 10.0, "uiAmountString": "10.0", "decimals": 9, "amount": "0"}},
                        {"accountIndex": 1, "mint": TOKEN_MINT, "uiTokenAmount": {"uiAmount": 100.0, "uiAmountString": "100.0", "decimals": 9, "amount": "0"}}
                    ],
                    "postTokenBalances": [
                        {"accountIndex": 0, "mint": SOL_MINT, "uiTokenAmount": {"uiAmount": 11.0, "uiAmountString": "11.0", "decimals": 9, "amount": "0"}},
                        {"accountIndex": 1, "mint": TOKEN_MINT, "uiTokenAmount": {"uiAmount": 0.0, "uiAmountString": "0.0", "decimals": 9, "amount": "0"}}
                    ],
                    "innerInstructions": [], "logMessages": [], "rewards": []
                },
                "blockTime": 1700000123
            }
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_poll_wallet_transactions_sell_and_quote_fallbacks() {
        // SELL: token_out == SOL → amount = amount_out (1 SOL).
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(1)
            } else if body.contains("getTransaction") {
                sell_tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let txs = poll_wallet_transactions(&client, WALLET, None, rate_limiter(), None)
            .await
            .unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].direction.as_deref(), Some("SELL"));
        assert_eq!(txs[0].token_address.as_deref(), Some(TOKEN_MINT));
        assert_eq!(txs[0].amount_sol, Some(Decimal::ONE));

        // NOTE: the BUY/SELL amount fallback branches (token_in != SOL /
        // token_out != SOL) are unreachable: parse_transaction always resolves
        // the SOL leg for SOL-quoted swaps, so the quote mint is always SOL.
    }

    #[tokio::test]
    async fn test_poll_with_db_speculative_and_signature_errors() {
        // db wired: speculative update fails → warn; signature update fails → warn.
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        db.speculative_error.store(true, Ordering::Relaxed);
        db.signature_error.store(true, Ordering::Relaxed);
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(1)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let txs =
            poll_wallet_transactions(&client, WALLET, None, rate_limiter(), Some(db.as_ref()))
                .await
                .unwrap();
        assert_eq!(txs.len(), 1, "warnings are non-fatal");
    }

    #[tokio::test]
    async fn test_poll_wallets_batch_monitoring_lookup_error() {
        // get_wallet_monitoring fails → last_signature None → still polls.
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        db.monitoring_error.store(true, Ordering::Relaxed);
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(1)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let txs = poll_wallets_batch(
            &client,
            &[WALLET.to_string()],
            60,
            1,
            rate_limiter(),
            Arc::new(RpcPollingState::new()),
            Some(db.as_ref()),
        )
        .await
        .unwrap();
        assert_eq!(txs.len(), 1);
    }

    #[tokio::test]
    async fn test_poll_wallets_batch_dedupes_and_spaces() {
        let url = mock_rpc(|body| {
            if body.contains("getSignaturesForAddress") {
                signatures_body(2)
            } else if body.contains("getTransaction") {
                tx_body()
            } else {
                error_body(-32601)
            }
        })
        .await;
        let client = RpcClient::new_with_timeout(url, Duration::from_secs(5));
        let state = Arc::new(RpcPollingState::new());

        let txs = poll_wallets_batch(
            &client,
            &[
                WALLET.to_string(),
                "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            ],
            60,
            1,
            rate_limiter(),
            state.clone(),
            None,
        )
        .await
        .expect("batch succeeds");

        // Each wallet yields one BUY transaction (same signature per wallet).
        assert_eq!(txs.len(), 2);
        for tx in &txs {
            assert_eq!(tx.direction.as_deref(), Some("BUY"));
        }

        // Signatures are now seen; last-poll is recorded → immediate re-poll
        // within the interval is skipped.
        assert!(state.has_seen(&sig(1)).await);
        let txs2 = poll_wallets_batch(
            &client,
            &[WALLET.to_string()],
            60,
            1,
            rate_limiter(),
            state.clone(),
            None,
        )
        .await
        .unwrap();
        assert!(
            txs2.is_empty(),
            "should_poll filters wallets within interval"
        );
    }
}
