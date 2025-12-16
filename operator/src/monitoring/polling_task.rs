//! Background RPC polling task for wallet monitoring
//!
//! Automatically polls ACTIVE wallets for new transactions and generates copy trading signals.
//! This provides an alternative to webhooks for local development and production fallback.

use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use solana_client::rpc_client::RpcClient;

use crate::db::DbPool;
use crate::engine::EngineHandle;
use crate::models::{Signal, SignalPayload, Strategy, Action};
use super::{RpcPollingState, RateLimiter, rpc_polling};
use rust_decimal::Decimal;

/// Configuration for the polling task
#[derive(Debug, Clone)]
pub struct PollingConfig {
    /// Interval between polling cycles (seconds)
    pub interval_secs: u64,
    /// Number of wallets to poll in each batch
    pub batch_size: usize,
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Rate limit for RPC calls (requests per second)
    pub rate_limit: u32,
}

/// Start the RPC polling background task
///
/// This task runs continuously, polling ACTIVE wallets for new transactions
/// and generating signals for the trading engine.
pub async fn start_polling_task(
    db: DbPool,
    engine: EngineHandle,
    config: PollingConfig,
    cancel_token: CancellationToken,
) {
    tracing::info!(
        interval_secs = config.interval_secs,
        batch_size = config.batch_size,
        rpc_url = %config.rpc_url,
        "Starting RPC polling task"
    );

    let polling_state = Arc::new(RpcPollingState::new());
    let rate_limiter = Arc::new(RateLimiter::new(config.rate_limit, 1));
    
    // Create RPC client (RpcClient::new doesn't return Result, just creates client)
    let rpc_client = Arc::new(RpcClient::new(config.rpc_url.clone()));

    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut poll_count = 0u64;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("RPC polling task shutting down");
                break;
            }
            _ = interval.tick() => {
                poll_count += 1;
                
                // Query ACTIVE wallets from database
                let wallets = match get_active_monitored_wallets(&db).await {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to query active wallets, skipping poll cycle");
                        continue;
                    }
                };

                if wallets.is_empty() {
                    if poll_count % 10 == 0 { // Log every 10 cycles to avoid spam
                        tracing::debug!("No active wallets to monitor");
                    }
                    continue;
                }

                tracing::debug!(
                    wallet_count = wallets.len(),
                    poll_cycle = poll_count,
                    "Polling active wallets"
                );

                // Poll wallets for new transactions
                let transactions = match rpc_polling::poll_wallets_batch(
                    &rpc_client,
                    &wallets,
                    config.interval_secs,
                    config.batch_size,
                    rate_limiter.clone(),
                    polling_state.clone(),
                    Some(&db),
                )
                .await
                {
                    Ok(txs) => txs,
                    Err(e) => {
                        tracing::warn!(error = %e, "RPC polling batch failed");
                        continue;
                    }
                };

                if transactions.is_empty() {
                    tracing::trace!("No new transactions detected");
                    continue;
                }

                tracing::info!(
                    transaction_count = transactions.len(),
                    "Detected new transactions, processing..."
                );

                // Process each transaction
                for tx in transactions {
                    if let Err(e) = process_transaction(&db, &engine, tx).await {
                        tracing::warn!(error = %e, "Failed to process transaction");
                    }
                }
            }
        }
    }
}

/// Get list of ACTIVE wallets that should be monitored
async fn get_active_monitored_wallets(db: &DbPool) -> Result<Vec<String>> {
    let wallets = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT w.address
        FROM wallets w
        LEFT JOIN wallet_monitoring wm ON w.address = wm.wallet_address
        WHERE w.status = 'ACTIVE'
        AND (wm.monitoring_enabled IS NULL OR wm.monitoring_enabled = 1)
        ORDER BY w.last_trade_at DESC
        "#
    )
    .fetch_all(db)
    .await
    .context("Failed to query active monitored wallets")?;

    Ok(wallets)
}

/// Process a single transaction and generate trading signal
async fn process_transaction(
    db: &DbPool,
    engine: &EngineHandle,
    tx: rpc_polling::WalletTransaction,
) -> Result<()> {
    // Get wallet info from database
    let wallet = match crate::db::get_wallet_by_address(db, &tx.wallet_address).await? {
        Some(w) => w,
        None => {
            tracing::warn!(wallet = %tx.wallet_address, "Wallet not found in database");
            return Ok(());
        }
    };

    // Only process ACTIVE wallets
    if wallet.status != "ACTIVE" {
        tracing::debug!(
            wallet = %tx.wallet_address,
            status = %wallet.status,
            "Skipping non-ACTIVE wallet"
        );
        return Ok(());
    }

    // Parse transaction to extract swap details
    // Note: The WalletTransaction from RPC polling is simplified
    // We need to fetch full transaction details and parse them
    
    // For now, we'll use the basic info from polling
    // TODO: Enhance this to fetch and parse full transaction details if needed
    
    let (direction, token_address) = match (tx.direction.as_deref(), tx.token_address.as_ref()) {
        (Some("BUY"), Some(token)) => (Action::Buy, token.clone()),
        (Some("SELL"), Some(token)) => (Action::Sell, token.clone()),
        _ => {
            tracing::trace!(
                signature = %tx.signature,
                "Transaction not a clear BUY/SELL, skipping"
            );
            return Ok(());
        }
    };

    let amount_sol = tx.amount_sol.unwrap_or_else(|| Decimal::from(1)); // Default to 1 SOL if unknown

    // Determine strategy based on wallet quality score
    let strategy = if wallet.wqs_score.unwrap_or(0.0) >= 70.0 {
        Strategy::Spear
    } else {
        Strategy::Shield
    };

    // Create signal payload
    let payload = SignalPayload {
        strategy,
        token: token_address.clone(), // Using token address as token symbol for now
        token_address: Some(token_address.clone()),
        action: direction,
        amount_sol,
        wallet_address: tx.wallet_address.clone(),
        trade_uuid: None, // Will be auto-generated
    };

    // Create signal
    let signal = Signal {
        trade_uuid: payload.generate_trade_uuid(tx.timestamp),
        payload: payload.clone(),
        timestamp: tx.timestamp,
        source_ip: Some("rpc_polling".to_string()), // Mark source as RPC polling
    };

    tracing::info!(
        wallet = %tx.wallet_address,
        token = %token_address,
        direction = ?direction,
        amount_sol = %amount_sol,
        strategy = ?strategy,
        signature = %tx.signature,
        "Generated signal from RPC polling"
    );

    // Queue signal to engine
    engine.queue_signal(signal, wallet.wqs_score).await
        .map_err(|e| anyhow::anyhow!("Failed to queue signal: {}", e))?;

    Ok(())
}


