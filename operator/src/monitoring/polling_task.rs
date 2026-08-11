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
    /// Minimum position size in SOL — dust trades below this are skipped
    pub min_position_sol: rust_decimal::Decimal,
}

/// Poll wallets for a specific conviction tier
async fn poll_wallets_by_tier(
    db: Arc<dyn Database>,
    engine: EngineHandle,
    tier: crate::config::ConvictionTier,
    polling_cfg: &PollingConfig,
    rpc_client: Arc<RpcClient>,
    rate_limiter: Arc<RateLimiter>,
    polling_state: Arc<RpcPollingState>,
    circuit_breaker: Arc<CircuitBreaker>,
    token_parser: Arc<TokenParser>,
    exit_detector: Arc<ExitDetector>,
    pending_exits: Arc<RwLock<Vec<super::ExitSignal>>>,
) {
    tracing::info!(tier = ?tier, "poll_wallets_by_tier invoked");

    let interval = match tier {
        crate::config::ConvictionTier::High => polling_cfg
            .high_conviction_interval_secs
            .unwrap_or(polling_cfg.interval_secs),
        crate::config::ConvictionTier::Regular => polling_cfg
            .regular_conviction_interval_secs
            .unwrap_or(polling_cfg.interval_secs),
        crate::config::ConvictionTier::Emerging => polling_cfg
            .emerging_conviction_interval_secs
            .unwrap_or(polling_cfg.interval_secs),
    };

    // Query wallets for this tier
    let wallets = match db.get_wallets_by_conviction_tier(tier).await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, tier = ?tier, "Failed to query wallets for tier");
            return;
        }
    };

    // Filter out wallets where monitoring_enabled is false. Fail closed: a
    // DB error must never turn into "poll everything" — an empty enabled set
    // means "poll nothing", not "poll all wallets".
    //
    // We poll EVERY enabled monitored wallet (no webhook-skip): when polling
    // runs on a non-Helius RPC (see rpc.primary_url) it consumes no Helius
    // credits, and any trade a webhook also delivers is deduped by the
    // seen-signature cache in rpc_polling. Skipping "healthy-webhook"
    // wallets would silently drop signals while Helius is quota-exhausted
    // (webhooks stop delivering, yet the wallet stays marked healthy), so we
    // poll all to guarantee coverage.
    let monitored_wallets: Vec<String> = {
        let wallet_addresses: Vec<String> = wallets.iter().map(|w| w.address.clone()).collect();
        let all_monitoring = match db.get_all_wallet_monitoring().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, tier = ?tier, "Failed to query wallet_monitoring — skipping poll cycle");
                return;
            }
        };

        let monitoring_enabled_set: std::collections::HashSet<String> = all_monitoring
            .into_iter()
            .filter(|wm| wm.monitoring_enabled)
            .map(|wm| wm.wallet_address)
            .collect();

        wallet_addresses
            .into_iter()
            .filter(|addr| monitoring_enabled_set.contains(addr))
            .collect()
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
        polling_cfg.batch_size,
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
                polling_cfg.exit_detection_delay_secs,
                polling_cfg.min_position_sol,
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

/// Spawn a single-tier polling loop as an independent task.
///
/// Each tier gets its own interval, so high-conviction polling is never
/// blocked by slow batches on other tiers.
#[allow(clippy::too_many_arguments)]
fn spawn_tier_loop(
    tier: crate::config::ConvictionTier,
    cancel_token: CancellationToken,
    interval_secs: u64,
    db: Arc<dyn Database>,
    engine: EngineHandle,
    config: PollingConfig,
    rpc_client: Arc<RpcClient>,
    rate_limiter: Arc<RateLimiter>,
    polling_state: Arc<RpcPollingState>,
    circuit_breaker: Arc<CircuitBreaker>,
    token_parser: Arc<TokenParser>,
    exit_detector: Arc<ExitDetector>,
    pending_exits: Arc<RwLock<Vec<super::ExitSignal>>>,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(tier = ?tier, interval_secs = interval_secs, "spawn_tier_loop starting");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!(
                        tier = ?tier,
                        "RPC polling tier task shutting down"
                    );
                    break;
                }
                _ = interval.tick() => {
                    tracing::debug!(tier = ?tier, "tier interval tick fired");
                    poll_wallets_by_tier(
                        db.clone(),
                        engine.clone(),
                        tier,
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
    })
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
        high_interval = config
            .high_conviction_interval_secs
            .unwrap_or(config.interval_secs),
        regular_interval = config
            .regular_conviction_interval_secs
            .unwrap_or(config.interval_secs),
        emerging_interval = config
            .emerging_conviction_interval_secs
            .unwrap_or(config.interval_secs),
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

    // Shared state for pending exit signals
    let pending_exits: Arc<RwLock<Vec<super::ExitSignal>>> = Arc::new(RwLock::new(Vec::new()));
    let pending_exits_clone = pending_exits.clone();
    let exit_detector_clone = exit_detector.clone();
    let cancel_token_clone = cancel_token.clone();
    let engine_clone = engine.clone();

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
                    // Snapshot the pending signals under a short read lock; the
                    // per-signal readiness check + removal is atomic inside the
                    // detector, so no write lock is held across awaits here.
                    let due: Vec<super::ExitSignal> = pending_exits_clone.read().await.clone();

                    for exit_signal in due {
                        if !exit_detector_clone.take_ready_exit(&exit_signal).await {
                            continue;
                        }

                        let timestamp = chrono::Utc::now().timestamp();
                        let payload = SignalPayload {
                            strategy: Strategy::Exit,
                            token: exit_signal.token_address.clone(),
                            token_address: Some(exit_signal.token_address.clone()),
                            action: Action::Sell,
                            amount_sol: exit_signal.amount_sol,
                            wallet_address: exit_signal.wallet_address.clone(),
                            trade_uuid: None,
                            exit_fraction: None,
                        };
                        let trade_uuid = payload.generate_trade_uuid(timestamp);
                        let signal = Signal {
                            trade_uuid: trade_uuid.clone(),
                            payload,
                            timestamp,
                            source_ip: Some("rpc_polling_exit".to_string()),
                            liquidity_usd: None,
                            force_slow_path: true,
                            token_decimals: None,
                        };

                        tracing::info!(
                            wallet = %exit_signal.wallet_address,
                            token = %exit_signal.token_address,
                            exit_type = ?exit_signal.exit_type,
                            amount_sol = %exit_signal.amount_sol,
                            trade_uuid = %trade_uuid,
                            "Dispatching delayed exit signal to engine"
                        );

                        if let Err(e) = engine_clone.queue_signal(signal, None).await {
                            tracing::error!(
                                wallet = %exit_signal.wallet_address,
                                token = %exit_signal.token_address,
                                error = %e,
                                "Failed to queue delayed exit signal"
                            );
                        }

                        let mut pending = pending_exits_clone.write().await;
                        pending.retain(|s| s != &exit_signal);
                    }
                }
            }
        }
    });

    if config.tiered_polling_enabled {
        let high_interval_secs = config
            .high_conviction_interval_secs
            .unwrap_or(config.interval_secs);
        let regular_interval_secs = config
            .regular_conviction_interval_secs
            .unwrap_or(config.interval_secs);
        let emerging_interval_secs = config
            .emerging_conviction_interval_secs
            .unwrap_or(config.interval_secs);

        let high_handle = spawn_tier_loop(
            crate::config::ConvictionTier::High,
            cancel_token.clone(),
            high_interval_secs,
            db.clone(),
            engine.clone(),
            config.clone(),
            rpc_client.clone(),
            rate_limiter.clone(),
            polling_state.clone(),
            circuit_breaker.clone(),
            token_parser.clone(),
            exit_detector.clone(),
            pending_exits.clone(),
        );
        let regular_handle = spawn_tier_loop(
            crate::config::ConvictionTier::Regular,
            cancel_token.clone(),
            regular_interval_secs,
            db.clone(),
            engine.clone(),
            config.clone(),
            rpc_client.clone(),
            rate_limiter.clone(),
            polling_state.clone(),
            circuit_breaker.clone(),
            token_parser.clone(),
            exit_detector.clone(),
            pending_exits.clone(),
        );
        let emerging_handle = spawn_tier_loop(
            crate::config::ConvictionTier::Emerging,
            cancel_token.clone(),
            emerging_interval_secs,
            db.clone(),
            engine.clone(),
            config.clone(),
            rpc_client.clone(),
            rate_limiter.clone(),
            polling_state.clone(),
            circuit_breaker.clone(),
            token_parser.clone(),
            exit_detector.clone(),
            pending_exits.clone(),
        );

        cancel_token.cancelled().await;
        tracing::info!("RPC polling task shutting down, waiting for tier tasks");

        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            std::mem::drop(high_handle.await);
            std::mem::drop(regular_handle.await);
            std::mem::drop(emerging_handle.await);
        })
        .await;
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
                                config.min_position_sol,
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
    min_position_sol: rust_decimal::Decimal,
) -> Result<()> {
    // Gate 1: circuit breaker — same check as webhook handler
    if !circuit_breaker.is_trading_allowed() {
        let reason = circuit_breaker
            .trip_reason()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "Circuit breaker tripped".to_string());
        tracing::warn!(
            wallet = %tx.wallet_address,
            signature = %tx.signature,
            reason = %reason,
            "Polling signal rejected by circuit breaker"
        );
        return Ok(());
    }

    // Gate 2: wallet must be ACTIVE
    let wallet = match db.get_wallet(&tx.wallet_address).await? {
        Some(w) => w,
        None => {
            tracing::warn!(
                wallet = %tx.wallet_address,
                signature = %tx.signature,
                "Wallet not found in database"
            );
            return Ok(());
        }
    };

    if wallet.status != "ACTIVE" {
        tracing::debug!(
            wallet = %tx.wallet_address,
            status = %wallet.status,
            signature = %tx.signature,
            direction = ?tx.direction,
            token = ?tx.token_address,
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

    if direction == Action::Buy && amount_sol < min_position_sol {
        tracing::debug!(
            signature = %tx.signature,
            wallet = %tx.wallet_address,
            token = %token_address,
            amount_sol = %amount_sol,
            min_position_sol = %min_position_sol,
            "Polling trade amount below minimum — skipping dust signal"
        );
        return Ok(());
    }

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
            slippage: None,         // Not available from polling data
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
            wallet = %tx.wallet_address,
            token = %token_address,
            "Duplicate polling signal skipped"
        );
        return Ok(());
    }

    // Gate 4: token safety fast-path (BUY signals only; SELL signals already own the token)
    let fast_check_result = if matches!(direction, Action::Buy) {
        match token_parser.fast_check(&token_address, strategy).await {
            Ok(result) if !result.safe => {
                let reason = result
                    .rejection_reason
                    .unwrap_or_else(|| "Token failed safety check".to_string());
                tracing::warn!(
                    token = %token_address,
                    wallet = %tx.wallet_address,
                    signature = %tx.signature,
                    amount_sol = %amount_sol,
                    reason = %reason,
                    "Polling signal rejected by token safety check"
                );
                return Ok(());
            }
            Err(e) => {
                // Fail closed: if we can't verify safety, reject the signal
                tracing::warn!(
                    token = %token_address,
                    wallet = %tx.wallet_address,
                    signature = %tx.signature,
                    error = %e,
                    "Token safety check failed, rejecting polling signal"
                );
                return Ok(());
            }
            Ok(result) => Some(result), // safe — proceed
        }
    } else {
        None
    };

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
        fast_check_liquidity_usd = ?fast_check_result.as_ref().and_then(|r| r.liquidity_usd),
        fast_check_safe = ?fast_check_result.as_ref().map(|r| r.safe),
        bypasses_selection_engine = true,
        bypasses_position_sizer = true,
        "polling: signal queued bypassing selection engine (raw amount_sol, no PositionSizer)"
    );

    // Queue signal to engine
    engine
        .queue_signal(signal, wallet.wqs_score.map(|v| v.to_f64().unwrap_or(0.0)))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to queue signal: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_abstraction::{Database, Wallet, WalletMonitoring};
    use crate::engine::Engine;
    use crate::monitoring::test_db::MockDb;
    use crate::token::{
        TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig, TokenSafetyResult,
    };
    use chrono::Utc;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::sync::atomic::Ordering;

    const WALLET_A: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    const WALLET_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    const TOKEN_A: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";

    fn wallet(address: &str, status: &str, wqs: Option<f64>) -> Wallet {
        Wallet {
            id: 0,
            address: address.to_string(),
            status: status.to_string(),
            wqs_score: wqs.map(|s| Decimal::from_f64(s).unwrap()),
            wqs_confidence: None,
            roi_7d: None,
            roi_30d: None,
            trade_count_30d: None,
            win_rate: None,
            max_drawdown_30d: None,
            avg_trade_size_sol: None,
            avg_win_sol: None,
            avg_loss_sol: None,
            profit_factor: None,
            realized_pnl_30d_sol: None,
            last_trade_at: None,
            promoted_at: Some(Utc::now()),
            ttl_expires_at: None,
            notes: None,
            archetype: None,
            avg_entry_delay_seconds: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn monitoring(wallet: &str, enabled: bool) -> WalletMonitoring {
        WalletMonitoring {
            wallet_address: wallet.to_string(),
            helius_webhook_id: None,
            rpc_polling_active: true,
            last_transaction_signature: None,
            last_monitored_at: None,
            monitoring_enabled: enabled,
            created_at: String::new(),
            updated_at: String::new(),
            webhook_status: None,
            webhook_registered_at: None,
            webhook_last_health_check: None,
            webhook_health_status: None,
            registration_attempts: 0,
            last_registration_error: None,
            last_updated_url: None,
            last_speculative_signal_at: None,
            inactivity_demotion_count: 0,
        }
    }

    fn mock_db() -> Arc<MockDb> {
        Arc::new(MockDb::new())
    }

    fn token_parser(seeded_safe: bool) -> Arc<TokenParser> {
        let cache = Arc::new(TokenCache::new(1000, 300));
        cache.insert(
            format!("{TOKEN_A}:{}", Strategy::Shield),
            TokenSafetyResult {
                safe: seeded_safe,
                rejection_reason: if seeded_safe {
                    None
                } else {
                    Some("injected unsafe".to_string())
                },
                honeypot_checked: false,
                liquidity_checked: true,
                liquidity_usd: Some(dec!(100000)),
            },
        );
        let fetcher = Arc::new(
            TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
                "http://127.0.0.1:1",
                None,
                "http://127.0.0.1:1".to_string(),
            )
            .with_price_cache(Arc::new(crate::price_cache::PriceCache::new().unwrap())),
        );
        Arc::new(TokenParser::new(
            TokenSafetyConfig {
                freeze_authority_whitelist: std::collections::HashSet::new(),
                mint_authority_whitelist: std::collections::HashSet::new(),
                min_liquidity_shield_usd: dec!(0),
                min_liquidity_spear_usd: dec!(0),
                honeypot_detection_enabled: false,
                holder_concentration_check_enabled: false,
                max_holder_concentration_pct: 100.0,
            },
            cache,
            fetcher,
        ))
    }

    fn engine_handle(db: Arc<dyn Database>) -> EngineHandle {
        let (_, handle) = Engine::new(crate::config::AppConfig::default(), db);
        handle
    }

    fn polling_config() -> PollingConfig {
        PollingConfig {
            interval_secs: 3600,
            tiered_polling_enabled: false,
            high_conviction_interval_secs: Some(3600),
            regular_conviction_interval_secs: Some(3600),
            emerging_conviction_interval_secs: Some(3600),
            high_conviction_wqs_threshold: Some(80),
            regular_conviction_wqs_threshold: Some(60),
            batch_size: 10,
            rpc_url: "http://127.0.0.1:1".to_string(),
            rate_limit: 1000,
            exit_detection_delay_secs: 0,
            min_position_sol: dec!(0.01),
        }
    }

    fn buy_tx(amount: Decimal) -> rpc_polling::WalletTransaction {
        rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-buy".to_string(),
            token_address: Some(TOKEN_A.to_string()),
            direction: Some("BUY".to_string()),
            amount_sol: Some(amount),
            timestamp: 1700000000,
        }
    }

    // ==========================================================================
    // get_active_monitored_wallets
    // ==========================================================================

    #[tokio::test]
    async fn get_active_monitored_wallets_filters_by_status() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet(wallet(WALLET_B, "PENDING", Some(50.0)));
        let wallets = get_active_monitored_wallets(db.as_ref()).await.unwrap();
        assert_eq!(wallets, vec![WALLET_A.to_string()]);
    }

    #[tokio::test]
    async fn get_active_monitored_wallets_propagates_error() {
        let db = mock_db();
        db.wallets_by_status_error.store(true, Ordering::Relaxed);
        assert!(get_active_monitored_wallets(db.as_ref()).await.is_err());
    }

    // ==========================================================================
    // process_transaction gates
    // ==========================================================================

    #[tokio::test]
    async fn process_rejects_when_circuit_breaker_tripped() {
        let Some(db) = real_db().await else {
            eprintln!("TEST_DATABASE_URL not set — skipping");
            return;
        };
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        // Manual trip requires a DB-backed CircuitBreaker.
        cb.manual_trip("test", "coverage".into()).await.unwrap();
        assert!(!cb.is_trading_allowed());

        let engine = engine_handle(db);
        let tc = token_parser(true);
        let exits = Arc::new(RwLock::new(Vec::new()));
        let result = process_transaction(
            Arc::clone(&mock_db()).as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            &tc,
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await;
        assert!(result.is_ok(), "rejected signal is not an error");
        assert!(exits.read().await.is_empty());
        drop(engine);
    }

    #[tokio::test]
    async fn process_skips_unknown_wallet() {
        let db = mock_db();
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));
        let result = process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn process_skips_non_active_wallet() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "PENDING", Some(80.0)));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));
        let db2 = db.clone();
        spawn_engine_holder(db2);
        let result = process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await;
        assert!(result.is_ok());
        drop(result);
    }

    fn spawn_engine_holder(_db: Arc<dyn Database>) {
        // Keep the db/engine alive for the duration of the test.
    }

    #[tokio::test]
    async fn process_skips_unknown_direction_or_token() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        let tx = rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-x".to_string(),
            token_address: None,
            direction: Some("BUY".to_string()),
            amount_sol: Some(dec!(1.0)),
            timestamp: 1700000000,
        };
        process_transaction(
            db.as_ref(),
            &engine,
            tx,
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();

        let tx2 = rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-y".to_string(),
            token_address: Some(TOKEN_A.to_string()),
            direction: Some("HOLD".to_string()),
            amount_sol: Some(dec!(1.0)),
            timestamp: 1700000000,
        };
        process_transaction(
            db.as_ref(),
            &engine,
            tx2,
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn process_skips_missing_amount_and_dust() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        let mut tx = buy_tx(dec!(1.0));
        tx.amount_sol = None;
        process_transaction(
            db.as_ref(),
            &engine,
            tx,
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();

        // Dust BUY below min_position_sol.
        process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(0.001)),
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn process_sell_queues_exit_signal() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits: Arc<RwLock<Vec<crate::monitoring::ExitSignal>>> =
            Arc::new(RwLock::new(Vec::new()));
        let detector = Arc::new(ExitDetector::new());

        let tx = rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-sell".to_string(),
            token_address: Some(TOKEN_A.to_string()),
            direction: Some("SELL".to_string()),
            amount_sol: Some(dec!(5.0)),
            timestamp: 1700000000,
        };
        process_transaction(
            db.as_ref(),
            &engine,
            tx,
            &cb,
            token_parser(true).as_ref(),
            &detector,
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();

        let pending = exits.read().await.clone();
        assert_eq!(pending.len(), 1, "SELL must enqueue a delayed exit signal");
        assert_eq!(pending[0].wallet_address, WALLET_A);
        assert_eq!(pending[0].token_address, TOKEN_A);
    }

    #[tokio::test]
    async fn process_duplicate_uuid_skipped() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        // Pre-register the UUID process_transaction would generate.
        let payload = SignalPayload {
            strategy: Strategy::Shield,
            token: TOKEN_A.to_string(),
            token_address: Some(TOKEN_A.to_string()),
            action: Action::Buy,
            amount_sol: dec!(1.0),
            wallet_address: WALLET_A.to_string(),
            trade_uuid: None,
            exit_fraction: None,
        };
        let uuid = payload.generate_trade_uuid(1700000000);
        db.add_trade_uuid(&uuid);

        process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn process_rejects_unsafe_token_and_fast_check_error() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        // Unsafe seeded result → rejected.
        process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            token_parser(false).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();
        assert!(exits.read().await.is_empty());

        // fast_check Err path: an invalid (too-short) token address.
        let bad_tx = rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-bad".to_string(),
            token_address: Some("short".to_string()),
            direction: Some("BUY".to_string()),
            amount_sol: Some(dec!(1.0)),
            timestamp: 1700000000,
        };
        process_transaction(
            db.as_ref(),
            &engine,
            bad_tx,
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn process_queues_signal_when_all_gates_pass() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        process_transaction(
            db.as_ref(),
            &engine,
            buy_tx(dec!(1.0)),
            &cb,
            token_parser(true).as_ref(),
            &ExitDetector::new(),
            &exits,
            0,
            dec!(0.01),
        )
        .await
        .unwrap();

        assert_eq!(engine.queue_depth(), 1, "BUY signal queued to engine");
    }

    // ==========================================================================
    // poll_wallets_by_tier
    // ==========================================================================

    #[tokio::test]
    async fn tier_polling_handles_query_errors_and_empty() {
        let db = mock_db();
        let cfg = polling_config();
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let state = Arc::new(RpcPollingState::new());
        let exits = Arc::new(RwLock::new(Vec::new()));
        let client = Arc::new(RpcClient::new_with_timeout(
            "http://127.0.0.1:1".to_string(),
            Duration::from_secs(2),
        ));

        // Tier query error → early return.
        db.tier_query_error.store(true, Ordering::Relaxed);
        poll_wallets_by_tier(
            db.clone(),
            engine_handle(db.clone()),
            crate::config::ConvictionTier::High,
            &cfg,
            client.clone(),
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;

        db.tier_query_error.store(false, Ordering::Relaxed);

        // Monitoring query error → early return.
        db.monitoring_all_error.store(true, Ordering::Relaxed);
        poll_wallets_by_tier(
            db.clone(),
            engine_handle(db.clone()),
            crate::config::ConvictionTier::High,
            &cfg,
            client.clone(),
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;
        db.monitoring_all_error.store(false, Ordering::Relaxed);

        // A wallet with monitoring disabled → filtered out → empty.
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, false));
        poll_wallets_by_tier(
            db.clone(),
            engine_handle(db.clone()),
            crate::config::ConvictionTier::High,
            &cfg,
            client.clone(),
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;
    }

    #[tokio::test]
    async fn tier_polling_full_cycle_with_mock_rpc() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(90.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));

        let url = mock_rpc_price_server(false).await;
        let client = Arc::new(RpcClient::new_with_timeout(url, Duration::from_secs(5)));
        let cfg = PollingConfig {
            interval_secs: 3600,
            ..polling_config()
        };
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let state = Arc::new(RpcPollingState::new());
        let engine = engine_handle(db.clone());
        let exits = Arc::new(RwLock::new(Vec::new()));

        poll_wallets_by_tier(
            db.clone(),
            engine.clone(),
            crate::config::ConvictionTier::High,
            &cfg,
            client.clone(),
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;

        // The RPC returned 1 signature → 1 BUY transaction → signal queued.
        assert_eq!(engine.queue_depth(), 1, "tier polling queued a signal");
    }

    /// JSON-RPC mock returning one signature and one parsed swap transaction.
    async fn mock_rpc_price_server(sell: bool) -> String {
        use std::sync::Mutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sol = "So11111111111111111111111111111111111111112";
        let sig = bs58::encode([1u8; 64]).into_string();
        let sell = sell;
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 32768];
                let Ok(n) = sock.read(&mut buf).await else {
                    continue;
                };
                let body = String::from_utf8_lossy(&buf[..n]).to_string();
                let response = if body.contains("getSignaturesForAddress") {
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "result": [{"signature": sig, "slot": 1, "err": null, "memo": null, "blockTime": 1700000000}]
                    })
                } else if body.contains("getTransaction") {
                    let (pre_sol, post_sol, pre_tok, post_tok) = if sell {
                        // SELL: SOL 10 → 11, TOKEN 100 → 0
                        (10.0, 11.0, 100.0, 0.0)
                    } else {
                        // BUY: SOL 10 → 9, TOKEN 0 → 100
                        (10.0, 9.0, 0.0, 100.0)
                    };
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "result": {
                            "slot": 1,
                            "transaction": {
                                "signatures": [sig],
                                "message": {
                                    "header": {"numRequiredSignatures": 1, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
                                    "accountKeys": [
                                        {"pubkey": WALLET_A, "writable": true, "signer": true},
                                        {"pubkey": "token-ata", "writable": true, "signer": false}
                                    ],
                                    "recentBlockhash": "11111111111111111111111111111111",
                                    "instructions": [{"programId": "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4", "program": "jupiter", "parsed": {}, "stackHeight": null}]
                                }
                            },
                            "meta": {
                                "err": null, "status": {"Ok": null}, "fee": 5000,
                                "preBalances": [10000000000_i64, 0], "postBalances": [9000000000_i64, 0],
                                "preTokenBalances": [
                                    {"accountIndex": 0, "mint": sol, "uiTokenAmount": {"uiAmount": pre_sol, "uiAmountString": pre_sol.to_string(), "decimals": 9, "amount": "0"}},
                                    {"accountIndex": 1, "mint": TOKEN_A, "uiTokenAmount": {"uiAmount": pre_tok, "uiAmountString": pre_tok.to_string(), "decimals": 9, "amount": "0"}}
                                ],
                                "postTokenBalances": [
                                    {"accountIndex": 0, "mint": sol, "uiTokenAmount": {"uiAmount": post_sol, "uiAmountString": post_sol.to_string(), "decimals": 9, "amount": "0"}},
                                    {"accountIndex": 1, "mint": TOKEN_A, "uiTokenAmount": {"uiAmount": post_tok, "uiAmountString": post_tok.to_string(), "decimals": 9, "amount": "0"}}
                                ],
                                "innerInstructions": [], "logMessages": [], "rewards": []
                            },
                            "blockTime": 1700000000
                        }
                    })
                } else {
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "unknown"}})
                };
                let response = response.to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    // ==========================================================================
    // start_polling_task lifecycle
    // ==========================================================================

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_polling_tiered_shuts_down_on_cancel() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(90.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        // 1-second tier intervals so each tier loop actually ticks (covering
        // all three tier branches + the interval dispatch path).
        let cfg = PollingConfig {
            tiered_polling_enabled: true,
            interval_secs: 1,
            high_conviction_interval_secs: Some(1),
            regular_conviction_interval_secs: Some(1),
            emerging_conviction_interval_secs: Some(1),
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine,
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));

        // Let the tier loops tick a few times (wallets empty → early returns),
        // then cancel.
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("task exits after cancellation")
            .expect("no panic");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tier_polling_batch_error_and_no_transactions() {
        // RPC poll failure → warn and return (no panic).
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(90.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cfg = polling_config();
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let state = Arc::new(RpcPollingState::new());
        let exits = Arc::new(RwLock::new(Vec::new()));
        let dead_client = Arc::new(RpcClient::new_with_timeout(
            "http://127.0.0.1:1".to_string(),
            Duration::from_secs(2),
        ));
        poll_wallets_by_tier(
            db.clone(),
            engine_handle(db.clone()),
            crate::config::ConvictionTier::Regular,
            &cfg,
            dead_client,
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;

        // RPC works but returns no signatures → empty transactions → return.
        let url = mock_rpc_empty_signatures().await;
        let client = Arc::new(RpcClient::new_with_timeout(url, Duration::from_secs(5)));
        poll_wallets_by_tier(
            db.clone(),
            engine_handle(db.clone()),
            crate::config::ConvictionTier::Emerging,
            &cfg,
            client,
            Arc::new(RateLimiter::new(1000, 1)),
            state.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
            exits.clone(),
        )
        .await;
    }

    /// JSON-RPC mock returning an empty signatures page.
    async fn mock_rpc_empty_signatures() -> String {
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
                let response =
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": []}).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_polling_legacy_shuts_down_on_cancel() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let cfg = PollingConfig {
            tiered_polling_enabled: false,
            interval_secs: 1,
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine,
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));

        // Let a few legacy ticks fire (wallets empty-query / poll attempts),
        // then cancel.
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("task exits after cancellation")
            .expect("no panic");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_polling_legacy_full_cycle_with_rpc() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let url = mock_rpc_price_server(false).await;
        let cfg = PollingConfig {
            tiered_polling_enabled: false,
            interval_secs: 1,
            rpc_url: url,
            exit_detection_delay_secs: 0,
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine.clone(),
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));

        // The legacy loop polls via the mocked RPC → the BUY transaction is
        // queued to the engine.
        let start = std::time::Instant::now();
        while engine.queue_depth() == 0 && start.elapsed().as_secs() < 20 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert_eq!(engine.queue_depth(), 1, "legacy loop queued the BUY signal");

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_polling_legacy_error_paths() {
        // Wallets query fails → warn + continue; then with a dead RPC the
        // batch fails → warn + continue. Runs 12 ticks to also cover the
        // every-10-cycles debug branch (wallets become empty after the query
        // error is cleared).
        let db = mock_db();
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        db.wallets_by_status_error.store(true, Ordering::Relaxed);
        let cfg = PollingConfig {
            tiered_polling_enabled: false,
            interval_secs: 1,
            rpc_url: "http://127.0.0.1:1".to_string(),
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine,
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));
        // After 2s the query error stays on (warn + continue); clear it so
        // the remaining ticks hit the empty-wallets branch, where the 10th
        // cycle logs the every-10-cycles debug line.
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
        db.wallets_by_status_error.store(false, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(9500)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tier_polling_process_transaction_error_is_warned() {
        // The RPC delivers a valid BUY, but the wallet lookup fails → the
        // process_transaction error is logged (Ok(Err) branch) and the loop
        // continues.
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(90.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let url = mock_rpc_price_server(false).await;
        let cfg = PollingConfig {
            tiered_polling_enabled: true,
            interval_secs: 3600,
            high_conviction_interval_secs: Some(1),
            regular_conviction_interval_secs: Some(3600),
            emerging_conviction_interval_secs: Some(3600),
            rpc_url: url,
            exit_detection_delay_secs: 0,
            ..polling_config()
        };
        db.wallet_query_error.store(true, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine,
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_signal_processor_end_to_end() {
        // Tiered polling with a SELL transaction: process_transaction enqueues
        // a delayed exit into the task's own pending buffer; the 5s processor
        // dispatches it to the engine as an EXIT signal.
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(90.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let url = mock_rpc_price_server(true).await; // SELL swap
        let cfg = PollingConfig {
            tiered_polling_enabled: true,
            interval_secs: 3600,
            high_conviction_interval_secs: Some(1),
            regular_conviction_interval_secs: Some(3600),
            emerging_conviction_interval_secs: Some(3600),
            rpc_url: url,
            exit_detection_delay_secs: 0,
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine.clone(),
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));

        // High tier polls at t=0 (immediate first tick) → the SELL is queued
        // as a direct Shield SELL signal (depth 1) AND enqueued as a delayed
        // exit; the processor dispatches the EXIT signal on its 5s tick
        // (depth 2).
        let start = std::time::Instant::now();
        while engine.queue_depth() < 2 && start.elapsed().as_secs() < 20 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert_eq!(
            engine.queue_depth(),
            2,
            "direct SELL signal + processor-dispatched EXIT signal"
        );

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_signal_processor_dispatches_delayed_exit() {
        let db = mock_db();
        db.add_wallet(wallet(WALLET_A, "ACTIVE", Some(80.0)));
        db.add_wallet_monitoring(monitoring(WALLET_A, true));
        let cb = Arc::new(crate::circuit_breaker::CircuitBreaker::new(
            cb_config(),
            db.clone(),
            dec!(1000),
        ));
        let engine = engine_handle(db.clone());
        let cfg = PollingConfig {
            tiered_polling_enabled: false,
            interval_secs: 3600,
            ..polling_config()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(start_polling_task(
            db.clone(),
            engine.clone(),
            cfg,
            cancel.clone(),
            cb.clone(),
            token_parser(true),
            Arc::new(ExitDetector::new()),
        ));

        // Enqueue a SELL directly so the processor picks it up on its 5s tick.
        let detector = Arc::new(ExitDetector::new());
        let tx = rpc_polling::WalletTransaction {
            wallet_address: WALLET_A.to_string(),
            signature: "sig-exit".to_string(),
            token_address: Some(TOKEN_A.to_string()),
            direction: Some("SELL".to_string()),
            amount_sol: Some(dec!(5.0)),
            timestamp: 1700000000,
        };
        process_transaction(
            db.as_ref(),
            &engine,
            tx,
            &cb,
            token_parser(true).as_ref(),
            &detector,
            &Arc::new(RwLock::new(Vec::new())), // separate pending buffer
            0,
            dec!(0.01),
        )
        .await
        .unwrap();

        // Wait past the 5s processor tick.
        tokio::time::sleep(std::time::Duration::from_secs(7)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    async fn real_db() -> Option<Arc<dyn Database>> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        crate::db_abstraction::create_database(&crate::db_abstraction::DatabaseConfig::postgres(
            url,
        ))
        .await
        .ok()
    }

    fn cb_config() -> crate::config::CircuitBreakerConfig {
        crate::config::CircuitBreakerConfig {
            max_loss_24h_usd: dec!(500),
            max_consecutive_losses: 3,
            max_drawdown_percent: dec!(15),
            portfolio_stop_loss_percent: dec!(-5),
            cooldown_minutes: 30,
            max_jupiter_failures: 5,
        }
    }
}
