//! Smart RPC polling fallback for transaction monitoring
//!
//! Used when webhooks fail or for validation. Implements signature caching
//! and prioritized polling to minimize credit usage.

use crate::db::DbPool;
use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::rate_limiter::RequestPriority;
use crate::monitoring::transaction_parser;
use anyhow::{Context, Result};
use lru::LruCache;
use rust_decimal::Decimal;
use serde_json::Value;
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

/// Poll wallet transactions using RPC
///
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
    db: Option<&DbPool>,
) -> Result<Vec<WalletTransaction>> {
    // Rate limit before polling
    rate_limiter
        .acquire_standard(RequestPriority::Polling)
        .await;

    // Paginated fetch: signatures are returned newest-first. Walk pages of 10 until
    // the anchor (last_signature) is found or we exhaust 5 pages (50 signatures max).
    const PAGE_SIZE: usize = 10;
    const MAX_PAGES: usize = 5;

    let pubkey = wallet_address.parse().context("Invalid wallet address")?;

    let mut new_signatures: Vec<String> = Vec::new();
    let mut anchor_found = false;
    let mut before_sig: Option<String> = None;

    'pages: for _page in 0..MAX_PAGES {
        rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        let config = solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config {
            before: before_sig
                .as_deref()
                .and_then(|s| s.parse::<solana_sdk::signature::Signature>().ok()),
            limit: Some(PAGE_SIZE),
            ..Default::default()
        };

        let page = rpc_client
            .get_signatures_for_address_with_config(&pubkey, config)
            .await
            .context("Failed to get signatures")?;

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

    for sig_str in new_signatures.iter().take(5) {
        // Limit to 5 transactions per poll
        rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        // Parse signature string to Signature type
        if let Ok(sig) = sig_str.parse::<solana_sdk::signature::Signature>() {
            if let Ok(tx) = rpc_client
                .get_transaction(&sig, solana_transaction_status::UiTransactionEncoding::Json)
                .await
            {
                // Convert UiTransaction to JSON Value for parser
                let tx_json: Value =
                    serde_json::to_value(&tx).context("Failed to serialize transaction to JSON")?;

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
                                transaction_parser::SwapDirection::Buy => Some("BUY".to_string()),
                                transaction_parser::SwapDirection::Sell => Some("SELL".to_string()),
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
        }
    }

    // Update last signature in database if we have new transactions and database access
    if let (Some(latest_sig), Some(db_pool)) = (latest_signature, db) {
        if let Err(e) =
            crate::db::update_wallet_monitoring_signature(db_pool, wallet_address, &latest_sig)
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
    db: Option<&DbPool>,
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
                match crate::db::get_wallet_monitoring(db_pool, wallet).await {
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
