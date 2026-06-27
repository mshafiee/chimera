//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

use axum::{
    middleware::{self as axum_middleware},
    routing::{get, post, put},
    Router,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;

use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction;
use chimera_operator::db_abstraction::ActivePositionEntry;
use chimera_operator::engine::{
    self, MarketRegimeDetector, MomentumExit, PortfolioHeat, PositionSizer, ProfitTargetAction,
    ProfitTargetManager, RecoveryManager, StopLossAction, StopLossManager, TipManager, VolumeCache,
};
use chimera_operator::handlers::{
    bulk_cleanup_webhooks,
    bulk_register_webhooks,
    disable_wallet_monitoring,
    enable_wallet_monitoring,
    export_trades,
    get_config,
    get_cost_metrics,
    get_health_check_details,
    get_market_conditions,
    get_market_regime,
    get_monitoring_status,
    get_performance_metrics,
    get_position,
    get_rate_limit_status,
    get_resources,
    get_scout_metrics,
    get_scout_status,
    get_budget_status,
    get_cache_stats,
    get_conviction_allocation,
    get_secrets,
    get_strategy_performance,
    get_wallet,
    get_wallet_monitoring_states,
    get_webhook_audit_log,
    // Webhook lifecycle handlers
    get_webhook_stats,
    get_wqs_distribution,
    health_check,
    health_simple,
    helius_webhook_handler,
    list_config_audit,
    list_dead_letter_queue,
    list_positions,
    list_trades,
    list_wallets,
    manual_health_check,
    manual_reconcile_webhooks,
    reset_circuit_breaker,
    retry_webhook_registration,
    roster_merge,
    roster_validate,
    toggle_wallet_webhook,
    trigger_scout_run,
    trip_circuit_breaker,
    update_config,
    update_reconciliation_metrics,
    update_secret_rotation_metrics,
    update_wallet,
    wallet_auth,
    webhook_handler,
    ws_handler,
    ApiState,
    AppState,
    OperationsState,
    RosterState,
    WalletAuthState,
    WebhookState,
    WsState,
};
use chimera_operator::metrics::{metrics_router, MetricsState};
use chimera_operator::middleware::{self, bearer_auth, AuthState, Role};
use chimera_operator::monitoring::{rate_limiter, HeliusClient, MonitoringState, SignalAggregator};
use chimera_operator::notifications::{self, NotificationEvent};
use chimera_operator::price_cache::PriceCache;
use chimera_operator::roster;
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::vault;
use chimera_operator::{Action, Signal, SignalPayload, Strategy};

async fn run_preflight(config: &AppConfig) -> anyhow::Result<()> {
    match config.trade_mode {
        chimera_operator::config::TradeMode::Paper => {
            let jupiter_url = format!(
                "{}/quote?inputMint=So11111111111111111111111111111111111111112&outputMint=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v&amount=100000000&slippageBps=50",
                config.jupiter.api_url
            );
            let client = reqwest::Client::new();
            let resp = client
                .get(&jupiter_url)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Pre-flight Jupiter probe failed: {}", e))?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "Pre-flight Jupiter probe returned HTTP {} — paper mode requires Jupiter API to be reachable",
                    resp.status()
                );
            }
            tracing::info!("Pre-flight passed: Jupiter API reachable (paper mode)");
        }
        chimera_operator::config::TradeMode::Devnet | chimera_operator::config::TradeMode::Live => {
            let secrets = chimera_operator::vault::load_secrets_with_fallback()
                .map_err(|e| anyhow::anyhow!("Pre-flight vault load failed: {}", e))?;
            let _keypair =
                chimera_operator::engine::transaction_builder::load_wallet_keypair(&secrets)
                    .map_err(|e| anyhow::anyhow!("Pre-flight keypair load failed: {}", e))?;
            let rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new(
                config.rpc.primary_url.clone(),
            );
            rpc_client
                .get_latest_blockhash()
                .await
                .map_err(|e| anyhow::anyhow!("Pre-flight RPC probe failed: {}", e))?;
            tracing::info!(
                "Pre-flight passed: vault, keypair, and RPC reachable ({})",
                config.trade_mode
            );
        }
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    init_tracing();

    // Load configuration
    let mut config = load_config()?;

    use chimera_operator::config::{resolve_trade_mode, TradeMode};

    let explicit_mode = {
        let mut mode: Option<TradeMode> = None;
        if let Ok(mode_str) = std::env::var("CHIMERA_TRADE_MODE") {
            mode = match mode_str.to_lowercase().as_str() {
                "devnet" => Some(TradeMode::Devnet),
                "paper" => Some(TradeMode::Paper),
                "live" => Some(TradeMode::Live),
                _ => {
                    tracing::warn!(provided = %mode_str, "Invalid CHIMERA_TRADE_MODE — must be devnet|paper|live. Ignoring.");
                    None
                }
            };
        }
        if let Ok(old_val) = std::env::var("CHIMERA_JUPITER__DEVNET_SIMULATION_MODE") {
            if (old_val == "true" || old_val == "1") && mode.is_none() {
                tracing::warn!("CHIMERA_JUPITER__DEVNET_SIMULATION_MODE is deprecated — use CHIMERA_TRADE_MODE=paper");
                mode = Some(TradeMode::Paper);
            }
        }
        mode
    };
    config.trade_mode =
        resolve_trade_mode(explicit_mode, config.trade_mode, &config.rpc.primary_url);

    match config.trade_mode {
        TradeMode::Paper => tracing::warn!("┌─────────────────────────────────────────┐"),
        _ => tracing::info!("┌─────────────────────────────────────────┐"),
    };
    tracing::info!("│  TRADE MODE: {:<28}  │", config.trade_mode.to_string());
    match config.trade_mode {
        TradeMode::Paper => tracing::warn!("│  NO REAL TRANSACTIONS WILL BE SUBMITTED │"),
        TradeMode::Devnet => tracing::info!("│  Transactions on DEVNET (test network)  │"),
        TradeMode::Live => tracing::info!("│  LIVE TRADING — REAL SOL AT RISK        │"),
    }
    tracing::info!("└─────────────────────────────────────────┘");

    match config.trade_mode {
        TradeMode::Paper => {
            tracing::info!("Paper mode: skipping vault validation (no keypair needed)");
        }
        TradeMode::Devnet | TradeMode::Live => {
            let _startup_secrets = vault::load_secrets_with_fallback()
                .map_err(|e| anyhow::anyhow!("Vault startup validation failed: {}", e))?;
            tracing::info!("Vault/secrets validated at startup");
        }
    }

    // Load API keys and JWT secret early for WebSocket state initialization
    let mut api_keys_map = std::collections::HashMap::new();
    for key_config in &config.security.api_keys {
        if let Ok(role) = key_config.role.parse::<Role>() {
            api_keys_map.insert(key_config.key.clone(), role);
            tracing::debug!(key_prefix = %&key_config.key[..key_config.key.len().min(8)], role = %role, "API key configured");
        } else {
            tracing::warn!(key_prefix = %&key_config.key[..key_config.key.len().min(8)], role = %key_config.role, "Invalid role in API key config");
        }
    }

    let chimera_env = std::env::var("CHIMERA_ENV").unwrap_or_default();
    let jwt_secret = match std::env::var("JWT_SECRET") {
        Ok(secret) => secret,
        Err(_) if chimera_env == "production" => {
            tracing::error!("JWT_SECRET environment variable must be set in production mode");
            return Err(anyhow::anyhow!(
                "JWT_SECRET environment variable is required in production mode but was not set"
            ));
        }
        Err(_) => {
            tracing::warn!(
                "JWT_SECRET not set — using development default (insecure, only for local testing)"
            );
            use std::fmt::Write;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut dev_secret = String::from("dev-");
            write!(&mut dev_secret, "{}", timestamp).unwrap();
            dev_secret
        }
    };

    // Initialize database
    let db_config = db_abstraction::DatabaseConfig {
        backend: db_abstraction::DatabaseBackend::SQLite,
        path: config.database.path.clone(),
        url: None,
        max_connections: config.database.max_connections,
        acquire_timeout_seconds: 30,
    };
    let db_pool = db_abstraction::create_database(&db_config).await?;
    db_pool.run_migrations().await?;
    db_pool.startup_integrity_check().await?;
    db_pool.recover_executing_trades().await?;
    tracing::info!("Database initialized");

    run_preflight(&config).await?;

    let cancel_token = CancellationToken::new();

    // Initialize WebSocket state with authentication (early initialization for circuit breaker)
    let ws_state = Arc::new(WsState::new(
        api_keys_map.clone(),
        jwt_secret.clone(),
        true, // Allow anonymous readonly for development dashboard
    ));

    // Initialize price cache
    let price_cache = match PriceCache::new() {
        Ok(cache) => Arc::new(cache),
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize price cache — HTTP client build failed");
            return Err(anyhow::anyhow!("Price cache initialization failed: {}", e));
        }
    };
    // Track SOL for volatility calculation
    price_cache.track_token("So11111111111111111111111111111111111111112");

    // Initialize volume cache for liquidity drop detection
    let _volume_cache = Arc::new(engine::volume_cache::VolumeCache::new());
    tracing::info!("✓ Volume Cache initialized for liquidity monitoring");

    // Validate webhook URL reachability if monitoring is enabled
    if let Some(ref monitoring_config) = config.monitoring {
        if monitoring_config.enabled {
            if let Some(ref webhook_url) = monitoring_config.helius_webhook_url {
                if !webhook_url.is_empty() {
                    match chimera_operator::monitoring::helius::validate_webhook_reachability(
                        webhook_url,
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::info!("Webhook URL validated successfully: {}", webhook_url);
                        }
                        Err(e) => {
                            tracing::warn!(
                                webhook_url = %webhook_url,
                                error = %e,
                                "Webhook URL validation failed — monitoring may not work correctly"
                            );
                            // Don't fail startup on webhook validation issues - log a warning instead
                            // The webhook may become available later, or monitoring may be optional
                        }
                    }
                }
            }
        }
    }

    // Initialize notification service
    let notifier = {
        let mut composite = notifications::CompositeNotifier::new();

        // Add Discord notifier if configured via environment variable
        if let Some(discord) = notifications::DiscordNotifier::from_env() {
            composite.add_service(Arc::new(discord));
            tracing::info!("Discord notifications enabled");
        }

        // Add Telegram notifier if configured
        if config.notifications.telegram.enabled {
            let telegram_config = notifications::telegram::TelegramConfig {
                bot_token: std::env::var("TELEGRAM_BOT_TOKEN")
                    .unwrap_or_else(|_| config.notifications.telegram.bot_token.clone()),
                chat_id: std::env::var("TELEGRAM_CHAT_ID")
                    .unwrap_or_else(|_| config.notifications.telegram.chat_id.clone()),
                enabled: true,
                rate_limit_seconds: config.notifications.telegram.rate_limit_seconds,
            };

            if !telegram_config.bot_token.is_empty() && !telegram_config.chat_id.is_empty() {
                composite.add_service(Arc::new(notifications::TelegramNotifier::new(
                    telegram_config,
                )));
                tracing::info!("Telegram notifications enabled");
            } else {
                tracing::warn!(
                    "Telegram notifications enabled in config but bot_token/chat_id not set"
                );
            }
        }

        composite.set_trade_mode(&config.trade_mode.to_string());

        Arc::new(composite)
    };
    tracing::info!("Notification service initialized");

    // Initialize circuit breaker
    let circuit_breaker = Arc::new(
        CircuitBreaker::new_with_ws(
            config.circuit_breakers.clone(),
            db_pool.clone(),
            Some(ws_state.clone()),
            config.position_sizing.total_capital_sol,
        )
        .with_price_cache(price_cache.clone()),
    );

    // Wire notification service into circuit breaker so manual/auto trips send push alerts
    circuit_breaker.set_notifier(notifier.clone());

    // FIX [R-C1]: Restore persisted circuit breaker state from DB before accepting connections.
    // This ensures that a trip persisted before last restart is re-applied and evaluate()
    // runs so cooldown expiry / breach re-evaluation happen immediately on startup.
    if let Err(e) = circuit_breaker.restore_from_db().await {
        tracing::error!(error = %e, "Failed to restore circuit breaker state from DB — starting Active");
    }

    // Restore kill-switch if it was active before last restart.
    // Reads from kill_switch_state (single-row UPSERT table) which is written synchronously
    // by the kill-switch API handler before tripping the circuit breaker in memory.
    {
        let is_active = match db_pool.get_kill_switch_state().await {
            Ok(state) => {
                tracing::info!("Kill-switch state loaded: {}", state.state);
                state.state == "ACTIVE"
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "FAIL-SAFE: Failed to read kill-switch state — assuming ACTIVE to prevent unintended trading"
                );
                true
            }
        };

        if is_active {
            tracing::warn!("Kill-switch was active before restart — re-tripping circuit breaker");
            if let Err(e) = circuit_breaker
                .manual_trip(
                    "SYSTEM_RESTART_RESTORE",
                    "Kill-switch was active before restart".to_string(),
                )
                .await
            {
                tracing::error!(error = %e, "CRITICAL: Failed to restore kill-switch — ABORTING STARTUP");
                return Err(anyhow::anyhow!("Failed to restore kill-switch: {}", e));
            }
        }
    }

    // Initialize tip manager
    let tip_manager = Arc::new(TipManager::new(config.jito.clone(), db_pool.clone()));
    if let Err(e) = tip_manager.init().await {
        tracing::error!(error = %e, "Failed to initialize tip manager — operating in cold-start mode");
    }

    // Initialize token parser (needed for slow-path safety checks in engine)
    // Create RPC rate limiter for token metadata fetching (simulation calls are weighted)
    let rpc_rate_limiter = Arc::new(rate_limiter::RateLimiter::new(
        config.rpc.rate_limit_per_second,
        1,
    ));
    let token_cache = Arc::new(TokenCache::new(
        config.token_safety.cache_capacity,
        config.token_safety.cache_ttl_seconds,
    ));
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            &config.rpc.primary_url,
            Some(rpc_rate_limiter.clone()),
            config.jupiter.api_url.clone(),
        )
        .with_unlisted_heuristic(config.token_safety.allow_unlisted_heuristic),
    );
    let token_safety_config = TokenSafetyConfig {
        freeze_authority_whitelist: config
            .token_safety
            .freeze_authority_whitelist
            .iter()
            .cloned()
            .collect(),
        mint_authority_whitelist: config
            .token_safety
            .mint_authority_whitelist
            .iter()
            .cloned()
            .collect(),
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        honeypot_detection_enabled: config.token_safety.honeypot_detection_enabled,
    };
    let token_parser = Arc::new(TokenParser::new(
        token_safety_config,
        token_cache.clone(),
        token_fetcher.clone(),
    ));
    tracing::info!("Token parser initialized");

    let portfolio_heat = Arc::new(PortfolioHeat::new(
        db_pool.clone(),
        config.position_sizing.total_capital_sol,
    ));
    tracing::info!(
        total_capital_sol = ?config.position_sizing.total_capital_sol,
        "Portfolio heat manager initialized"
    );

    // Create engine
    let (engine, _engine_handle) =
        engine::Engine::new_with_extras_tip_manager_price_cache_and_token_parser(
            config.clone(),
            db_pool.clone(),
            notifier.clone(),
            None,
            Some(ws_state.clone()),
            Some(tip_manager.clone()),
            Some(price_cache.clone()),
            Some(token_parser.clone()),
            Some(portfolio_heat.clone()),
        );
    tracing::info!("Engine created");

    let mut task_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Spawn engine
    task_handles.push(tokio::spawn(async move {
        engine.run().await;
    }));
    tracing::info!("Engine task spawned");

    // Spawn recovery manager
    let recovery_manager = Arc::new(RecoveryManager::new_with_rpc(
        db_pool.clone(),
        _engine_handle.clone(),
        Some(ws_state.clone()),
    ));
    let recovery_clone = recovery_manager.clone();
    task_handles.push(tokio::spawn(async move {
        recovery_clone.start_background_task().await;
    }));
    tracing::info!("Recovery manager task spawned");

    // Periodic EXECUTING cleanup
    {
        let exec_cleanup_db = db_pool.clone();
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match exec_cleanup_db.recover_executing_trades().await {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        count = n,
                        "Periodic sweep: recovered stuck EXECUTING trades to FAILED"
                    ),
                    Err(e) => tracing::error!(error = %e, "Periodic EXECUTING cleanup failed"),
                }
            }
        }));
    }

    // Spawn PnL refresh task — updates unrealized_pnl_percent every 30 seconds for active positions
    {
        let pnl_db = db_pool.clone();
        let pnl_pc = price_cache.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                match pnl_db.get_active_position_tokens().await {
                    Ok(positions) => {
                        for pos in positions {
                            if let Some(current_usd) = pnl_pc.get_price_usd(&pos.token_address) {
                                let entry = if pos.entry_price.is_zero() {
                                    current_usd
                                } else {
                                    pos.entry_price
                                };
                                // Get current SOL/USD price for converting USD prices to SOL terms
                                let current_sol_price = pnl_pc.get_sol_price_usd();
                                let pnl_sol = match (pos.entry_sol_price_usd, current_sol_price) {
                                    (Some(entry_sol), Some(curr_sol))
                                        if !entry_sol.is_zero() && !curr_sol.is_zero() =>
                                    {
                                        // Convert both entry and current USD prices to SOL-denominated terms
                                        let entry_price_sol = pos.entry_price / entry_sol;
                                        let current_price_sol = current_usd / curr_sol;
                                        let token_amount = pos.entry_amount_sol / entry_price_sol;
                                        (current_price_sol - entry_price_sol) * token_amount
                                    }
                                    // Fallback: if SOL price unavailable, compute with what we have
                                    _ => {
                                        if !entry.is_zero() {
                                            let usd_pnl = current_usd - entry;
                                            // Approximate SOL PnL using entry SOL price if available
                                            // or just use the USD difference scaled by entry ratio
                                            match pos.entry_sol_price_usd {
                                                Some(entry_sol) if !entry_sol.is_zero() => {
                                                    let pnl_fraction = usd_pnl / entry;
                                                    // Scale USD return to SOL terms
                                                    pnl_fraction
                                                        * pos.entry_amount_sol
                                                        * (entry / entry_sol)
                                                }
                                                _ => {
                                                    // Last resort: USD difference (misleading but won't crash)
                                                    tracing::warn!(
                                                        token = %pos.token_address,
                                                        "SOL price unavailable for PnL calc — using approximate value"
                                                    );
                                                    (current_usd - entry) / entry
                                                        * pos.entry_amount_sol
                                                }
                                            }
                                        } else {
                                            rust_decimal::Decimal::ZERO
                                        }
                                    }
                                };
                                let pnl_pct = if !entry.is_zero() {
                                    (current_usd - entry) / entry * rust_decimal::Decimal::from(100)
                                } else {
                                    rust_decimal::Decimal::ZERO
                                };
                                if let Err(e) = pnl_db
                                    .update_position_unrealized_pnl(
                                        &pos.trade_uuid,
                                        current_usd,
                                        pnl_sol,
                                        pnl_pct,
                                    )
                                    .await
                                {
                                    tracing::warn!(error = %e, token = %pos.token_address,
                                        "PnL refresh: failed to update position");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PnL refresh: failed to fetch active positions")
                    }
                }
            }
        });
    }
    tracing::info!("PnL refresh task spawned");

    // Spawn circuit breaker evaluation task with notification support
    let circuit_breaker_clone = circuit_breaker.clone();
    let notifier_cb = notifier.clone();
    let notify_rules = config.notifications.rules.clone();
    let cb_token = cancel_token.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut was_tripped = false;

        loop {
            tokio::select! {
                _ = cb_token.cancelled() => {
                    tracing::info!("Shutting down circuit breaker task");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = circuit_breaker_clone.evaluate().await {
                        tracing::error!(error = %e, "Circuit breaker evaluation failed");
                    }

                    // Recovery notification: trip notification is sent directly
                    // by trip() in circuit_breaker.rs (immediate, including manual trips).
                    let is_active = circuit_breaker_clone.is_trading_allowed();
                    if is_active && was_tripped {
                        was_tripped = false;
                        if notify_rules.circuit_breaker_triggered {
                            notifier_cb
                                .notify(NotificationEvent::CircuitBreakerRecovered)
                                .await;
                        }
                    } else if !is_active {
                        was_tripped = true;
                    }
                }
            }
        }
    });
    tracing::info!("Circuit breaker task spawned");

    // Spawn DLQ retry worker task
    let dlq_token = cancel_token.clone();
    let dlq_pool = db_pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // Every 5 minutes
        loop {
            tokio::select! {
                _ = dlq_token.cancelled() => {
                    tracing::info!("Shutting down DLQ retry worker");
                    break;
                }
                _ = interval.tick() => {
                    // Fetch retryable items from DLQ via the Database trait
                    match dlq_pool.get_retryable_dlq_items(50).await {
                        Ok(items) => {
                            const MAX_DLQ_RETRIES: i64 = 3;
                            tracing::info!(count = items.len(), "Processing DLQ retry items");

                            // Phase 1: Increment retry counts for all items
                            let update_params: Vec<chimera_operator::db_abstraction::UpdateDlqItemParams> = items
                                .iter()
                                .map(|item| {
                                    let new_count = item.retry_count + 1;
                                    let can_still_retry = new_count < MAX_DLQ_RETRIES;
                                    chimera_operator::db_abstraction::UpdateDlqItemParams {
                                        trade_uuid: item.trade_uuid.clone(),
                                        retry_count: new_count,
                                        can_retry: can_still_retry,
                                        mark_processed: false,
                                    }
                                })
                                .collect();

                            if let Err(e) = dlq_pool.update_dlq_items_batch(update_params).await {
                                tracing::error!(error = %e, "Failed to batch update DLQ retry counts");
                                continue;
                            }

                            // Phase 2: Parse payloads and mark successful ones as processed
                            let mut processed_items: Vec<chimera_operator::db_abstraction::UpdateDlqItemParams> = Vec::new();
                            let mut status_updates: Vec<chimera_operator::db_abstraction::UpdateTradeStatus> = Vec::new();

                            for item in &items {
                                let new_count = item.retry_count + 1;
                                let can_still_retry = new_count < MAX_DLQ_RETRIES;

                                if !can_still_retry {
                                    tracing::warn!(
                                        uuid = %item.trade_uuid,
                                        retry_count = new_count,
                                        "DLQ item permanently failed after max retries"
                                    );
                                    continue;
                                }

                                // Parse payload and prepare for re-queue
                                match serde_json::from_str::<serde_json::Value>(&item.payload) {
                                    Ok(_) => {
                                        processed_items.push(chimera_operator::db_abstraction::UpdateDlqItemParams {
                                            trade_uuid: item.trade_uuid.clone(),
                                            retry_count: new_count,
                                            can_retry: can_still_retry,
                                            mark_processed: true,
                                        });
                                        status_updates.push(chimera_operator::db_abstraction::UpdateTradeStatus {
                                            trade_uuid: item.trade_uuid.clone(),
                                            status: "RETRY".to_string(),
                                            tx_signature: None,
                                            error_message: None,
                                            network_fee_sol: None,
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, uuid = %item.trade_uuid, "Failed to parse DLQ payload");
                                    }
                                }
                            }

                            // Phase 3: Batch mark items as processed
                            if !processed_items.is_empty() {
                                if let Err(e) = dlq_pool.update_dlq_items_batch(processed_items).await {
                                    tracing::error!(error = %e, "Failed to batch mark DLQ items as processed");
                                } else {
                                    // Phase 4: Batch update trade statuses
                                    let mut updated_count = 0;
                                    for status_update in &status_updates {
                                        if dlq_pool.update_trade_status(status_update).await.is_ok() {
                                            updated_count += 1;
                                        }
                                    }
                                    tracing::info!("DLQ batch: {}/{} items re-queued to RETRY", updated_count, status_updates.len());
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to fetch DLQ items");
                        }
                    }
                }
            }
        }
    });
    tracing::info!("DLQ retry worker spawned");

    // Spawn price cache updater
    let price_cache_clone = price_cache.clone();
    tokio::spawn(async move {
        price_cache_clone.start_updater().await;
        // start_updater only returns on error or shutdown; log so silent crashes are visible.
        tracing::error!("Price cache updater exited — token price data will become stale. All price-dependent checks (stop-loss, circuit breaker USD thresholds) are now degraded.");
    });

    // Spawn daily summary notification task
    let notifier_daily = notifier.clone();
    let db_pool_daily = db_pool.clone();
    let daily_config = config.notifications.daily_summary.clone();
    let notify_daily_enabled = config.notifications.rules.daily_summary;

    // Create a specific cancellation token for this if needed, or rely on main process exit
    tokio::spawn(async move {
        if !daily_config.enabled || !notify_daily_enabled {
            return;
        }

        tracing::info!("Daily summary task started");

        loop {
            let now = Utc::now();
            let target_hour = daily_config.hour_utc as u32;
            let target_minute = daily_config.minute as u32;

            let mut next_run = now
                .date_naive()
                .and_hms_opt(target_hour, target_minute, 0)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        target_hour,
                        target_minute,
                        "Invalid daily_summary time in config, defaulting to 00:00 UTC"
                    );
                    now.date_naive()
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight always valid")
                })
                .and_utc();

            if next_run <= now {
                next_run += chrono::Duration::days(1);
            }

            let sleep_duration = (next_run - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(3600));
            tokio::time::sleep(sleep_duration).await;

            match generate_daily_summary(db_pool_daily.as_ref()).await {
                Ok((pnl_usd, trade_count, win_rate)) => {
                    notifier_daily
                        .notify(NotificationEvent::DailySummary {
                            pnl_usd,
                            trade_count,
                            win_rate,
                        })
                        .await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to generate daily summary");
                }
            }
        }
    });

    // Spawn TTL expiration background task
    let db_pool_ttl = db_pool.clone();
    let ttl_token = cancel_token.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = ttl_token.cancelled() => break,
                _ = interval.tick() => {
                    match db_pool_ttl.get_expired_ttl_wallets().await {
                        Ok(expired_wallets) => {
                            for address in expired_wallets {
                                tracing::info!(wallet = %address, "Demoting wallet due to TTL expiration");
                                if let Err(e) = db_pool_ttl.demote_wallet(
                                    &address,
                                    "Auto-demoted: TTL expired",
                                )
                                .await
                                {
                                    tracing::error!(wallet = %address, error = %e, "Failed to demote wallet");
                                } else {
                                    let _ = db_pool_ttl.log_config_change(
                                        &format!("wallet:{}", address),
                                        Some("ACTIVE"),
                                        "CANDIDATE",
                                        "SYSTEM_TTL",
                                        Some("TTL expired"),
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to check TTL expirations");
                        }
                    }
                }
            }
        }
    });

    // Spawn SIGHUP handler for roster merge (Unix only)
    #[cfg(unix)]
    {
        let db_pool_sighup = db_pool.clone();
        let roster_path = config
            .database
            .path
            .parent()
            .map(|p| p.join("roster_new.db"))
            .unwrap_or_else(|| PathBuf::from("roster_new.db"));

        tokio::spawn(async move {
            let mut sighup = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to register SIGHUP handler");
                    return;
                }
            };

            tracing::info!(
                roster_path = %roster_path.display(),
                "SIGHUP handler registered for roster merge"
            );

            loop {
                sighup.recv().await;
                tracing::info!("Received SIGHUP, triggering roster merge");

                match roster::merge_roster(&db_pool_sighup, &roster_path).await {
                    Ok(result) => {
                        tracing::info!(
                            wallets_merged = result.wallets_merged,
                            wallets_removed = result.wallets_removed,
                            "Roster merge completed successfully"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Roster merge failed");
                    }
                }
            }
        });
        tracing::info!("SIGHUP roster merge handler started");
    }

    // Spawn periodic RPC health check task
    let engine_handle_rpc = _engine_handle.clone();
    let rpc_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = rpc_token.cancelled() => break,
                _ = interval.tick() => {
                    engine_handle_rpc.refresh_rpc_health().await;
                }
            }
        }
    });
    tracing::info!("RPC health check task started");

    // Spawn periodic memory and disk pressure monitoring task
    let config_clone = config.clone();
    let monitor_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = monitor_token.cancelled() => break,
                _ = interval.tick() => {
                    if config_clone.degradation.memory_monitoring_enabled {
                        match crate::engine::check_memory_pressure().await {
                            Ok(usage) => {
                                if usage >= config_clone.degradation.memory_pressure_threshold {
                                    tracing::warn!(
                                        memory_usage_pct = usage * 100.0,
                                        threshold_pct = config_clone.degradation.memory_pressure_threshold * 100.0,
                                        "Memory pressure detected"
                                    );
                                } else {
                                    tracing::debug!(
                                        memory_usage_pct = usage * 100.0,
                                        "Memory usage normal"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to check memory pressure");
                            }
                        }
                    }

                    if config_clone.degradation.disk_monitoring_enabled {
                        // Check disk space in current directory
                        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                        match crate::engine::check_disk_space(&current_dir).await {
                            Ok(free_space) => {
                                if free_space <= config_clone.degradation.disk_space_warning_threshold {
                                    tracing::warn!(
                                        free_space_pct = free_space * 100.0,
                                        threshold_pct = config_clone.degradation.disk_space_warning_threshold * 100.0,
                                        "Disk space low"
                                    );
                                } else {
                                    tracing::debug!(
                                        free_space_pct = free_space * 100.0,
                                        "Disk space normal"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to check disk space");
                            }
                        }
                    }
                }
            }
        }
    });
    tracing::info!("Memory and disk pressure monitoring task started");

    // Spawn periodic log pruning task
    let config_prune = config.clone();
    let prune_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 minutes
        loop {
            tokio::select! {
                _ = prune_token.cancelled() => break,
                _ = interval.tick() => {
                    if config_prune.degradation.log_pruning_enabled {
                        let log_dir = std::path::PathBuf::from("logs");
                        let max_age_days = 7; // Default: prune logs older than 7 days
                        match crate::engine::prune_logs_if_needed(&log_dir, max_age_days).await {
                            Ok(_) => {
                                tracing::debug!("Log pruning check completed");
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to prune logs");
                            }
                        }
                    }
                }
            }
        }
    });
    tracing::info!("Log pruning task started");

    // Build position risk managers and spawn monitoring loop
    let market_regime_detector = Arc::new(MarketRegimeDetector::new(price_cache.clone()));
    // Create SignalAggregator early so the stop-loss manager can read consensus from
    // its in-memory cache instead of issuing a DB query on every 5-second position tick.
    let signal_aggregator = Arc::new(SignalAggregator::new(db_pool.clone()));
    {
        let stop_loss_mgr = Arc::new(StopLossManager::new(
            db_pool.clone(),
            Arc::new(config.profit_management.clone()),
            price_cache.clone(),
        ));
        stop_loss_mgr
            .set_signal_aggregator(signal_aggregator.clone())
            .await;
        let volume_cache = Arc::new(VolumeCache::new());
        let momentum_exit = Arc::new(MomentumExit::with_volume_cache(
            db_pool.clone(),
            price_cache.clone(),
            volume_cache,
            config.profit_management.wick_protection_secs,
        ));
        let profit_target_mgr = Arc::new(ProfitTargetManager::with_extras(
            db_pool.clone(),
            Arc::new(config.profit_management.clone()),
            price_cache.clone(),
            Some(momentum_exit),
            Some(market_regime_detector.clone()),
        ));

        // Dedicated HWM sweep task — runs every 5 minutes independent of the position
        // monitoring loop so memory is reclaimed even if that loop stalls or panics.
        {
            let sweep_pt = Arc::clone(&profit_target_mgr);
            let sweep_token = cancel_token.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
                loop {
                    tokio::select! {
                        _ = sweep_token.cancelled() => break,
                        _ = interval.tick() => {
                            let removed = sweep_pt.sweep_hwm_stale_entries().await;
                            if removed > 0 {
                                tracing::debug!(removed, "HWM sweep: removed stale entries");
                            }
                        }
                    }
                }
            });
            tracing::info!("HWM sweep task spawned (5-min interval)");
        }

        let monitor_db = db_pool.clone();
        let monitor_sl = stop_loss_mgr;
        let monitor_pt = profit_target_mgr;
        let monitor_engine = _engine_handle.clone();
        let monitor_token = cancel_token.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut last_checked: std::collections::HashMap<String, std::time::Instant> =
                std::collections::HashMap::new();
            let mut db_fail_count: u32 = 0;
            loop {
                tokio::select! {
                    _ = monitor_token.cancelled() => {
                        tracing::info!("Shutting down position monitoring task");
                        break;
                    }
                    _ = interval.tick() => {
                        let positions = match monitor_db.get_active_positions_with_entry().await {
                            Ok(p) => { db_fail_count = 0; p }
                            Err(e) => {
                                db_fail_count += 1;
                                if db_fail_count >= 3 {
                                    tracing::error!(
                                        consecutive_failures = db_fail_count,
                                        error = %e,
                                        "Position monitor: repeated DB failures — positions not being monitored"
                                    );
                                } else {
                                    tracing::warn!(error = %e, "Position monitor: DB query failed, will retry next tick");
                                }
                                continue;
                            }
                        };

                        let now = std::time::Instant::now();
                        for pos in &positions {
                            if let Some(&last) = last_checked.get(&pos.trade_uuid) {
                                if now.duration_since(last) > std::time::Duration::from_secs(60) {
                                    tracing::error!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        elapsed_secs = %now.duration_since(last).as_secs(),
                                        "MONITOR_STALENESS_ALERT: Position monitoring is stale (not checked for > 60s)"
                                    );
                                }
                            }
                        }

                        for pos in &positions {
                            last_checked.insert(pos.trade_uuid.clone(), now);
                        }

                        last_checked.retain(|uuid, _| positions.iter().any(|p| &p.trade_uuid == uuid));

                        for pos in positions {
                            // Check stop-loss first (higher priority)
                            let sl_action = monitor_sl.check_stop_loss(
                                &pos.trade_uuid,
                                &pos.wallet_address,
                                pos.entry_price,
                                &pos.token_address,
                                pos.entry_time,
                            ).await;

                            if sl_action == StopLossAction::Exit {
                                tracing::warn!(
                                    trade_uuid = %pos.trade_uuid,
                                    token = %pos.token_address,
                                    "Stop-loss triggered, queuing EXIT signal"
                                );
                                let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                                if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                    tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Stop-loss signal failed — will retry next monitoring cycle");
                                    continue;
                                }
                                monitor_pt.remove_position(&pos.trade_uuid).await;
                                continue;
                            }

                            // Register position with profit target manager (idempotent).
                            // Pass the actual trade open time so time-based exits fire
                            // correctly even after a restart.
                            let entry_st: std::time::SystemTime = pos.entry_time.into();
                            monitor_pt.register_position(
                                &pos.trade_uuid,
                                pos.entry_price,
                                pos.entry_amount_sol,
                                &pos.token_address,
                                entry_st,
                            ).await;

                            // Check profit targets
                            match monitor_pt.check_targets(&pos.trade_uuid, &pos.token_address, &pos.strategy).await {
                                ProfitTargetAction::FullExit => {
                                    tracing::info!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        "Full profit target reached, queuing EXIT signal"
                                    );
                                    let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                                    if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                        tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Full profit target signal failed — will retry");
                                    } else {
                                        monitor_pt.remove_position(&pos.trade_uuid).await;
                                    }
                                }
                                ProfitTargetAction::ExitAmount(amount_sol) => {
                                    tracing::info!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        amount_sol = %amount_sol,
                                        "Partial profit target reached, queuing partial EXIT signal"
                                    );
                                    let signal = build_exit_signal_amount(&pos, amount_sol);
                                    if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                        tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Partial profit target signal failed — will retry");
                                    }
                                }
                                ProfitTargetAction::None => {}
                            }
                        }
                    }
                }
            }
        });
        tracing::info!("Position monitoring task started");
    }

    // FIX [B-M3]: Removed duplicate wallet TTL expiration task (3600s interval).
    // The 60s interval task above (around line 505) already handles TTL expiration.
    // Having a second task at 60-minute intervals duplicated demote_wallet calls.

    // Create metrics state (shared between task and router)
    // If metrics initialization fails, we log the error and continue with a degraded state.
    // The /metrics endpoint will return an error, but the core service remains functional.
    let metrics_state = match MetricsState::new() {
        Ok(state) => Arc::new(state),
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize metrics system — /metrics endpoint unavailable, core service will continue");
            // Return early since we can't run without metrics state
            return Err(anyhow::anyhow!("Metrics initialization failed: {}", e));
        }
    };

    // Wire Prometheus metrics into circuit breaker for event-driven updates
    circuit_breaker.set_metrics(
        metrics_state.circuit_breaker_state.clone(),
        metrics_state.circuit_breaker_trips.clone(),
    );

    // Create exit detector early for use in both polling task and monitoring state
    let exit_detector = chimera_operator::monitoring::ExitDetector::new()
        .with_db(db_pool.clone());
    let exit_detector = Arc::new(exit_detector);

    // Spawn metrics update task
    let metrics_state_clone = metrics_state.clone();
    let circuit_breaker_clone = circuit_breaker.clone();
    let db_pool_metrics = db_pool.clone();
    let engine_handle_metrics = _engine_handle.clone();
    let ws_state_metrics = ws_state.clone();
    let metrics_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = metrics_token.cancelled() => break,
                _ = interval.tick() => {
                    // CB gauge is updated event-driven by circuit_breaker.rs — no polling needed.
                    let is_active = circuit_breaker_clone.is_trading_allowed();

                    // Update RPC health
                    if let Some(rpc_health) = engine_handle_metrics.get_rpc_health().await {
                        metrics_state_clone
                            .rpc_health
                            .set(if rpc_health.healthy { 1 } else { 0 });
                    }

                    // Update active positions count
                    if let Ok(positions) = db_pool_metrics.get_active_positions().await {
                        let count = positions.len() as i64;
                        metrics_state_clone.active_positions.set(count);
                    }

                    // Update total trades count
                    if let Ok(count) = db_pool_metrics.count_trades_filtered(None, None, None, None, None).await {
                        metrics_state_clone.total_trades.set(count);
                    }

                    // Broadcast health update via WebSocket
                    ws_state_metrics.broadcast(chimera_operator::handlers::WsEvent::HealthUpdate(
                        chimera_operator::handlers::HealthUpdateData {
                            status: "healthy".to_string(), // Could be more sophisticated
                            queue_depth: engine_handle_metrics.queue_depth(),
                            trading_allowed: is_active,
                        },
                    ));
                }
            }
        }
    });
    tracing::info!("Metrics update task started");

    // Start RPC polling task if enabled
    if config
        .monitoring
        .as_ref()
        .map(|m| m.rpc_polling_enabled)
        .unwrap_or(false)
    {
        let interval_secs = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_interval_secs)
            .unwrap_or(8);
        let batch_size = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_batch_size)
            .unwrap_or(6);
        let rate_limit = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_rate_limit)
            .unwrap_or(40);

        let polling_config = chimera_operator::monitoring::PollingConfig {
            interval_secs,
            batch_size,
            rpc_url: config.rpc.primary_url.clone(),
            rate_limit,
        };

        let polling_db = db_pool.clone();
        let polling_engine = _engine_handle.clone();
        let polling_token = cancel_token.clone();
        let polling_cb = circuit_breaker.clone();
        let polling_tp = token_parser.clone();
        let polling_ed = exit_detector.clone();

        tokio::spawn(async move {
            chimera_operator::monitoring::start_polling_task(
                polling_db,
                polling_engine,
                polling_config,
                polling_token,
                polling_cb,
                polling_tp,
                polling_ed,
            )
            .await;
        });

        tracing::info!(interval_secs, batch_size, "RPC polling task started");
    } else {
        tracing::info!("RPC polling disabled in configuration");
    }

    // Spawn market regime price history update task (every 5 minutes)
    {
        let regime_token = cancel_token.clone();
        let detector_clone = market_regime_detector.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                tokio::select! {
                    _ = regime_token.cancelled() => break,
                    _ = interval.tick() => {
                        detector_clone.update_price_history().await;
                    }
                }
            }
        });
    }

    tracing::info!("All background tasks spawned");

    // Now create the FULL router with all routes
    tracing::info!("Creating full router with states...");

    let app_state = Arc::new(AppState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        started_at: Utc::now(),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
        trade_mode: config.trade_mode.to_string().to_lowercase(),
    });

    // signal_aggregator was created earlier (before stop_loss_mgr) so it could be wired
    // into the stop-loss manager's consensus cache. Reuse it here.
    let helius_client: Option<Arc<HeliusClient>> = HeliusClient::new(
        config
            .monitoring
            .as_ref()
            .and_then(|m| m.helius_api_key.clone())
            .unwrap_or_default(),
    )
    .map(Arc::new)
    .map_err(|e| tracing::warn!(error = %e, "HeliusClient unavailable, signal quality limited"))
    .ok();

    // Create webhook API rate limiter for lifecycle management operations
    let webhook_api_rate_limiter: Arc<rate_limiter::RateLimiter> =
        Arc::new(rate_limiter::RateLimiter::new(
            config
                .monitoring
                .as_ref()
                .map(|m| m.webhook_processing_rate_limit)
                .unwrap_or(40),
            1,
        ));

    // Spawn webhook health monitoring task
    if let Some(ref monitoring_config) = config.monitoring {
        if let Some(ref webhook_lifecycle_config) = monitoring_config.webhook_lifecycle {
            if webhook_lifecycle_config.health_check_interval_secs > 0 {
                if let Some(ref helius) = helius_client {
                    let webhook_db = db_pool.clone();
                    let webhook_helius = helius.clone();
                    let webhook_limiter = webhook_api_rate_limiter.clone();
                    let webhook_token = cancel_token.clone();
                    let webhook_url = monitoring_config
                        .helius_webhook_url
                        .clone()
                        .unwrap_or_default();

                    let health_config = chimera_operator::monitoring::WebhookHealthConfig {
                        check_interval_secs: webhook_lifecycle_config.health_check_interval_secs,
                        stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                        webhook_url: webhook_url.clone(),
                    };

                    tokio::spawn(async move {
                        chimera_operator::monitoring::webhook_health_task::start_webhook_health_task(
                            webhook_db,
                            webhook_helius,
                            webhook_limiter,
                            health_config,
                            webhook_token,
                        )
                        .await;
                    });

                    tracing::info!(
                        interval_secs = webhook_lifecycle_config.health_check_interval_secs,
                        "Webhook health monitoring task started"
                    );
                }
            }
        }
    }

    // Create API state
    let api_state = Arc::new(ApiState {
        db: db_pool.clone(),
        circuit_breaker: circuit_breaker.clone(),
        config: Arc::new(tokio::sync::RwLock::new(config.clone())),
        notifier: notifier.clone(),
        engine: Some(Arc::new(_engine_handle.clone())),
        metrics: metrics_state.clone(),
        signal_aggregator: Some(signal_aggregator.clone()),
        market_regime_detector: Some(market_regime_detector.clone()),
        helius_client: helius_client.clone(),
        webhook_rate_limiter: Some(webhook_api_rate_limiter.clone()),
    });

    // Run startup webhook management check
    // This ensures all ACTIVE wallets have registered webhooks before server starts
    if config
        .monitoring
        .as_ref()
        .map(|m| m.enabled)
        .unwrap_or(false)
    {
        if let Some(webhook_lifecycle_config) = config
            .monitoring
            .as_ref()
            .and_then(|m| m.webhook_lifecycle.as_ref())
        {
            if webhook_lifecycle_config.auto_register_enabled {
                if let Some(ref startup_helius) = helius_client {
                    let startup_db = db_pool.clone();
                    let startup_rate_limiter = webhook_api_rate_limiter.clone();
                    let startup_webhook_url = config
                        .monitoring
                        .as_ref()
                        .and_then(|m| m.helius_webhook_url.clone())
                        .unwrap_or_default();

                    let startup_config = chimera_operator::monitoring::WebhookHealthConfig {
                        check_interval_secs: webhook_lifecycle_config.health_check_interval_secs,
                        stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                        webhook_url: startup_webhook_url,
                    };

                    tracing::info!("Running startup webhook check...");

                    let startup_result = chimera_operator::monitoring::webhook_health_task::run_startup_webhook_check(
                        startup_db,
                        startup_helius.clone(),
                        startup_rate_limiter,
                        startup_config,
                    ).await;

                    match startup_result {
                        Ok(result) => {
                            tracing::info!(
                                wallets_checked = result.wallets_checked,
                                registered = result.registered,
                                orphaned = result.orphaned,
                                cleaned_up = result.cleaned_up,
                                failed = result.failed,
                                duration_ms = result.duration_ms,
                                "Startup webhook check completed"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Startup webhook check failed");
                        }
                    }
                } else {
                    tracing::info!("Helius client not available, skipping startup webhook check");
                }

                // Spawn Helius webhook reconciliation as background task
                // This runs asynchronously and does NOT delay startup
                if webhook_lifecycle_config.helius_reconciliation_enabled {
                    if let Some(ref reconcile_helius) = helius_client {
                        let reconcile_db = db_pool.clone();
                        let reconcile_helius_client = reconcile_helius.clone();
                        let reconcile_rate_limiter = webhook_api_rate_limiter.clone();
                        let reconcile_webhook_url = config
                            .monitoring
                            .as_ref()
                            .and_then(|m| m.helius_webhook_url.clone())
                            .unwrap_or_default();
                        let reconcile_config = chimera_operator::monitoring::WebhookHealthConfig {
                            check_interval_secs: webhook_lifecycle_config
                                .health_check_interval_secs,
                            stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                            webhook_url: reconcile_webhook_url,
                        };

                        tokio::spawn(async move {
                            tracing::info!("Helius webhook reconciliation task started (async)");
                            match chimera_operator::monitoring::webhook_health_task::reconcile_helius_webhooks_async(
                                reconcile_db,
                                reconcile_helius_client,
                                reconcile_rate_limiter,
                                reconcile_config,
                            ).await {
                                Ok(result) => {
                                    tracing::info!(
                                        total = result.total_helius_webhooks,
                                        eligible = result.eligible_wallets,
                                        ineligible = result.ineligible_wallets,
                                        deleted = result.deleted_webhooks,
                                        duration_ms = result.duration_ms,
                                        "Helius webhook reconciliation completed"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Helius webhook reconciliation task failed");
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    // Create operations state
    let operations_state = Arc::new(OperationsState {
        db: db_pool.clone(),
        engine: Some(Arc::new(_engine_handle.clone())),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
    });

    // Create auth state (reuse already-loaded api_keys_map and jwt_secret)
    let auth_state = Arc::new(AuthState::with_auth_config(
        api_keys_map.clone(),
        jwt_secret.clone(),
    ));
    tracing::info!(
        api_key_count = config.security.api_keys.len(),
        "Auth state initialized"
    );
    tracing::info!("WebSocket state initialized");

    // Build health routes with AppState
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state.clone());

    // Rate limiter for public API routes (more permissive — 60 req/s, burst 100)
    let public_api_limiter_conf = tower_governor::governor::GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(100)
        .key_extractor(middleware::ProxyAwareKeyExtractor)
        .finish()
        .ok_or_else(|| anyhow::anyhow!("Failed to build public API rate limiter"))?;
    let public_api_limiter_conf = std::sync::Arc::new(public_api_limiter_conf);
    let public_api_governor_layer = tower_governor::GovernorLayer {
        config: public_api_limiter_conf,
    };

    // Build public read-only API routes (no auth required for dashboard)
    let public_api_routes = Router::new()
        .route("/positions", get(list_positions))
        .route("/positions/{trade_uuid}", get(get_position))
        .route("/trades", get(list_trades))
        .route("/trades/export", get(export_trades))
        .route("/metrics/strategy", get(get_strategy_performance))
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/costs", get(get_cost_metrics))
        .route(
            "/metrics/trade-latency",
            get(chimera_operator::handlers::get_trade_latency),
        )
        .route(
            "/metrics/database-performance",
            get(chimera_operator::handlers::get_database_performance),
        )
        .route(
            "/metrics/request-rate",
            get(chimera_operator::handlers::get_request_rate),
        )
        .route(
            "/metrics/rpc-latency",
            get(chimera_operator::handlers::get_rpc_latency),
        )
        .route(
            "/risk/portfolio",
            get(chimera_operator::handlers::get_portfolio_risk),
        )
        .route(
            "/risk/stop-loss",
            get(chimera_operator::handlers::get_stop_loss_metrics),
        )
        .route(
            "/risk/profit-target",
            get(chimera_operator::handlers::get_profit_target_metrics),
        )
        .route(
            "/risk/position-size",
            get(chimera_operator::handlers::get_position_size_analysis),
        )
        .route("/incidents/dead-letter", get(list_dead_letter_queue))
        .route("/incidents/config-audit", get(list_config_audit))
        .route(
            "/signals/consensus",
            get(chimera_operator::handlers::get_consensus),
        )
        .route(
            "/signals/clustering",
            get(chimera_operator::handlers::get_wallet_clustering),
        )
        .route(
            "/signals/aggregation",
            get(chimera_operator::handlers::get_signal_aggregation),
        )
        .route(
            "/signals/quality",
            get(chimera_operator::handlers::get_signal_quality),
        )
        .route(
            "/signals/sources",
            get(chimera_operator::handlers::get_signal_sources),
        )
        .route("/market/regime", get(get_market_regime))
        .route("/market/conditions", get(get_market_conditions))
        // Scout intelligence endpoints
        .route("/scout/status", get(get_scout_status))
        .route("/scout/wqs-distribution", get(get_wqs_distribution))
        .route("/scout/metrics", get(get_scout_metrics))
        .route("/scout/run", post(trigger_scout_run))
        // Scout integration features
        .route("/scout/budget", get(get_budget_status))
        .route("/scout/cache", get(get_cache_stats))
        .route("/scout/conviction", get(get_conviction_allocation))
        .with_state(api_state.clone())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            2 * 1024 * 1024,
        ))
        .layer(public_api_governor_layer.clone());

    // Build operations API routes (use OperationsState)
    let operations_routes = Router::new()
        .route("/operations/resources", get(get_resources))
        .route("/operations/secrets", get(get_secrets))
        .route("/operations/rate-limit", get(get_rate_limit_status))
        .route("/operations/health-checks", get(get_health_check_details))
        .with_state(operations_state.clone());

    // Build protected API routes (auth required — includes reads that expose sensitive config)
    let protected_api_routes = Router::new()
        .route("/config", get(get_config))
        .route("/wallets", get(list_wallets))
        .route("/wallets/{address}", get(get_wallet).put(update_wallet))
        .route("/config", put(update_config))
        .route("/config/circuit-breaker/reset", post(reset_circuit_breaker))
        .route("/config/circuit-breaker/trip", post(trip_circuit_breaker))
        .route(
            "/metrics/reconciliation",
            post(update_reconciliation_metrics),
        )
        .route(
            "/metrics/secret-rotation",
            post(update_secret_rotation_metrics),
        )
        // Reconciliation API endpoints
        .route(
            "/reconciliation/status",
            get(chimera_operator::handlers::get_reconciliation_status),
        )
        .route(
            "/reconciliation/history",
            get(chimera_operator::handlers::get_reconciliation_history),
        )
        .route(
            "/reconciliation/stats",
            get(chimera_operator::handlers::get_reconciliation_stats),
        )
        .route(
            "/reconciliation/trigger",
            post(chimera_operator::handlers::trigger_reconciliation),
        )
        .route(
            "/reconciliation/discrepancies/:id/resolve",
            post(chimera_operator::handlers::resolve_discrepancy),
        )
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Create webhook state (token_parser already created above)

    // Refresh total_capital_sol from the live wallet balance every 60 seconds so that
    // compounding gains and drawdown recovery propagate into heat capacity without restart.
    {
        use chimera_operator::engine::transaction_builder::load_wallet_keypair;
        use solana_client::nonblocking::rpc_client::RpcClient as NonblockingRpcClient;
        use solana_sdk::signature::Signer;

        let heat_clone = Arc::clone(&portfolio_heat);
        let cb_clone = Arc::clone(&circuit_breaker);
        let rpc_url = config.rpc.primary_url.clone();
        match vault::load_secrets_with_fallback()
            .ok()
            .and_then(|s| load_wallet_keypair(&s).ok())
        {
            Some(keypair) => {
                let pubkey = keypair.pubkey();
                tokio::spawn(async move {
                    let rpc = NonblockingRpcClient::new(rpc_url);
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        match rpc.get_balance(&pubkey).await {
                            Ok(lamports) => {
                                let sol = rust_decimal::Decimal::from(lamports)
                                    / rust_decimal::Decimal::from(1_000_000_000u64);
                                heat_clone.update_capital(sol);
                                // Keep circuit breaker capital in sync so its portfolio-stop
                                // threshold reflects the live balance, not the startup value.
                                cb_clone.update_capital(sol);
                                tracing::debug!(capital_sol = ?sol, "Portfolio capital refreshed from wallet");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to fetch wallet balance for capital refresh");
                            }
                        }
                    }
                });
                tracing::info!("Portfolio capital refresh task spawned (60s interval)");
            }
            None => {
                tracing::warn!("Wallet keypair unavailable — portfolio capital will not auto-refresh from wallet balance");
            }
        }
    }

    // Force-liquidation safety task: if an external capital drain causes portfolio heat
    // to exceed 150% of the configured limit, exit oldest positions until back in range.
    // Runs every 60 seconds — slow enough to not interfere with normal trading, fast
    // enough to act before a margin-call-like cascade.
    {
        let fl_heat = Arc::clone(&portfolio_heat);
        let fl_db = db_pool.clone();
        let fl_engine = _engine_handle.clone();
        let fl_token = cancel_token.clone();
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = fl_token.cancelled() => break,
                    _ = interval.tick() => {
                        let overexposed = match fl_heat.is_critically_overexposed().await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(error = %e, "Heat overexposure check failed");
                                false
                            }
                        };
                        if !overexposed {
                            continue;
                        }
                        tracing::warn!("HEAT_OVEREXPOSED: capital drain detected — force-exiting oldest positions");
                        let positions = match fl_db.get_active_positions_with_entry().await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::error!(error = %e, "Force-liquidation: DB query failed");
                                continue;
                            }
                        };
                        let mut simulated_exposure = match fl_heat.calculate_heat().await {
                            Ok(h) => h.total_exposure_sol,
                            Err(_) => rust_decimal::Decimal::ZERO,
                        };
                        let threshold_sol = fl_heat.get_critical_threshold_sol();

                        for pos in positions {
                            if simulated_exposure <= threshold_sol {
                                break;
                            }
                            let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                            if let Err(e) = fl_engine.queue_signal(signal, None).await {
                                tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Force-liquidation signal failed — will retry next cycle");
                                continue;
                            }
                            tracing::warn!(
                                trade_uuid = %pos.trade_uuid,
                                token = %pos.token_address,
                                "Force-exited position (heat overexposure)"
                            );
                            let entry_size = pos.entry_amount_sol;
                            simulated_exposure -= entry_size;
                        }
                    }
                }
            }
        }));
        tracing::info!("Force-liquidation task spawned (60s interval, triggers at 150% heat)");
    }

    let position_sizer = Arc::new(PositionSizer::new(
        db_pool.clone(),
        Arc::new(config.position_sizing.clone()),
    ));
    tracing::info!(
        "Position sizer initialized (Kelly sizing: {})",
        config.position_sizing.use_kelly_sizing
    );

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        token_parser: token_parser.clone(),
        circuit_breaker: circuit_breaker.clone(),
        portfolio_heat: Some(portfolio_heat.clone()),
        signal_aggregator: Some(signal_aggregator.clone()),
        market_regime: Some(market_regime_detector.clone()),
        helius_client: helius_client.clone(),
        position_sizer: Some(position_sizer),
        total_capital_sol: config.position_sizing.total_capital_sol,
        max_position_sol: config.position_sizing.max_size_sol,
        shield_signal_quality_threshold: config.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config.strategy.spear_signal_quality_threshold,
        shield_percent: config.strategy.shield_percent,
        spear_percent: config.strategy.spear_percent,
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
    });

    // Create roster state
    let roster_path = config
        .database
        .path
        .parent()
        .map(|p| p.join("roster_new.db"))
        .unwrap_or_else(|| PathBuf::from("roster_new.db"));
    let roster_state = Arc::new(RosterState {
        db: db_pool.clone(),
        default_roster_path: roster_path,
    });

    // Build HMAC secrets for webhook verification
    let mut hmac_secrets = Vec::new();
    if !config.security.webhook_secret.is_empty() {
        hmac_secrets.push(config.security.webhook_secret.clone());
    }
    // Try to load from vault if available
    if let Ok(secrets) = vault::load_secrets_with_fallback() {
        if !secrets.webhook_secret.is_empty() {
            hmac_secrets.push(secrets.webhook_secret.clone());
        }
        if let Some(prev) = &secrets.webhook_secret_previous {
            if !prev.is_empty() {
                hmac_secrets.push(prev.clone());
            }
        }
    }
    // Add previous secret from config if available
    if let Some(prev) = &config.security.webhook_secret_previous {
        if !prev.is_empty() && !hmac_secrets.contains(prev) {
            hmac_secrets.push(prev.clone());
        }
    }
    let hmac_state = Arc::new(middleware::HmacState::with_rotation(
        hmac_secrets,
        config.security.max_timestamp_drift_secs,
    )?);

    // Build rate limiter for webhook routes
    let governor_conf = tower_governor::governor::GovernorConfigBuilder::default()
        .per_second(config.security.webhook_rate_limit as u64)
        .burst_size(config.security.webhook_burst_size)
        .key_extractor(middleware::ProxyAwareKeyExtractor)
        .finish()
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to build rate limiter — webhook_rate_limit must be > 0")
        })?;
    let governor_conf = std::sync::Arc::new(governor_conf);
    let governor_layer = tower_governor::GovernorLayer {
        config: governor_conf,
    };

    // Build webhook routes with rate limiting and HMAC middleware
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(webhook_state.clone())
        .layer(governor_layer.clone())
        .layer(axum_middleware::from_fn_with_state(
            hmac_state.clone(),
            middleware::hmac_verify,
        ));

    // Build roster routes
    // In devnet, allow roster merge without auth for easier testing
    let chimera_env = std::env::var("CHIMERA_ENV").unwrap_or_default();
    let is_devnet = chimera_env == "devnet"
        || config.database.path.to_string_lossy().contains("devnet")
        || config.trade_mode == TradeMode::Devnet;

    let roster_routes = if is_devnet {
        tracing::info!("Devnet mode: roster merge endpoint does not require authentication");
        Router::new()
            .route("/roster/merge", post(roster_merge))
            .route("/roster/validate", get(roster_validate))
            .with_state(roster_state.clone())
    } else {
        Router::new()
            .route("/roster/merge", post(roster_merge))
            .route("/roster/validate", get(roster_validate))
            .with_state(roster_state.clone())
            .layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                bearer_auth,
            ))
    };

    // Build auth routes
    // jwt_secret already defined above
    let sqlite_pool = match db_pool.pool() {
        db_abstraction::DbPool::SQLite(pool) => pool,
        _ => return Err(anyhow::anyhow!("SQLite pool required for wallet auth")),
    };
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .with_state(Arc::new(WalletAuthState {
            db: sqlite_pool,
            jwt_secret,
            // FIX 11: Initialize auth nonce store for replay protection
            seen_auth_nonces: std::sync::Arc::new(parking_lot::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }));

    // Build WebSocket routes — authentication handled within the handler via query parameter
    let ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());

    // Build metrics routes
    let metrics_routes = metrics_router().with_state(metrics_state.clone());

    // Rate limiting is enabled on webhook routes with ProxyAwareKeyExtractor

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build monitoring routes - will be created after engine is initialized
    // Use _engine_handle which is created earlier
    let config_arc = Arc::new(config.clone());
    tracing::info!("Attempting to create MonitoringState...");
    let monitoring_routes = match MonitoringState::new(
        db_pool.clone(),
        _engine_handle.clone(),
        config_arc.clone(),
        Some(token_fetcher.clone()),
    )
    .map(|ms| {
        ms.with_circuit_breaker(circuit_breaker.clone())
            .with_token_parser(token_parser.clone())
            .with_portfolio_heat(portfolio_heat.clone())
            .with_exit_detector(exit_detector.clone())
    }) {
        Ok(monitoring_state) => {
            let monitoring_state_arc = Arc::new(monitoring_state);
            tracing::info!(
                "Monitoring state initialized successfully, registering monitoring routes"
            );
            // Helius webhook and status are public (Helius calls from external service)
            let monitoring_public = Router::new()
                .route("/monitoring/status", get(get_monitoring_status))
                .route("/monitoring/helius-webhook", post(helius_webhook_handler))
                .with_state(monitoring_state_arc.clone());
            // Enable/disable wallet monitoring require operator role
            let monitoring_protected = Router::new()
                .route(
                    "/monitoring/wallets/{wallet_address}/enable",
                    post(enable_wallet_monitoring),
                )
                .route(
                    "/monitoring/wallets/{wallet_address}/disable",
                    post(disable_wallet_monitoring),
                )
                // Webhook lifecycle management routes (require operator role)
                .route("/monitoring/webhooks/stats", get(get_webhook_stats))
                .route(
                    "/monitoring/webhooks/bulk-register",
                    post(bulk_register_webhooks),
                )
                .route(
                    "/monitoring/webhooks/bulk-cleanup",
                    post(bulk_cleanup_webhooks),
                )
                .route(
                    "/monitoring/webhooks/reconcile",
                    post(manual_reconcile_webhooks),
                )
                .route(
                    "/monitoring/webhooks/health-check",
                    post(manual_health_check),
                )
                .route("/monitoring/webhooks/audit", get(get_webhook_audit_log))
                .route(
                    "/monitoring/webhooks/:wallet_address/retry",
                    post(retry_webhook_registration),
                )
                .route(
                    "/monitoring/webhooks/:wallet_address/toggle",
                    post(toggle_wallet_webhook),
                )
                // Wallet monitoring state (requires readonly+)
                .route(
                    "/monitoring/wallets/states",
                    get(get_wallet_monitoring_states),
                )
                .with_state(monitoring_state_arc)
                .layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    bearer_auth,
                ));
            monitoring_public.merge(monitoring_protected)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize MonitoringState, monitoring routes disabled");
            Router::new()
        }
    };

    // Root-level WebSocket for web dashboard — authentication handled within handler via query parameter
    let root_ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());

    // Create full router with all routes and middleware
    // Note: Layer order matters - bottom layers are applied first (innermost)
    let app = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route("/health", get(health_simple))
        .merge(root_ws_routes)
        .nest("/api/v1", health_routes)
        .nest("/api/v1", public_api_routes)
        .nest("/api/v1", protected_api_routes)
        .nest("/api/v1", operations_routes)
        .nest("/api/v1", webhook_routes)
        .nest("/api/v1", roster_routes)
        .nest("/api/v1", auth_routes)
        .nest("/api/v1", ws_routes)
        .nest("/api/v1", monitoring_routes)
        .merge(metrics_routes)
        .layer(cors)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            10 * 1024 * 1024,
        ))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                )
            }),
        );
    // Rate limiting is applied per-route (webhook routes have governor_layer)

    tracing::info!("Full router created with all routes and middleware");

    // Start server
    let addr: SocketAddr = match format!("{}:{}", config.server.host, config.server.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            tracing::error!(error = %e, host = %config.server.host, port = %config.server.port, "Invalid server address");
            return Err(anyhow::anyhow!(
                "Invalid server address {}:{} - check config: {}",
                config.server.host,
                config.server.port,
                e
            ));
        }
    };

    tracing::info!(%addr, "Starting server with FULL router");

    let shutdown_token = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, %addr, "Failed to bind server port — is it already in use?");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
            })
            .await
        {
            tracing::error!(error = %e, "Server exited with error");
        }
    });

    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("Shutdown signal received"),
        Err(err) => tracing::error!("Unable to listen for shutdown signal: {}", err),
    }

    cancel_token.cancel();
    if let Err(e) = server_handle.await {
        tracing::error!(error = %e, "Server task panicked during shutdown");
    }

    // Wait for remaining background tasks with a timeout
    for handle in task_handles {
        if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            tracing::warn!(error = %e, "Background task did not complete within 5s shutdown window");
        }
    }

    tracing::info!("Chimera Operator shut down successfully");

    Ok(())
}

/// Build an EXIT signal from an active position entry (for stop-loss / profit-target exits)
fn build_exit_signal(pos: &ActivePositionEntry, fraction: rust_decimal::Decimal) -> Signal {
    use rust_decimal::prelude::*;
    let base_amount = if pos.entry_amount_sol.is_zero() {
        rust_decimal::Decimal::from_str("0.01").unwrap_or(rust_decimal::Decimal::ONE)
    } else {
        pos.entry_amount_sol
    };
    let amount = (base_amount * fraction)
        .max(rust_decimal::Decimal::from_str("0.001").unwrap_or(rust_decimal::Decimal::ZERO));
    let payload = SignalPayload {
        strategy: Strategy::Exit,
        token: pos.token_symbol.clone(),
        token_address: Some(pos.token_address.clone()),
        action: Action::Sell,
        amount_sol: amount,
        wallet_address: pos.wallet_address.clone(),
        trade_uuid: Some(pos.trade_uuid.clone()),
        exit_fraction: Some(fraction),
    };
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
}

/// Build an exit signal for an absolute SOL amount.
/// Unlike `build_exit_signal` (which takes a fraction of the original position),
/// this takes an explicit SOL amount — eliminating the oversell bug where
/// the prior `ExitPercent` was applied against the original entry instead of the remaining balance.
/// The `exit_fraction` is computed as amount_sol / entry_amount_sol so the engine's
/// `close_position` (which multiplies exit_fraction by entry_amount) produces the correct amount.
fn build_exit_signal_amount(
    pos: &ActivePositionEntry,
    amount_sol: rust_decimal::Decimal,
) -> Signal {
    use rust_decimal::prelude::*;
    let amount = amount_sol
        .max(rust_decimal::Decimal::from_str("0.001").unwrap_or(rust_decimal::Decimal::ZERO));
    let base = if pos.entry_amount_sol.is_zero() {
        rust_decimal::Decimal::from_str("0.01").unwrap_or(rust_decimal::Decimal::ONE)
    } else {
        pos.entry_amount_sol
    };
    let fraction = if !base.is_zero() {
        (amount / base).min(Decimal::ONE)
    } else {
        Decimal::ONE
    };
    let payload = SignalPayload {
        strategy: Strategy::Exit,
        token: pos.token_symbol.clone(),
        token_address: Some(pos.token_address.clone()),
        action: Action::Sell,
        amount_sol: amount,
        wallet_address: pos.wallet_address.clone(),
        trade_uuid: Some(pos.trade_uuid.clone()),
        exit_fraction: Some(fraction),
    };
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
}

/// Initialize tracing/logging
fn init_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chimera_operator=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}

/// Load and validate configuration
fn load_config() -> anyhow::Result<AppConfig> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Hard-fail if dev mode is active in a production environment. CHIMERA_ENV=production
    // must not coexist with CHIMERA_DEV_MODE — the latter skips token safety and config
    // validation, creating a silent security bypass that is hard to detect post-deploy.
    if std::env::var("CHIMERA_DEV_MODE").is_ok()
        && std::env::var("CHIMERA_ENV").as_deref() == Ok("production")
    {
        return Err(anyhow::anyhow!(
            "CHIMERA_DEV_MODE is set in a production environment (CHIMERA_ENV=production). \
             Unset CHIMERA_DEV_MODE before deploying to production."
        ));
    }

    let config = AppConfig::load_config().map_err(|e| {
        tracing::error!(error = %e, "Failed to load configuration");
        anyhow::anyhow!("Configuration error: {}", e)
    })?;

    // Validate configuration
    if let Err(e) = config.validate() {
        // In development, allow missing webhook secret
        if std::env::var("CHIMERA_DEV_MODE").is_ok() {
            tracing::warn!("Running in dev mode - skipping configuration validation");
        } else {
            return Err(anyhow::anyhow!("Configuration validation failed: {}", e));
        }
    }

    Ok(config)
}

/// Generate daily trading summary from database
async fn generate_daily_summary(
    db: &dyn db_abstraction::Database,
) -> anyhow::Result<(rust_decimal::Decimal, u32, f64)> {
    // Get yesterday's date range
    let now = Utc::now();
    let yesterday_start = (now - chrono::Duration::days(1))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();
    let yesterday_end = (now - chrono::Duration::days(1))
        .format("%Y-%m-%dT23:59:59Z")
        .to_string();

    // Query trades from yesterday
    let trades = db
        .get_trades_filtered(
            Some(&yesterday_start),
            Some(&yesterday_end),
            Some("CLOSED"),
            None,
            None, // No wallet_address filter for daily summary
            1000,
            0,
        )
        .await?;

    if trades.is_empty() {
        return Ok((rust_decimal::Decimal::ZERO, 0, 0.0));
    }

    let trade_count = trades.len() as u32;
    let mut total_pnl_usd = rust_decimal::Decimal::ZERO;
    let mut winning_trades = 0u32;

    for trade in &trades {
        if let Some(pnl_usd) = trade.pnl_usd {
            total_pnl_usd += pnl_usd;
            if pnl_usd > rust_decimal::Decimal::ZERO {
                winning_trades += 1;
            }
        }
    }

    let win_rate = if trade_count > 0 {
        (winning_trades as f64 / trade_count as f64) * 100.0
    } else {
        0.0
    };

    Ok((total_pnl_usd, trade_count, win_rate))
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_version() {
        // Ensure version is set
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
