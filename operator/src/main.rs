//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

mod config;
mod db;
mod engine;
mod error;
mod handlers;
mod middleware;
mod models;

use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::AppConfig;
use crate::handlers::{health_check, health_simple, webhook_handler, AppState, WebhookState};
use crate::middleware::HmacState;

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

    // Initialize database
    let db_pool = db::init_pool(&config.database).await?;
    db::run_migrations(&db_pool).await?;
    tracing::info!("Database initialized");

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
    });

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: engine_handle,
    });

    let hmac_state = HmacState::new(
        config.security.webhook_secret.clone(),
        config.security.max_timestamp_drift_secs,
    );

    // Build router
    let app = build_router(app_state, webhook_state, hmac_state);

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .expect("Invalid server address");

    tracing::info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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

/// Build the Axum router with all routes and middleware
fn build_router(
    app_state: Arc<AppState>,
    webhook_state: Arc<WebhookState>,
    hmac_state: HmacState,
) -> Router {
    // Webhook routes (require HMAC authentication)
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .layer(axum_middleware::from_fn_with_state(
            hmac_state,
            middleware::hmac_verify,
        ))
        .with_state(webhook_state);

    // Health routes (no authentication)
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state);

    // Simple health check for load balancers
    let root_routes = Router::new().route("/health", get(health_simple));

    // Combine all routes under /api/v1
    let api_routes = Router::new()
        .merge(webhook_routes)
        .merge(health_routes);

    // Build final router
    Router::new()
        .nest("/api/v1", api_routes)
        .merge(root_routes)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
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
