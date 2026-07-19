//! Background RPC polling task for wallet monitoring
//!
//! Automatically polls ACTIVE wallets for new transactions and generates copy trading signals.
//! This provides an alternative to webhooks for local development and production fallback.

use anyhow::{Context, Result};
use rust_decimal::prelude::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::{rpc_polling, ExitDetector, RateLimiter, RpcPollingState};
use crate::circuit_breaker::CircuitBreaker;
use crate::db_abstraction::Database;
use crate::engine::EngineHandle;
use crate::models::{Action, Signal, SignalPayload, Strategy};
use crate::token::TokenParser;
use tokio::sync::RwLock;

/// Configuration for the polling task
#[derive(Debug, Clone)]
pub struct PollingConfig {
    /// Legacy single interval (for backward compatibility)
    pub interval_secs: u64,
    /// Enable tiered polling based on conviction level
    pub tiered_polling_enabled: bool,
    /// Tiered polling intervals
    pub high_conviction_interval_secs: Option<u64>,
    pub regular_conviction_interval_secs: Option<u64>,
    pub emerging_conviction_interval_secs: Option<u64>,
    /// WQS thresholds
    pub high_conviction_wqs_threshold: Option<i32>,
    pub regular_conviction_wqs_threshold: Option<i32>,
    /// Number of wallets to poll in each batch
    pub batch_size: usize,
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Rate limit for RPC calls (requests per second)
    pub rate_limit: u32,
    /// Delay (seconds) before treating a SELL as a position exit
    pub exit_detection_delay_secs: u64,
}

/// Poll wallets for a specific conviction tier
async fn poll_wallets_by_tier(
    db: Arc<dyn Database>,
    engine: EngineHandle,
    tier: crate::config::ConvictionTier,
    config: &PollingConfig,
    rpc_client: Arc<RpcClient>,
    rate_limiter: Arc<RateLimiter>,
    polling_state: Arc<RpcPollingState>,
    circuit_breaker: Arc<CircuitBreaker>,
    token_parser: Arc<TokenParser>,
    exit_detector: Arc<ExitDetector>,
    pending_exits: Arc<RwLock<Vec<super::ExitSignal>>>,
) {
    let interval = match tier {
        crate::config::ConvictionTier::High => config.high_conviction_interval_secs.unwrap_or(config.interval_secs),
        crate::config::ConvictionTier::Regular => config.regular_conviction_interval_secs.unwrap_or(config.interval_secs),
        crate::config::ConvictionTier::Emerging => config.emerging_conviction_interval_secs.unwrap_or(config.interval_secs),
    };

    // Query wallets for this tier
    let wallets = match db.get_wallets_by_conviction_tier(tier).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, tier = ?tier, "Failed to query wallets for tier");
            return;
        }
    };

    // Filter out wallets where monitoring_enabled is false
    let monitored_wallets: Vec<String> = {
        let wallet_addresses: Vec<String> = wallets.iter().map(|w| w.address.clone()).collect();
        let all_monitoring = match db.get_all_wallet_monitoring().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query wallet_monitoring, using all wallets");
                // Fallback to all wallets if monitoring query fails - return empty monitoring list
                vec![]
            }
        };

        let monitoring_enabled_set: std::collections::HashSet<String> = all_monitoring
            .into_iter()
            .filter(|wm| wm.monitoring_enabled > 0)
            .map(|wm| wm.wallet_address)
            .collect();

        // If monitoring query failed, return all wallets as fallback
        if monitoring_enabled_set.is_empty() {
            wallet_addresses
        } else {
            wallet_addresses
                .into_iter()
                .filter(|addr| monitoring_enabled_set.contains(addr))
                .collect()
        }
    };

    if monitored_wallets.is_empty() {
        tracing::trace!(tier = ?tier, "No monitored wallets to poll for this tier");
        return;
    }

    tracing::debug!(
        tier = ?tier,
        wallet_count = monitored_wallets.len(),
        interval_secs = interval,
        "Polling wallets for tier"
    );

    // Poll wallets for new transactions
    let transactions = match rpc_polling::poll_wallets_batch(
        &rpc_client,
        &monitored_wallets,
        interval,
        config.batch_size,
        rate_limiter.clone(),
        polling_state.clone(),
        Some(db.as_ref()),
    )
    .await
    {
        Ok(txs) => txs,
        Err(e) => {
            tracing::warn!(error = %e, "RPC polling batch failed");
            return;
        }
    };

    if transactions.is_empty() {
        tracing::trace!("No new transactions detected for tier {:?}", tier);
        return;
    }

    tracing::info!(
        transaction_count = transactions.len(),
        tier = ?tier,
        "Detected new transactions from tiered polling, processing..."
    );

    // Process each transaction (30-second timeout guards against hung RPC calls)
    for tx in transactions {
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            process_transaction(
                db.as_ref(),
                &engine,
                tx,
                &circuit_breaker,
                &token_parser,
                &exit_detector,
                &pending_exits,
                config.exit_detection_delay_secs,
            ),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "Failed to process transaction"),
            Err(_) => tracing::warn!("process_transaction timed out after 30s"),
        }
    }
}

/// Start the RPC polling background task
///
/// This task runs continuously, polling ACTIVE wallets for new transactions
/// and generating signals for the trading engine.
pub async fn start_polling_task(
    db: Arc<dyn Database>,
    engine: EngineHandle,
    config: PollingConfig,
    cancel_token: CancellationToken,
    circuit_breaker: Arc<CircuitBreaker>,
    token_parser: Arc<TokenParser>,
    exit_detector: Arc<ExitDetector>,
) {
    tracing::info!(
        tiered = config.tiered_polling_enabled,
        high_interval = config.high_conviction_interval_secs.unwrap_or(config.interval_secs),
        regular_interval = config.regular_conviction_interval_secs.unwrap_or(config.interval_secs),
        emerging_interval = config.emerging_conviction_interval_secs.unwrap_or(config.interval_secs),
        "Starting RPC polling task with tiered intervals"
    );

    let polling_state = Arc::new(RpcPollingState::new());
    let rate_limiter = Arc::new(RateLimiter::new(config.rate_limit, 1));

    // Create RPC client with a 5-second timeout. Without a timeout, a hung Helius
    // connection blocks the entire polling loop and prevents failover to QuickNode.
    let rpc_client = Arc::new(RpcClient::new_with_timeout(
        config.rpc_url.clone(),
        Duration::from_secs(5),
    ));

    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Shared state for pending exit signals
    let pending_exits: Arc<RwLock<Vec<super::ExitSignal>>> = Arc::new(RwLock::new(Vec::new()));
    let pending_exits_clone = pending_exits.clone();
    let exit_detector_clone = exit_detector.clone();
    let cancel_token_clone = cancel_token.clone();

    // Background task to process pending exit signals
    tokio::spawn(async move {
        let mut exit_interval = tokio::time::interval(Duration::from_secs(5));
        exit_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel_token_clone.cancelled() => {
                    tracing::info!("Exit signal processor shutting down");
                    break;
                }
                _ = exit_interval.tick() => {
                    let mut pending = pending_exits_clone.write().await;
                    let mut to_remove = Vec::new();

                    for (idx, exit_signal) in pending.iter().enumerate() {
                        if exit_detector_clone.should_generate_exit(exit_signal).await {
                            tracing::info!(
                                wallet = %exit_signal.wallet_address,
                                token = %exit_signal.token_address,
                                exit_type = ?exit_signal.exit_type,
                                "Generating delayed exit signal"
                            );

                            // Mark as processed
                            exit_detector_clone.mark_exit_processed(exit_signal).await;
                            to_remove.push(idx);
                        }
                    }

                    // Remove processed signals (in reverse order to maintain indices)
                    for idx in to_remove.into_iter().rev() {
                        pending.remove(idx);
                    }
                }
            }
        }
    });

    if config.tiered_polling_enabled {
        // Tiered polling: separate intervals for each tier
        let mut high_interval = tokio::time::interval(Duration::from_secs(
            config.high_conviction_interval_secs.unwrap_or(config.interval_secs)
        ));
        let mut regular_interval = tokio::time::interval(Duration::from_secs(
            config.regular_conviction_interval_secs.unwrap_or(config.interval_secs)
        ));
        let mut emerging_interval = tokio::time::interval(Duration::from_secs(
            config.emerging_conviction_interval_secs.unwrap_or(config.interval_secs)
        ));

        high_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        regular_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        emerging_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("RPC polling task shutting down");
                    break;
                }
                _ = high_interval.tick() => {
                    poll_wallets_by_tier(
                        db.clone(),
                        engine.clone(),
                        crate::config::ConvictionTier::High,
                        &config,
                        rpc_client.clone(),
                        rate_limiter.clone(),
                        polling_state.clone(),
                        circuit_breaker.clone(),
                        token_parser.clone(),
                        exit_detector.clone(),
                        pending_exits.clone(),
                    ).await;
                }
                _ = regular_interval.tick() => {
                    poll_wallets_by_tier(
                        db.clone(),
                        engine.clone(),
                        crate::config::ConvictionTier::Regular,
                        &config,
                        rpc_client.clone(),
                        rate_limiter.clone(),
                        polling_state.clone(),
                        circuit_breaker.clone(),
                        token_parser.clone(),
                        exit_detector.clone(),
                        pending_exits.clone(),
                    ).await;
                }
                _ = emerging_interval.tick() => {
                    poll_wallets_by_tier(
                        db.clone(),
                        engine.clone(),
                        crate::config::ConvictionTier::Emerging,
                        &config,
                        rpc_client.clone(),
                        rate_limiter.clone(),
                        polling_state.clone(),
                        circuit_breaker.clone(),
                        token_parser.clone(),
                        exit_detector.clone(),
                        pending_exits.clone(),
                    ).await;
                }
            }
        }
    } else {
        // Legacy single-interval polling (unchanged)
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
                    let wallets = match get_active_monitored_wallets(db.as_ref()).await {
                        Ok(w) => w,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to query active wallets, skipping poll cycle");
                            continue;
                        }
                    };

                    if wallets.is_empty() {
                        if poll_count.is_multiple_of(10) { // Log every 10 cycles to avoid spam
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
                        Some(db.as_ref()),
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

                    // Process each transaction (30-second timeout guards against hung RPC calls)
                    for tx in transactions {
                        let result = tokio::time::timeout(
                            Duration::from_secs(30),
                            process_transaction(
                                db.as_ref(),
                                &engine,
                                tx,
                                &circuit_breaker,
                                &token_parser,
                                &exit_detector,
                                &pending_exits,
                                config.exit_detection_delay_secs,
                            ),
                        )
                        .await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => tracing::warn!(error = %e, "Failed to process transaction"),
                            Err(_) => tracing::warn!("process_transaction timed out after 30s"),
                        }
                    }
                }
            }
        }
    }
}

/// Get list of ACTIVE wallets that should be monitored
async fn get_active_monitored_wallets(db: &dyn Database) -> Result<Vec<String>> {
    let wallets = db
        .get_wallets_by_status("ACTIVE")
        .await
        .context("Failed to query active monitored wallets")?;

    Ok(wallets.into_iter().map(|w| w.address).collect())
}

/// Process a single transaction and generate trading signal
#[allow(clippy::too_many_arguments)]
async fn process_transaction(
    db: &dyn Database,
    engine: &EngineHandle,
    tx: rpc_polling::WalletTransaction,
    circuit_breaker: &CircuitBreaker,
    token_parser: &TokenParser,
    exit_detector: &ExitDetector,
    pending_exits: &Arc<RwLock<Vec<super::ExitSignal>>>,
    exit_detection_delay_secs: u64,
) -> Result<()> {
    // Gate 1: circuit breaker — same check as webhook handler
    if !circuit_breaker.is_trading_allowed() {
        let reason = circuit_breaker
            .trip_reason()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "Circuit breaker tripped".to_string());
        tracing::warn!(
            wallet = %tx.wallet_address,
            reason = %reason,
            "Polling signal rejected by circuit breaker"
        );
        return Ok(());
    }

    // Gate 2: wallet must be ACTIVE
    let wallet = match db.get_wallet(&tx.wallet_address).await? {
        Some(w) => w,
        None => {
            tracing::warn!(wallet = %tx.wallet_address, "Wallet not found in database");
            return Ok(());
        }
    };

    if wallet.status != "ACTIVE" {
        tracing::debug!(
            wallet = %tx.wallet_address,
            status = %wallet.status,
            "Skipping non-ACTIVE wallet"
        );
        return Ok(());
    }

    // Parse transaction to extract swap details
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

    // Require explicit amount — don't guess or default
    let amount_sol = match tx.amount_sol {
        Some(amt) => amt,
        None => {
            tracing::warn!(
                signature = %tx.signature,
                wallet = %tx.wallet_address,
                "Cannot determine transaction amount, skipping signal"
            );
            return Ok(());
        }
    };

    // For SELL transactions, check if this is an exit from a tracked position
    if matches!(direction, Action::Sell) {
        // Convert WalletTransaction to ParsedSwap for exit detection
        let swap_direction = super::transaction_parser::SwapDirection::Sell;
        let parsed_swap = super::transaction_parser::ParsedSwap {
            direction: swap_direction,
            token_in: token_address.clone(),
            token_out: "So11111111111111111111111111111111111111112".to_string(), // SOL
            amount_in: amount_sol,
            amount_out: amount_sol, // Simplified - would need actual conversion
            dex: "unknown".to_string(), // Not available from polling data
            slippage: None, // Not available from polling data
        };

        // Detect exit with configurable delay
        let delay_secs = exit_detection_delay_secs;
        if let Some(exit_signal) = exit_detector
            .detect_exit(&tx.wallet_address, &parsed_swap, delay_secs)
            .await
        {
            tracing::info!(
                wallet = %exit_signal.wallet_address,
                token = %exit_signal.token_address,
                exit_type = ?exit_signal.exit_type,
                delay_secs = exit_signal.delay_secs,
                "Detected exit signal, queueing for delayed generation"
            );

            // Store pending exit for background processing
            let mut exits = pending_exits.write().await;
            exits.push(exit_signal);
        }
    }

    // Polling-generated signals always use Shield: we cannot verify strategy intent
    // from on-chain data alone, so use the conservative path which enforces strict
    // stop-losses and correct per-strategy sizing.
    let strategy = Strategy::Shield;

    // Create signal payload
    let payload = SignalPayload {
        strategy,
        token: token_address.clone(), // Using token address as token symbol for now
        token_address: Some(token_address.clone()),
        action: direction,
        amount_sol,
        wallet_address: tx.wallet_address.clone(),
        trade_uuid: None, // Will be auto-generated
        exit_fraction: None,
    };

    // Gate 3: duplicate UUID check — prevents re-processing on restart/pagination gaps
    let trade_uuid = payload.generate_trade_uuid(tx.timestamp);
    if db.trade_uuid_exists(&trade_uuid).await.unwrap_or(false) {
        tracing::debug!(
            trade_uuid = %trade_uuid,
            "Duplicate polling signal skipped"
        );
        return Ok(());
    }

    // Gate 4: token safety fast-path (BUY signals only; SELL signals already own the token)
    if matches!(direction, Action::Buy) {
        match token_parser.fast_check(&token_address, strategy).await {
            Ok(result) if !result.safe => {
                let reason = result
                    .rejection_reason
                    .unwrap_or_else(|| "Token failed safety check".to_string());
                tracing::warn!(
                    token = %token_address,
                    wallet = %tx.wallet_address,
                    reason = %reason,
                    "Polling signal rejected by token safety check"
                );
                return Ok(());
            }
            Err(e) => {
                // Fail closed: if we can't verify safety, reject the signal
                tracing::warn!(
                    token = %token_address,
                    error = %e,
                    "Token safety check failed, rejecting polling signal"
                );
                return Ok(());
            }
            Ok(_) => {} // safe — proceed
        }
    }

    // Create signal (liquidity_usd not available from RPC polling path — executor uses config fallback)
    // force_slow_path is false: RPC polling signals have not gone through fast_check at all,
    // so slow-path runs unconditionally in the engine as normal.
    let token_decimals = token_parser.get_token_decimals(&token_address).await;
    let signal = Signal {
        trade_uuid,
        payload: payload.clone(),
        timestamp: tx.timestamp,
        source_ip: Some("rpc_polling".to_string()),
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals,
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
    engine
        .queue_signal(signal, wallet.wqs_score.map(|v| v.to_f64().unwrap_or(0.0)))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to queue signal: {}", e))?;

    Ok(())
}
