//! Health check endpoint

use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::Serialize;
use std::sync::Arc;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerState};
use crate::db::DbPool;
use crate::engine::EngineHandle;
use crate::price_cache::PriceCache;

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Overall system status
    pub status: HealthStatus,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Current queue depth
    pub queue_depth: usize,
    /// RPC latency in milliseconds (0 if not available)
    pub rpc_latency_ms: u64,
    /// Timestamp of last trade
    pub last_trade_at: Option<String>,
    /// Database status
    pub database: ComponentHealth,
    /// RPC status
    pub rpc: ComponentHealth,
    /// Circuit breaker status
    pub circuit_breaker: CircuitBreakerHealth,
    /// Price cache status
    pub price_cache: PriceCacheHealth,
}

/// Health status enum
#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// All systems operational
    Healthy,
    /// Some systems degraded but operational
    Degraded,
    /// Critical systems failing
    Unhealthy,
}

/// Component health status
#[derive(Debug, Serialize)]
pub struct ComponentHealth {
    pub status: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Circuit breaker health info
#[derive(Debug, Serialize)]
pub struct CircuitBreakerHealth {
    pub state: String,
    pub trading_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_secs: Option<i64>,
}

/// Price cache health info
#[derive(Debug, Serialize)]
pub struct PriceCacheHealth {
    pub total_entries: usize,
    pub tracked_tokens: usize,
}

/// Shared application state for health checks
pub struct AppState {
    /// Database connection pool
    pub db: DbPool,
    /// Engine handle for queue status
    pub engine: EngineHandle,
    /// Application start time
    pub started_at: chrono::DateTime<Utc>,
    /// Circuit breaker
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Price cache
    pub price_cache: Arc<PriceCache>,
}

/// Health check handler
///
/// GET /api/v1/health
pub async fn health_check(State(state): State<Arc<AppState>>) -> (StatusCode, Json<HealthResponse>) {
    let now = Utc::now();
    let uptime = (now - state.started_at).num_seconds();

    // Check database health
    let db_health = check_database(&state.db).await;

    // Get queue depth from engine
    let queue_depth = state.engine.queue_depth();

    // Get last trade timestamp
    let last_trade_at = get_last_trade_time(&state.db).await;

    // Get circuit breaker status
    let cb_status = state.circuit_breaker.status();
    let circuit_breaker_health = CircuitBreakerHealth {
        state: cb_status.state.to_string(),
        trading_allowed: state.circuit_breaker.is_trading_allowed(),
        trip_reason: cb_status.trip_reason,
        cooldown_remaining_secs: cb_status.cooldown_remaining_secs,
    };

    // Get price cache stats
    let price_stats = state.price_cache.stats();
    let price_cache_health = PriceCacheHealth {
        total_entries: price_stats.total_entries,
        tracked_tokens: price_stats.tracked_tokens,
    };

    // Determine overall status
    let overall_status = if matches!(db_health.status, HealthStatus::Unhealthy) {
        HealthStatus::Unhealthy
    } else if matches!(db_health.status, HealthStatus::Degraded)
        || queue_depth > 800
        || cb_status.state == CircuitBreakerState::Tripped
    {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    };

    let status_code = match overall_status {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Degraded => StatusCode::OK, // Still return 200 for degraded
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    };

    let response = HealthResponse {
        status: overall_status,
        uptime_seconds: uptime,
        queue_depth,
        rpc_latency_ms: 0, // TODO: Implement RPC latency tracking
        last_trade_at,
        database: db_health,
        rpc: ComponentHealth {
            status: HealthStatus::Healthy, // TODO: Implement RPC health tracking
            message: None,
        },
        circuit_breaker: circuit_breaker_health,
        price_cache: price_cache_health,
    };

    (status_code, Json(response))
}

/// Simple health check (for load balancers)
///
/// GET /health
pub async fn health_simple() -> StatusCode {
    StatusCode::OK
}

/// Check database health
async fn check_database(pool: &DbPool) -> ComponentHealth {
    match sqlx::query("SELECT 1").fetch_one(pool).await {
        Ok(_) => ComponentHealth {
            status: HealthStatus::Healthy,
            message: None,
        },
        Err(e) => {
            tracing::error!(error = %e, "Database health check failed");
            ComponentHealth {
                status: HealthStatus::Unhealthy,
                message: Some(e.to_string()),
            }
        }
    }
}

/// Get the timestamp of the last trade
async fn get_last_trade_time(pool: &DbPool) -> Option<String> {
    let result: Result<(String,), _> = sqlx::query_as(
        "SELECT created_at FROM trades ORDER BY created_at DESC LIMIT 1"
    )
    .fetch_one(pool)
    .await;

    result.ok().map(|(ts,)| ts)
}
