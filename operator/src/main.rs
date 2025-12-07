//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

mod circuit_breaker;
mod config;
mod db;
mod engine;
mod error;
mod handlers;
mod middleware;
mod models;
mod price_cache;
mod roster;
mod token;
mod vault;

use axum::{
    middleware as axum_middleware,
    routing::{get, post},
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

use crate::circuit_breaker::CircuitBreaker;
use crate::config::AppConfig;
use crate::engine::{RecoveryManager, TipManager};
use crate::handlers::{
    health_check, health_simple, roster_merge, roster_validate, webhook_handler, AppState,
    RosterState, WebhookState,
};
use crate::middleware::HmacState;
use crate::price_cache::PriceCache;
use crate::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    // Initialize circuit breaker
    let circuit_breaker = Arc::new(CircuitBreaker::new(
        config.circuit_breakers.clone(),
        db_pool.clone(),
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

    // Initialize stuck-state recovery manager
    let recovery_manager = Arc::new(RecoveryManager::new(db_pool.clone()));

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

    // Spawn circuit breaker evaluation task
    let circuit_breaker_clone = circuit_breaker.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = circuit_breaker_clone.evaluate().await {
                tracing::error!(error = %e, "Circuit breaker evaluation failed");
            }
        }
    });
    tracing::info!("Circuit breaker evaluation task started");

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

    // Create engine
    let (engine, engine_handle) = engine::Engine::new(config.clone(), db_pool.clone());

    // Spawn engine processing loop
    tokio::spawn(async move {
        engine.run().await;
    });
    tracing::info!("Engine started");

    // Create shared state
    let app_state = Arc::new(AppState {
        db: db_pool.clone(),
        engine: engine_handle.clone(),
        started_at: Utc::now(),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
    });

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: engine_handle,
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

    // Create HMAC state with rotation support
    let hmac_secrets = build_hmac_secrets(&secrets, &config);
    let hmac_state = HmacState::with_rotation(hmac_secrets, config.security.max_timestamp_drift_secs);

    if hmac_state.is_rotation_active() {
        tracing::info!("Secret rotation grace period active - accepting both current and previous secrets");
    }

    // Create rate limiter configuration
    let rate_limit_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(config.security.webhook_rate_limit as u64)
            .burst_size(config.security.webhook_burst_size)
            .finish()
            .expect("Failed to create rate limiter config"),
    );

    tracing::info!(
        rate_limit = config.security.webhook_rate_limit,
        burst_size = config.security.webhook_burst_size,
        "Rate limiting configured"
    );

    // Build router with rate limiting
    let rate_limit_layer = GovernorLayer::new(rate_limit_config);

    // Webhook routes (require HMAC authentication + rate limiting)
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .layer(axum_middleware::from_fn_with_state(
            hmac_state,
            middleware::hmac_verify,
        ))
        .layer(rate_limit_layer)
        .with_state(webhook_state);

    // Health routes (no authentication)
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state);

    // Roster management routes (should have admin auth in production)
    let roster_routes = Router::new()
        .route("/roster/merge", post(roster_merge))
        .route("/roster/validate", get(roster_validate))
        .with_state(roster_state);

    // Simple health check for load balancers
    let root_routes = Router::new().route("/health", get(health_simple));

    // Combine all routes under /api/v1
    let api_routes = Router::new()
        .merge(webhook_routes)
        .merge(health_routes)
        .merge(roster_routes);

    // Build final router
    let app = Router::new()
        .nest("/api/v1", api_routes)
        .merge(root_routes)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .expect("Invalid server address");

    tracing::info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        // Ensure version is set
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
