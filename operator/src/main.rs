//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

use axum::{
    extract::{Request, State},
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

use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::AppConfig;
use chimera_operator::db;
use chimera_operator::engine::{self, RecoveryManager, TipManager};
use chimera_operator::handlers::{
    export_trades, get_config, get_performance_metrics, get_position, get_strategy_performance,
    get_wallet, health_check, health_simple, list_config_audit, list_dead_letter_queue,
    list_positions, list_trades, list_wallets, reset_circuit_breaker, roster_merge,
    roster_validate, update_config, update_wallet, update_reconciliation_metrics,
    update_secret_rotation_metrics, wallet_auth, webhook_handler, ws_handler,
    ApiState, AppState, RosterState, WalletAuthState, WebhookState, WsState,
};
use chimera_operator::middleware::{self, bearer_auth, AuthState, HmacState, Role};
use chimera_operator::metrics::{MetricsState, metrics_router};
use chimera_operator::notifications::{self, CompositeNotifier, DiscordNotifier, NotificationEvent, TelegramNotifier};
use chimera_operator::price_cache::PriceCache;
use chimera_operator::roster;
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::vault;

/// TEST 4: Add engine + recovery manager tasks
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    init_tracing();
    
    tracing::info!("TEST 4: Adding engine + recovery manager tasks");
    
    // Load configuration
    let config = load_config()?;
    tracing::info!(host = %config.server.host, port = config.server.port, "Configuration loaded");
    
    // Initialize database
    let db_pool = db::init_pool(&config.database).await?;
    db::run_migrations(&db_pool).await?;
    tracing::info!("Database initialized");
    
    // Create WebSocket state
    let ws_state = Arc::new(WsState::new());
    
    // Initialize circuit breaker
    let circuit_breaker = Arc::new(CircuitBreaker::new_with_ws(
        config.circuit_breakers.clone(),
        db_pool.clone(),
        Some(ws_state.clone()),
    ));
    
    // Initialize price cache
    let price_cache = Arc::new(PriceCache::new());
    
    // Initialize tip manager
    let tip_manager = Arc::new(TipManager::new(config.jito.clone(), db_pool.clone()));
    let _ = tip_manager.init().await;
    
    // Initialize notification service
    let notifier = Arc::new(notifications::CompositeNotifier::new());
    
    // Create engine
    let (engine, _engine_handle) = engine::Engine::new_with_extras_and_tip_manager(
        config.clone(),
        db_pool.clone(),
        notifier.clone(),
        None,
        Some(ws_state.clone()),
        Some(tip_manager.clone()),
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
    
    // Spawn circuit breaker task
    let cb_clone = circuit_breaker.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = cb_clone.evaluate().await {
                tracing::error!(error = %e, "Circuit breaker evaluation failed");
            }
        }
    });
    
    // Spawn price cache updater
    let price_cache_clone = price_cache.clone();
    tokio::spawn(async move {
        price_cache_clone.start_updater().await;
    });
    
    // Spawn periodic RPC health check task
    let engine_handle_rpc = _engine_handle.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            engine_handle_rpc.refresh_rpc_health().await;
        }
    });
    tracing::info!("RPC health check task started");
    
    tracing::info!("All background tasks spawned");
    
    // Now create the FULL router with all routes
    tracing::info!("Creating full router with states...");
    
    // Create app state
    let metrics_state = Arc::new(MetricsState::new());
    
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
    let auth_state = Arc::new(AuthState::new(db_pool.clone()));
    
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
        .route("/metrics/reconciliation", post(update_reconciliation_metrics))
        .route("/metrics/secret-rotation", post(update_secret_rotation_metrics))
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));
    
    // Create webhook state
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
    let token_parser = Arc::new(TokenParser::new(token_safety_config, token_cache.clone(), token_fetcher));
    
    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        token_parser,
        circuit_breaker: circuit_breaker.clone(),
    });
    
    // Create roster state
    let roster_path = config.database.path.parent()
        .map(|p| p.join("roster_new.db"))
        .unwrap_or_else(|| PathBuf::from("roster_new.db"));
    let roster_state = Arc::new(RosterState {
        db: db_pool.clone(),
        default_roster_path: roster_path,
    });
    
    // Build webhook routes
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(webhook_state.clone());
    
    // Build roster routes
    let roster_routes = Router::new()
        .route("/roster/merge", post(roster_merge))
        .route("/roster/validate", get(roster_validate))
        .with_state(roster_state.clone())
        .layer(axum_middleware::from_fn_with_state(auth_state.clone(), bearer_auth));
    
    // Build auth routes
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .with_state(Arc::new(WalletAuthState { db: db_pool.clone(), jwt_secret }));
    
    // Build WebSocket routes
    let ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());
    
    // Build metrics routes
    let metrics_routes = metrics_router().with_state(metrics_state.clone());
    
    // Rate limiter disabled - tower_governor needs special config for docker networking
    // Can be re-enabled with proper key extractor configuration
    
    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    
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
    // Note: Rate limiting disabled for now - governor needs special configuration for docker
    // .layer(governor_layer)
    
    tracing::info!("Full router created with all routes and middleware");
    
    // Start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .expect("Invalid server address");
    
    tracing::info!(%addr, "Starting server with FULL router");
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Server listening on {}", addr);
    
    axum::serve(listener, app).await?;
    
    Ok(())
}

#[allow(dead_code)]
async fn main_full() -> anyhow::Result<()> {
    // Initialize tracing
    init_tracing();

    tracing::info!("Starting Chimera Operator v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = load_config()?;
    tracing::info!(
        host = %config.server.host,
        port = config.server.port,
        "Configuration loaded"
    );

    // Try to load secrets from vault, fall back to env vars
    let secrets = vault::load_secrets_with_fallback().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to load vault secrets, using env vars only");
        vault::VaultSecrets {
            webhook_secret: config.security.webhook_secret.clone(),
            webhook_secret_previous: config.security.webhook_secret_previous.clone(),
            wallet_private_key: None,
            rpc_api_key: None,
            fallback_rpc_api_key: None,
        }
    });

    // Initialize database
    let db_pool = db::init_pool(&config.database).await?;
    db::run_migrations(&db_pool).await?;
    tracing::info!("Database initialized");

    // Create WebSocket state for real-time updates (needed for circuit breaker)
    let ws_state = Arc::new(WsState::new());

    // Initialize circuit breaker with WebSocket support
    let circuit_breaker = Arc::new(CircuitBreaker::new_with_ws(
        config.circuit_breakers.clone(),
        db_pool.clone(),
        Some(ws_state.clone()),
    ));
    tracing::info!("Circuit breaker initialized");

    // Initialize price cache
    let price_cache = Arc::new(PriceCache::new());
    tracing::info!("Price cache initialized");

    // Initialize Jito tip manager
    let tip_manager = Arc::new(TipManager::new(config.jito.clone(), db_pool.clone()));
    if let Err(e) = tip_manager.init().await {
        tracing::warn!(error = %e, "Failed to initialize tip manager from history");
    }
    tracing::info!(
        cold_start = tip_manager.is_cold_start(),
        "Tip manager initialized"
    );

    // Initialize token parser
    let token_cache = Arc::new(TokenCache::new(
        config.token_safety.cache_capacity,
        config.token_safety.cache_ttl_seconds,
    ));

    let token_fetcher = Arc::new(TokenMetadataFetcher::new(&config.rpc.primary_url));

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
        token_fetcher,
    ));
    tracing::info!("Token parser initialized");

    // Initialize notification service
    let notifier = {
        let mut composite = CompositeNotifier::new();

        // Add Discord notifier if configured
        if let Some(discord) = DiscordNotifier::from_env() {
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
                composite.add_service(Arc::new(TelegramNotifier::new(telegram_config)));
                tracing::info!("Telegram notifications enabled");
            } else {
                tracing::warn!(
                    "Telegram notifications enabled in config but bot_token/chat_id not set"
                );
            }
        }

        // Add Discord notifier if configured via environment variable
        if let Some(discord) = DiscordNotifier::from_env() {
            composite.add_service(Arc::new(discord));
            tracing::info!("Discord notifications enabled");
        }

        Arc::new(composite)
    };
    tracing::info!("Notification service initialized");

    // Initialize stuck-state recovery manager with WebSocket support
    let recovery_manager = Arc::new(RecoveryManager::new_with_ws(
        db_pool.clone(),
        config.rpc.primary_url.clone(),
        Some(ws_state.clone()),
    ));

    // Spawn recovery background task
    let recovery_manager_clone = recovery_manager.clone();
    tokio::spawn(async move {
        recovery_manager_clone.start_background_task().await;
    });
    tracing::info!("Stuck-state recovery manager started");

    // Spawn price cache updater
    let price_cache_clone = price_cache.clone();
    tokio::spawn(async move {
        price_cache_clone.start_updater().await;
    });
    tracing::info!("Price cache updater started");

    // Spawn circuit breaker evaluation task with notification support
    let circuit_breaker_clone = circuit_breaker.clone();
    let notifier_cb = notifier.clone();
    let notify_rules = config.notifications.rules.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut was_tripped = false;

        loop {
            interval.tick().await;
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
    });
    tracing::info!("Circuit breaker evaluation task started");

    // Spawn daily summary notification task
    let notifier_daily = notifier.clone();
    let db_pool_daily = db_pool.clone();
    let daily_config = config.notifications.daily_summary.clone();
    let notify_daily_enabled = config.notifications.rules.daily_summary;
    tokio::spawn(async move {
        if !daily_config.enabled || !notify_daily_enabled {
            tracing::info!("Daily summary notifications disabled");
            return;
        }

        tracing::info!(
            hour = daily_config.hour_utc,
            minute = daily_config.minute,
            "Daily summary task started"
        );

        loop {
            // Calculate time until next summary
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
            tracing::debug!(
                sleep_seconds = sleep_duration.as_secs(),
                "Sleeping until next daily summary"
            );
            tokio::time::sleep(sleep_duration).await;

            // Generate and send daily summary
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
    tracing::info!("Daily summary notification task started");

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

    // Create metrics state
    let metrics_state = Arc::new(MetricsState::new());

    // Create engine with notification, metrics, WebSocket support, and tip manager
    // (ws_state already created above)
    let (engine, engine_handle) = engine::Engine::new_with_extras_and_tip_manager(
        config.clone(),
        db_pool.clone(),
        notifier.clone(),
        Some(metrics_state.clone()),
        Some(ws_state.clone()),
        Some(tip_manager.clone()),
    );

    // Spawn engine processing loop
    tokio::spawn(async move {
        engine.run().await;
    });
    tracing::info!("Engine started");

    // Spawn metrics update task
    let metrics_state_clone = metrics_state.clone();
    let circuit_breaker_clone = circuit_breaker.clone();
    let db_pool_metrics = db_pool.clone();
    // Spawn periodic RPC health check task
    let engine_handle_rpc = engine_handle.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            engine_handle_rpc.refresh_rpc_health().await;
        }
    });
    tracing::info!("RPC health check task started");

    let engine_handle_metrics = engine_handle.clone();
    let ws_state_metrics = ws_state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;

            // Update circuit breaker state
            let cb_state = circuit_breaker_clone.current_state();
            let is_active = cb_state == chimera_operator::circuit_breaker::CircuitBreakerState::Active;
            metrics_state_clone
                .circuit_breaker_state
                .set(if is_active { 1 } else { 0 });

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
    });
    tracing::info!("Metrics update task started");

    // Clone engine handle for all states that need it
    let engine_handle_for_app = engine_handle.clone();
    let engine_handle_for_webhook = engine_handle.clone();
    let engine_handle_for_api = engine_handle.clone();
    
    // Create shared state
    let app_state = Arc::new(AppState {
        db: db_pool.clone(),
        engine: engine_handle_for_app,
        started_at: Utc::now(),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
    });

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: engine_handle_for_webhook,
        token_parser,
        circuit_breaker: circuit_breaker.clone(),
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

    // Create API state with shared config
    let shared_config = Arc::new(tokio::sync::RwLock::new(config.clone()));
    let api_state = Arc::new(ApiState {
        db: db_pool.clone(),
        circuit_breaker: circuit_breaker.clone(),
        config: shared_config.clone(),
        notifier: notifier.clone(),
        engine: Some(Arc::new(engine_handle_for_api)),
        metrics: metrics_state.clone(),
    });

    // WebSocket state is created above with engine
    tracing::info!("WebSocket broadcast channel initialized");

    // Create auth state with API keys from config
    let mut api_keys_map = std::collections::HashMap::new();
    for key_config in &config.security.api_keys {
        if let Ok(role) = key_config.role.parse::<Role>() {
            api_keys_map.insert(key_config.key.clone(), role);
            tracing::debug!(role = %role, "API key configured");
        } else {
            tracing::warn!(role = %key_config.role, "Invalid role in API key config");
        }
    }
    let auth_state = Arc::new(AuthState::with_api_keys(db_pool.clone(), api_keys_map));
    tracing::info!(
        api_key_count = config.security.api_keys.len(),
        "Auth state initialized"
    );

    // Spawn TTL expiration background task
    let db_pool_ttl = db_pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
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
                            // Log to config_audit
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
    });
    tracing::info!("Wallet TTL expiration task started");

    // STEP A: Comment out middleware for minimal test
    /*
    // Build HMAC secrets for webhook verification
    let hmac_secrets = build_hmac_secrets(&secrets, &config);
    let hmac_state = Arc::new(HmacState::with_rotation(hmac_secrets, 300)); // 5 minute drift window

    // Build rate limiter
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(100)
        .burst_size(200)
        .finish()
        .unwrap();
    let governor_layer = GovernorLayer::new(governor_conf);

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    */

    // STEP A: Comment out all complex routes for minimal test
    // Build API routes (health check needs AppState, others need ApiState)
    /*
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state.clone());
    
    let api_routes = Router::new()
        .route("/positions", get(list_positions))
        .route("/positions/{trade_uuid}", get(get_position))
        .route("/wallets", get(list_wallets))
        .route("/wallets/{address}", get(get_wallet).put(update_wallet))
        .route("/trades", get(list_trades))
        .route("/trades/export", get(export_trades))
        .route("/config", get(get_config).put(update_config))
        .route("/config/circuit-breaker/reset", post(reset_circuit_breaker))
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/strategy/{strategy}", get(get_strategy_performance))
        .route("/metrics/reconciliation", post(update_reconciliation_metrics))
        .route("/metrics/secret-rotation", post(update_secret_rotation_metrics))
        .route("/incidents/dead-letter", get(list_dead_letter_queue))
        .route("/incidents/config-audit", get(list_config_audit))
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Build webhook routes
    // Note: HMAC verification is handled in the webhook handler itself for now
    // TODO: Add proper HMAC middleware once the server is working
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(webhook_state.clone());

    // Build roster routes
    let roster_routes = Router::new()
        .route("/roster/merge", post(roster_merge))
        .route("/roster/validate", get(roster_validate))
        .with_state(roster_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Build auth routes
    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "dev-secret-change-in-production".to_string());
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .with_state(Arc::new(WalletAuthState {
            db: db_pool.clone(),
            jwt_secret,
        }));

    // Build WebSocket routes
    let ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());

    // Build metrics routes
    let metrics_routes = metrics_router().with_state(metrics_state.clone());
    */

    // STEP A: Test with absolutely minimal router (no state, no middleware, no nesting)
    tracing::info!("STEP A: Building minimal test router (no state, no middleware)");
    
    let app = Router::new()
        .route("/ping", get(|| async {
            tracing::info!("PING endpoint called - minimal router test");
            "pong"
        }))
        .route("/test", get(|| async {
            tracing::info!("TEST endpoint called - minimal router test");
            "test-ok"
        }));
    
    tracing::info!("STEP A: Minimal router built - testing with only /ping and /test");

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .expect("Invalid server address");

    tracing::info!(%addr, "Starting Chimera Operator server");
    
    // Add a simple test to verify the runtime is working
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            tracing::debug!("Runtime heartbeat - server should be processing requests");
        }
    });
    
    tracing::info!("Binding TCP listener");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Server listening on {}", addr);
    
    // Try using tokio::spawn to run the server
    tracing::info!("Starting HTTP server - calling axum::serve directly");
    // Use axum::serve directly - it should block forever processing requests
    let result = axum::serve(listener, app).await;
    tracing::error!("axum::serve returned unexpectedly: {:?}", result);
    result?;

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
