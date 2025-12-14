//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::{self as axum_middleware, Next},
    routing::{get, post, put},
    Router,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;


use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::AppConfig;
use chimera_operator::db;
use chimera_operator::engine::{self, RecoveryManager, TipManager};
use chimera_operator::handlers::{
    export_trades, get_config, get_cost_metrics, get_performance_metrics, get_position, get_strategy_performance,
    get_wallet, health_check, health_simple, list_config_audit, list_dead_letter_queue,
    list_positions, list_trades, list_wallets, reset_circuit_breaker, trip_circuit_breaker, roster_merge,
    roster_validate, update_config, update_wallet, update_reconciliation_metrics,
    update_secret_rotation_metrics, wallet_auth, webhook_handler, ws_handler,
    get_monitoring_status, enable_wallet_monitoring, disable_wallet_monitoring, helius_webhook_handler,
    ApiState, AppState, RosterState, WalletAuthState, WebhookState, WsState,
};
use chimera_operator::middleware::{self, bearer_auth, AuthState, HmacState, Role};
use chimera_operator::metrics::{MetricsState, metrics_router};
use chimera_operator::notifications::{self, CompositeNotifier, DiscordNotifier, NotificationEvent, TelegramNotifier};
use chimera_operator::price_cache::PriceCache;
use chimera_operator::roster;
use chimera_operator::monitoring::{HeliusClient, SignalAggregator, MonitoringState};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::vault;


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
    
    // Initialize database
    let db_pool = db::init_pool(&config.database).await?;
    db::run_migrations(&db_pool).await?;
    tracing::info!("Database initialized");
    
    // Create WebSocket state
    let ws_state = Arc::new(WsState::new());
    let cancel_token = CancellationToken::new();
    
    // Initialize circuit breaker
    let circuit_breaker = Arc::new(CircuitBreaker::new_with_ws(
        config.circuit_breakers.clone(),
        db_pool.clone(),
        Some(ws_state.clone()),
    ));
    
    // Initialize price cache
    let price_cache = Arc::new(PriceCache::new());
    // Track SOL for volatility calculation
    price_cache.track_token("So11111111111111111111111111111111111111112");
    
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
                composite.add_service(Arc::new(notifications::TelegramNotifier::new(telegram_config)));
                tracing::info!("Telegram notifications enabled");
            } else {
                tracing::warn!("Telegram notifications enabled in config but bot_token/chat_id not set");
            }
        }

        Arc::new(composite)
    };
    
    // Initialize token parser (needed for slow-path safety checks in engine)
    let token_cache = Arc::new(TokenCache::new(
        config.token_safety.cache_capacity,
        config.token_safety.cache_ttl_seconds,
    ));
    let token_fetcher = Arc::new(TokenMetadataFetcher::new(&config.rpc.primary_url));
    let token_safety_config = TokenSafetyConfig {
        freeze_authority_whitelist: config.token_safety.freeze_authority_whitelist.iter().cloned().collect(),
        mint_authority_whitelist: config.token_safety.mint_authority_whitelist.iter().cloned().collect(),
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        honeypot_detection_enabled: config.token_safety.honeypot_detection_enabled,
    };
    let token_parser = Arc::new(TokenParser::new(token_safety_config, token_cache.clone(), token_fetcher.clone()));
    tracing::info!("Token parser initialized");
    
    // Create engine
    let (engine, _engine_handle) = engine::Engine::new_with_extras_tip_manager_price_cache_and_token_parser(
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
    
    // Spawn recovery manager
    let recovery_manager = Arc::new(RecoveryManager::new_with_ws(
        db_pool.clone(),
        config.rpc.primary_url.clone(),
        Some(ws_state.clone()),
    ));
    let recovery_clone = recovery_manager.clone();
    tokio::spawn(async move {
        recovery_clone.start_background_task().await;
    });
    tracing::info!("Recovery manager task spawned");
    
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
    
    // Spawn price cache updater
    let price_cache_clone = price_cache.clone();
    tokio::spawn(async move {
        price_cache_clone.start_updater().await;
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
                .unwrap()
                .and_utc();

            if next_run <= now {
                next_run = next_run + chrono::Duration::days(1);
            }

            let sleep_duration = (next_run - now).to_std().unwrap_or(std::time::Duration::from_secs(3600));
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
                        metrics_state_clone.active_positions.set(count as i64);
                    }

                    // Update total trades count
                    if let Ok(count) = db::count_total_trades(&db_pool_metrics).await {
                        metrics_state_clone.total_trades.set(count as i64);
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
    
    let mut admin_wallets_map = std::collections::HashMap::new();
    for wallet_config in &config.security.admin_wallets {
        if let Ok(role) = wallet_config.role.parse::<Role>() {
            admin_wallets_map.insert(wallet_config.address.clone(), role);
            tracing::debug!(wallet = %wallet_config.address, role = %role, "Admin wallet configured");
        } else {
            tracing::warn!(wallet = %wallet_config.address, role = %wallet_config.role, "Invalid role in admin wallet config");
        }
    }
    
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let auth_state = Arc::new(AuthState::with_auth_config(
        api_keys_map, 
        admin_wallets_map,
        jwt_secret.clone()
    ));
    tracing::warn!(
        api_key_count = config.security.api_keys.len(),
        admin_wallet_count = config.security.admin_wallets.len(),
        admin_wallets = ?config.security.admin_wallets.iter().map(|w| &w.address).collect::<Vec<_>>(),
        "Auth state initialized with admin wallets"
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
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/costs", get(get_cost_metrics))
        .route("/metrics/strategy/{strategy}", get(get_strategy_performance))
        .route("/incidents/dead-letter", get(list_dead_letter_queue))
        .route("/incidents/config-audit", get(list_config_audit))
        .route("/config", get(get_config))
        .route("/wallets", get(list_wallets))
        .with_state(api_state.clone());
    
    // Build protected API routes (auth required for writes)
    let protected_api_routes = Router::new()
        .route("/wallets/{address}", get(get_wallet).put(update_wallet))
        .route("/config", put(update_config))
        .route("/config/circuit-breaker/reset", post(reset_circuit_breaker))
        .route("/config/circuit-breaker/trip", post(trip_circuit_breaker))
        .route("/metrics/reconciliation", post(update_reconciliation_metrics))
        .route("/metrics/secret-rotation", post(update_secret_rotation_metrics))
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));
    
    // Create webhook state (token_parser already created above)
    
    // Create SignalAggregator and HeliusClient for signal quality enhancements
    let signal_aggregator = Arc::new(SignalAggregator::new(db_pool.clone()));
    let helius_client = Arc::new(
        HeliusClient::new(
            config.monitoring.as_ref()
                .and_then(|m| m.helius_api_key.clone())
                .unwrap_or_default(),
        )
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to create HeliusClient, signal quality will be limited");
            // Create a dummy client with empty API key (will fail gracefully)
            HeliusClient::new(String::new()).unwrap()
        })
    );
    
    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        token_parser,
        circuit_breaker: circuit_breaker.clone(),
        portfolio_heat: None, // Optional - can be enabled later
        signal_aggregator: Some(signal_aggregator.clone()),
        helius_client: Some(helius_client.clone()),
    });
    
    // Create roster state
    let roster_path = config.database.path.parent()
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
        .unwrap();
    let governor_conf = std::sync::Arc::new(governor_conf);
    let governor_layer = tower_governor::GovernorLayer { config: governor_conf };
    
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
    let is_devnet = chimera_env == "devnet" || config.database.path.to_string_lossy().contains("devnet");
    
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
            .layer(axum_middleware::from_fn_with_state(auth_state.clone(), bearer_auth))
    };
    
    // Build auth routes
    // jwt_secret already defined above
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .with_state(Arc::new(WalletAuthState { db: db_pool.clone(), jwt_secret }));
    
    // Build WebSocket routes
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
    let monitoring_routes = match MonitoringState::new(db_pool.clone(), _engine_handle.clone(), config_arc.clone()) {
        Ok(monitoring_state) => {
            let monitoring_state_arc = Arc::new(monitoring_state);
            tracing::info!("Monitoring state initialized successfully, registering monitoring routes");
            Router::new()
                .route("/monitoring/status", get(get_monitoring_status))
                .route("/monitoring/helius-webhook", post(helius_webhook_handler))
                .route("/monitoring/wallets/{wallet_address}/enable", post(enable_wallet_monitoring))
                .route("/monitoring/wallets/{wallet_address}/disable", post(disable_wallet_monitoring))
                .with_state(monitoring_state_arc)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize MonitoringState, monitoring routes disabled");
            Router::new()
        }
    };

    // Create full router with all routes and middleware
    // Note: Layer order matters - bottom layers are applied first (innermost)
    let app = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route("/health", get(health_simple))
        .route("/ws", get(ws_handler))  // Root-level WebSocket for web dashboard
        .with_state(ws_state.clone())
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
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
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
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
            })
            .await
            .unwrap();
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


/// Build list of HMAC secrets from vault and config
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
async fn generate_daily_summary(db: &db::DbPool) -> anyhow::Result<(f64, u32, f64)> {
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
        return Ok((0.0, 0, 0.0));
    }

    let trade_count = trades.len() as u32;
    let mut total_pnl_usd = 0.0;
    let mut winning_trades = 0u32;

    for trade in &trades {
        if let Some(pnl) = trade.pnl_sol {
            // Convert SOL to USD (using approximate rate, should use price cache in production)
            total_pnl_usd += pnl * 100.0; // Approximate SOL price
            if pnl > 0.0 {
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
    use super::*;

    #[test]
    fn test_version() {
        // Ensure version is set
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
