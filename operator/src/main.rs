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
use chimera_operator::db;
use chimera_operator::db::ActivePositionEntry;
use chimera_operator::engine::{
    self, MarketRegimeDetector, MomentumExit, PortfolioHeat, PositionSizer, ProfitTargetAction,
    ProfitTargetManager, RecoveryManager, StopLossAction, StopLossManager, TipManager, VolumeCache,
};
use chimera_operator::handlers::{
    disable_wallet_monitoring, enable_wallet_monitoring, export_trades, get_config,
    get_cost_metrics, get_monitoring_status, get_performance_metrics, get_position,
    get_strategy_performance, get_wallet, health_check, health_simple, helius_webhook_handler,
    list_config_audit, list_dead_letter_queue, list_positions, list_trades, list_wallets,
    reset_circuit_breaker, roster_merge, roster_validate, trip_circuit_breaker, update_config,
    update_reconciliation_metrics, update_secret_rotation_metrics, update_wallet, wallet_auth,
    webhook_handler, ws_handler, ApiState, AppState, RosterState, WalletAuthState, WebhookState,
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    init_tracing();

    // Load configuration
    let mut config = load_config()?;

    // Explicitly override Jupiter simulation mode from environment if set
    // This ensures the env var takes precedence over YAML/config defaults
    if let Ok(sim_mode) = std::env::var("CHIMERA_JUPITER__DEVNET_SIMULATION_MODE") {
        let sim_mode_bool = sim_mode.to_lowercase() == "true" || sim_mode == "1";
        if sim_mode_bool != config.jupiter.devnet_simulation_mode {
            tracing::info!(
                old_value = config.jupiter.devnet_simulation_mode,
                new_value = sim_mode_bool,
                "Overriding Jupiter simulation mode from environment variable"
            );
            config.jupiter.devnet_simulation_mode = sim_mode_bool;
        }
    }

    tracing::info!(host = %config.server.host, port = config.server.port, "Configuration loaded");
    tracing::info!(
        jupiter_simulation_mode = config.jupiter.devnet_simulation_mode,
        jupiter_api_url = %config.jupiter.api_url,
        "Jupiter configuration loaded"
    );

    // Validate vault/secrets early — fail loudly before any DB or network I/O.
    // load_secrets_with_fallback() returns Err only when CHIMERA_VAULT_KEY is set but invalid
    // or the vault file exists but cannot be decrypted; if vault is not configured it returns Ok
    // with env-var secrets.
    let _startup_secrets = vault::load_secrets_with_fallback()
        .map_err(|e| anyhow::anyhow!("Vault startup validation failed: {}", e))?;
    tracing::info!("Vault/secrets validated at startup");

    // Initialize database
    let db_pool = db::init_pool(&config.database).await?;
    db::run_migrations(&db_pool).await?;
    db::startup_integrity_check(&db_pool).await?;
    db::recover_executing_trades(&db_pool).await?;
    tracing::info!("Database initialized");

    // Create WebSocket state
    let ws_state = Arc::new(WsState::new());
    let cancel_token = CancellationToken::new();

    // Initialize price cache
    let price_cache = Arc::new(PriceCache::new());
    // Track SOL for volatility calculation
    price_cache.track_token("So11111111111111111111111111111111111111112");

    // Initialize circuit breaker
    let circuit_breaker = Arc::new(CircuitBreaker::new_with_ws(
        config.circuit_breakers.clone(),
        db_pool.clone(),
        Some(ws_state.clone()),
    )
    .with_total_capital(config.position_sizing.total_capital_sol)
    .with_price_cache(price_cache.clone()));

    // FIX [R-C1]: Restore persisted circuit breaker state from DB before accepting connections.
    // This ensures that a trip persisted before last restart is re-applied and evaluate()
    // runs so cooldown expiry / breach re-evaluation happen immediately on startup.
    if let Err(e) = circuit_breaker.restore_from_db().await {
        tracing::error!(error = %e, "Failed to restore circuit breaker state from DB — starting Active");
    }

    // Restore kill-switch if it was active before last restart.
    // Reads from kill_switch_state (single-row UPSERT table) which is written synchronously
    // by the kill-switch API handler before tripping the circuit breaker in memory.
    // Falls back to config_audit if kill_switch_state row is absent (pre-migration DBs).
    {
        let is_active = if db::is_kill_switch_active(&db_pool).await {
            true
        } else {
            // Fallback: config_audit (legacy path for DBs without kill_switch_state row)
            let row: Option<String> = sqlx::query_scalar(
                "SELECT new_value FROM config_audit WHERE key = 'kill_switch' ORDER BY changed_at DESC LIMIT 1",
            )
            .fetch_optional(&db_pool)
            .await
            .unwrap_or(None);
            row.as_deref() == Some("ACTIVE")
        };

        if is_active {
            tracing::warn!("Kill-switch was active before restart — re-tripping circuit breaker");
            let _ = circuit_breaker
                .manual_trip("SYSTEM_RESTART_RESTORE", "Kill-switch was active before restart".to_string())
                .await;
        }
    }

    // Initialize tip manager
    let tip_manager = Arc::new(TipManager::new(config.jito.clone(), db_pool.clone()));
    let _ = tip_manager.init().await;

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

        Arc::new(composite)
    };

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
        );
    tracing::info!("Engine created");

    // Spawn engine
    tokio::spawn(async move {
        engine.run().await;
    });
    tracing::info!("Engine task spawned");

    // Shared RPC client for the recovery manager — reuses the same connection as
    // the executor so that any failover logic applied to that client is also
    // available to recovery operations instead of having a separate single-point
    // connection with no fallback.
    let shared_rpc_client = Arc::new(
        solana_client::nonblocking::rpc_client::RpcClient::new(config.rpc.primary_url.clone()),
    );

    // Spawn recovery manager
    let recovery_manager = Arc::new(RecoveryManager::new_with_rpc(
        db_pool.clone(),
        shared_rpc_client.clone(),
        Some(ws_state.clone()),
    ));
    let recovery_clone = recovery_manager.clone();
    tokio::spawn(async move {
        recovery_clone.start_background_task().await;
    });
    tracing::info!("Recovery manager task spawned");

    // Periodic EXECUTING cleanup: catch trades that get stuck in EXECUTING due to a
    // crash or panic mid-flight. The startup sweep only covers the previous run; this
    // covers long-running operators that never restart.
    {
        let exec_cleanup_db = db_pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match db::recover_executing_trades(&exec_cleanup_db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(count = n, "Periodic sweep: recovered stuck EXECUTING trades to FAILED"),
                    Err(e) => tracing::error!(error = %e, "Periodic EXECUTING cleanup failed"),
                }
            }
        });
    }

    // Spawn PnL refresh task — updates unrealized_pnl_percent every 30 seconds for active positions
    {
        let pnl_db = db_pool.clone();
        let pnl_pc = price_cache.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                match db::get_active_position_tokens(&pnl_db).await {
                    Ok(positions) => {
                        for pos in positions {
                            if let Some(current) = pnl_pc.get_price_usd(&pos.token_address) {
                                let entry = if pos.entry_price.is_zero() {
                                    current
                                } else {
                                    pos.entry_price
                                };
                                let pnl_sol = (current - entry) * pos.entry_amount_sol;
                                let pnl_pct = if !entry.is_zero() {
                                    (current - entry) / entry * rust_decimal::Decimal::from(100)
                                } else {
                                    rust_decimal::Decimal::ZERO
                                };
                                if let Err(e) = db::update_position_unrealized_pnl(
                                    &pnl_db,
                                    &pos.trade_uuid,
                                    current,
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
                    let was_active = circuit_breaker_clone.is_trading_allowed();

                    if let Err(e) = circuit_breaker_clone.evaluate().await {
                        tracing::error!(error = %e, "Circuit breaker evaluation failed");
                    }

                    // Check if circuit breaker just tripped
                    let is_active = circuit_breaker_clone.is_trading_allowed();
                    if was_active && !is_active && !was_tripped {
                        was_tripped = true;
                        if notify_rules.circuit_breaker_triggered {
                            let reason = circuit_breaker_clone
                                .trip_reason()
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "Unknown reason".to_string());

                            notifier_cb
                                .notify(NotificationEvent::CircuitBreakerTriggered { reason })
                                .await;
                        }
                    } else if is_active {
                        was_tripped = false;
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
                    // Fetch retryable items from DLQ
                    match sqlx::query_as::<_, (String, String, i64)>(
                        "SELECT trade_uuid, payload, retry_count FROM dead_letter_queue WHERE can_retry = 1 AND processed_at IS NULL LIMIT 50"
                    )
                    .fetch_all(&dlq_pool)
                    .await {
                        Ok(items) => {
                            const MAX_DLQ_RETRIES: i64 = 3;
                            tracing::info!(count = items.len(), "Processing DLQ retry items");
                            for (trade_uuid, payload_str, retry_count) in items {
                                // Increment retry_count and enforce max-retry limit
                                let new_count = retry_count + 1;
                                let can_still_retry = if new_count >= MAX_DLQ_RETRIES { 0i64 } else { 1i64 };
                                if let Err(e) = sqlx::query(
                                    "UPDATE dead_letter_queue SET retry_count = ?, can_retry = ? WHERE trade_uuid = ? AND processed_at IS NULL"
                                )
                                .bind(new_count)
                                .bind(can_still_retry)
                                .bind(&trade_uuid)
                                .execute(&dlq_pool)
                                .await {
                                    tracing::warn!(error = %e, uuid = %trade_uuid, "Failed to increment DLQ retry_count");
                                    continue;
                                }

                                if can_still_retry == 0 {
                                    tracing::warn!(
                                        uuid = %trade_uuid,
                                        retry_count = new_count,
                                        "DLQ item permanently failed after max retries"
                                    );
                                    continue;
                                }

                                // Parse payload and re-queue
                                match serde_json::from_str::<serde_json::Value>(&payload_str) {
                                    Ok(_) => {
                                        // Mark as processed and update trade status to RETRY
                                        if let Err(e) = sqlx::query(
                                            "UPDATE dead_letter_queue SET processed_at = CURRENT_TIMESTAMP WHERE trade_uuid = ?"
                                        )
                                        .bind(&trade_uuid)
                                        .execute(&dlq_pool)
                                        .await {
                                            tracing::warn!(error = %e, "Failed to mark DLQ item as processed");
                                        } else if let Err(e) = crate::db::update_trade_status(&dlq_pool, &trade_uuid, "RETRY", None, None).await {
                                            tracing::warn!(error = %e, uuid = %trade_uuid, "Failed to update trade status to RETRY");
                                        } else {
                                            tracing::info!(uuid = %trade_uuid, retry_count = new_count, "DLQ item re-queued to RETRY");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, uuid = %trade_uuid, "Failed to parse DLQ payload");
                                    }
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

            match generate_daily_summary(&db_pool_daily).await {
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
                    match db::get_expired_ttl_wallets(&db_pool_ttl).await {
                        Ok(expired_wallets) => {
                            for address in expired_wallets {
                                tracing::info!(wallet = %address, "Demoting wallet due to TTL expiration");
                                if let Err(e) = db::demote_wallet(
                                    &db_pool_ttl,
                                    &address,
                                    "Auto-demoted: TTL expired",
                                )
                                .await
                                {
                                    tracing::error!(wallet = %address, error = %e, "Failed to demote wallet");
                                } else {
                                    let _ = db::log_config_change(
                                        &db_pool_ttl,
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
        stop_loss_mgr.set_signal_aggregator(signal_aggregator.clone()).await;
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
        let monitor_total_capital = config.position_sizing.total_capital_sol;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut last_checked: std::collections::HashMap<String, std::time::Instant> = std::collections::HashMap::new();
            let mut db_fail_count: u32 = 0;
            loop {
                tokio::select! {
                    _ = monitor_token.cancelled() => {
                        tracing::info!("Shutting down position monitoring task");
                        break;
                    }
                    _ = interval.tick() => {
                        // Check portfolio-level stop once per cycle before individual positions
                        match monitor_sl.check_portfolio_stop(monitor_total_capital).await {
                            StopLossAction::PauseAll => {
                                tracing::warn!("Portfolio stop triggered — skipping position checks this cycle");
                                continue;
                            }
                            _ => {}
                        }

                        let positions = match db::get_active_positions_with_entry(&monitor_db).await {
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
                                let _ = monitor_engine.queue_signal(signal, None).await;
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
                                    let _ = monitor_engine.queue_signal(signal, None).await;
                                    monitor_pt.remove_position(&pos.trade_uuid).await;
                                }
                                ProfitTargetAction::ExitPercent(pct) => {
                                    // pct is 0-100; convert to 0.0-1.0 fraction
                                    let fraction = (pct / rust_decimal::Decimal::from(100))
                                        .max(rust_decimal::Decimal::ZERO)
                                        .min(rust_decimal::Decimal::ONE);
                                    tracing::info!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        fraction = %fraction,
                                        "Partial profit target reached, queuing partial EXIT signal"
                                    );
                                    let signal = build_exit_signal(&pos, fraction);
                                    let _ = monitor_engine.queue_signal(signal, None).await;
                                    // Don't remove from tracker — position remains open for remaining amount
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
    let metrics_state = Arc::new(MetricsState::new());

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
                    // Update circuit breaker state
                    let cb_state = circuit_breaker_clone.current_state();
                    let is_active = cb_state == chimera_operator::circuit_breaker::CircuitBreakerState::Active;
                    metrics_state_clone
                        .circuit_breaker_state
                        .set(if is_active { 1 } else { 0 });

                    // Update RPC health
                    if let Some(rpc_health) = engine_handle_metrics.get_rpc_health().await {
                        metrics_state_clone
                            .rpc_health
                            .set(if rpc_health.healthy { 1 } else { 0 });
                    }

                    // Update active positions count
                    if let Ok(count) = db::count_active_positions(&db_pool_metrics).await {
                        metrics_state_clone.active_positions.set(count);
                    }

                    // Update total trades count
                    if let Ok(count) = db::count_total_trades(&db_pool_metrics).await {
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

        tokio::spawn(async move {
            chimera_operator::monitoring::start_polling_task(
                polling_db,
                polling_engine,
                polling_config,
                polling_token,
                polling_cb,
                polling_tp,
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
    });

    // Create API state
    let api_state = Arc::new(ApiState {
        db: db_pool.clone(),
        circuit_breaker: circuit_breaker.clone(),
        config: Arc::new(tokio::sync::RwLock::new(config.clone())),
        notifier: notifier.clone(),
        engine: Some(Arc::new(_engine_handle.clone())),
        metrics: metrics_state.clone(),
    });

    // Create auth state
    // Load API keys and admin wallets from config
    let mut api_keys_map = std::collections::HashMap::new();
    for key_config in &config.security.api_keys {
        if let Ok(role) = key_config.role.parse::<Role>() {
            api_keys_map.insert(key_config.key.clone(), role);
            tracing::debug!(key_prefix = %&key_config.key[..key_config.key.len().min(8)], role = %role, "API key configured");
        } else {
            tracing::warn!(key_prefix = %&key_config.key[..key_config.key.len().min(8)], role = %key_config.role, "Invalid role in API key config");
        }
    }

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());

    let auth_state = Arc::new(AuthState::with_auth_config(
        api_keys_map,
        jwt_secret.clone(),
    ));
    tracing::info!(
        api_key_count = config.security.api_keys.len(),
        "Auth state initialized"
    );

    // Build health routes with AppState
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state.clone());

    // Build public read-only API routes (no auth required for dashboard)
    let public_api_routes = Router::new()
        .route("/positions", get(list_positions))
        .route("/positions/{trade_uuid}", get(get_position))
        .route("/trades", get(list_trades))
        .route("/trades/export", get(export_trades))
        .route("/metrics/strategy", get(get_strategy_performance))
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/costs", get(get_cost_metrics))
        .route("/incidents/dead-letter", get(list_dead_letter_queue))
        .route("/incidents/config-audit", get(list_config_audit))
        .with_state(api_state.clone());

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
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Create webhook state (token_parser already created above)

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

    let portfolio_heat = Arc::new(PortfolioHeat::new(
        db_pool.clone(),
        config.position_sizing.total_capital_sol,
    ));
    tracing::info!(
        total_capital_sol = ?config.position_sizing.total_capital_sol,
        "Portfolio heat manager initialized"
    );

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
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(60));
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
        tokio::spawn(async move {
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
                        let mut positions = match db::get_active_positions_with_entry(&fl_db).await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::error!(error = %e, "Force-liquidation: DB query failed");
                                continue;
                            }
                        };
                        positions.sort_by_key(|p| p.entry_time);
                        for pos in positions {
                            let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                            let _ = fl_engine.queue_signal(signal, None).await;
                            tracing::warn!(
                                trade_uuid = %pos.trade_uuid,
                                token = %pos.token_address,
                                "Force-exited position (heat overexposure)"
                            );
                            match fl_heat.is_critically_overexposed().await {
                                Ok(false) => break,
                                _ => {}
                            }
                        }
                    }
                }
            }
        });
        tracing::info!("Force-liquidation task spawned (60s interval, triggers at 150% heat)");
    }

    let position_sizer = Arc::new(PositionSizer::new(
        db_pool.clone(),
        Arc::new(config.position_sizing.clone()),
    ));
    tracing::info!("Position sizer initialized (Kelly sizing: {})", config.position_sizing.use_kelly_sizing);

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        token_parser,
        circuit_breaker: circuit_breaker.clone(),
        portfolio_heat: Some(portfolio_heat),
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
    ));

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
    let is_devnet =
        chimera_env == "devnet" || config.database.path.to_string_lossy().contains("devnet");

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
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .with_state(Arc::new(WalletAuthState {
            db: db_pool.clone(),
            jwt_secret,
        }));

    // Build WebSocket routes — require bearer auth to prevent unauthenticated position data leaks
    let ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

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
    ) {
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

    // Root-level WebSocket for web dashboard — bearer auth required to prevent data leaks
    let root_ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Create full router with all routes and middleware
    // Note: Layer order matters - bottom layers are applied first (innermost)
    let app = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route("/health", get(health_simple))
        .merge(root_ws_routes)
        .nest("/api/v1", health_routes)
        .nest("/api/v1", public_api_routes)
        .nest("/api/v1", protected_api_routes)
        .nest("/api/v1", webhook_routes)
        .nest("/api/v1", roster_routes)
        .nest("/api/v1", auth_routes)
        .nest("/api/v1", ws_routes)
        .nest("/api/v1", monitoring_routes)
        .merge(metrics_routes)
        .layer(cors)
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
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .expect("Invalid server address");

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
    let _ = server_handle.await;
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
    let amount = (base_amount * fraction).max(
        rust_decimal::Decimal::from_str("0.001").unwrap_or(rust_decimal::Decimal::ZERO),
    );
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

/// Build list of HMAC secrets from vault and config
#[allow(dead_code)]
fn build_hmac_secrets(secrets: &vault::VaultSecrets, config: &AppConfig) -> Vec<String> {
    let mut hmac_secrets = Vec::new();

    // Primary secret from vault takes precedence
    if !secrets.webhook_secret.is_empty() {
        hmac_secrets.push(secrets.webhook_secret.clone());
    } else if !config.security.webhook_secret.is_empty() {
        hmac_secrets.push(config.security.webhook_secret.clone());
    }

    // Previous secret for rotation
    if let Some(ref prev) = secrets.webhook_secret_previous {
        if !prev.is_empty() {
            hmac_secrets.push(prev.clone());
        }
    } else if let Some(ref prev) = config.security.webhook_secret_previous {
        if !prev.is_empty() {
            hmac_secrets.push(prev.clone());
        }
    }

    hmac_secrets
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

    let config = AppConfig::load().map_err(|e| {
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
    db: &db::DbPool,
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
    let trades = db::get_trades(
        db,
        Some(&yesterday_start),
        Some(&yesterday_end),
        Some("CLOSED"),
        None,
        None, // No wallet_address filter for daily summary
        None,
        None,
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
