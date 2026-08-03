//! Health check endpoint

use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerState};
use crate::db_abstraction::Database;
use crate::engine::EngineHandle;
use crate::price_cache::PriceCache;

/// Grace window for transient DB failures (seconds).
/// If the last successful DB check was within this window, a failure is reported
/// as Degraded rather than Unhealthy.
const DB_GRACE_WINDOW_SECS: u64 = 60;

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
    /// Current trade mode
    pub trade_mode: String,
    /// Time spent in fallback mode (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_duration_secs: Option<i64>,
    /// Unique identifier of this process run (C1 evidence)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Git commit hash of the running build (C1 evidence)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_revision: Option<String>,
    /// Admission-threshold config hash in force (C1 evidence)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
}

/// Health status enum
#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
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
    pub db: Arc<dyn Database>,
    /// Engine handle for queue status
    pub engine: EngineHandle,
    /// Application start time
    pub started_at: chrono::DateTime<Utc>,
    /// Circuit breaker
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Price cache
    pub price_cache: Arc<PriceCache>,
    /// Current trade mode
    pub trade_mode: String,
    /// Run-scoped identity (C1). Optional so health still works if the run
    /// context was not constructed (e.g. tests).
    pub run_context: Option<Arc<crate::engine::RunContext>>,
    /// Epoch seconds of the last successful DB health probe (for grace window).
    /// 0 means no success yet; during that state DB failures are always Unhealthy.
    pub last_db_ok_epoch: Arc<std::sync::atomic::AtomicU64>,
}

/// Health check handler
///
/// GET /api/v1/health
pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<HealthResponse>) {
    let now = Utc::now();
    let uptime = (now - state.started_at).num_seconds();

    // Check database health — bounded by a timeout so a *hung* query cannot
    // hang the health endpoint indefinitely (the grace window only applies
    // to errors, not to stalls).
    let db_health = match tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        check_database(state.db.as_ref(), &state.last_db_ok_epoch),
    )
    .await
    {
        Ok(h) => h,
        Err(_) => {
            tracing::error!("Database health check timed out");
            ComponentHealth {
                status: HealthStatus::Unhealthy,
                message: Some("Database health check timed out".to_string()),
            }
        }
    };

    // Get queue depth from engine
    let queue_depth = state.engine.queue_depth();

    // Get last trade timestamp
    let last_trade_at = match tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        get_last_trade_time(state.db.as_ref()),
    )
    .await
    {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("Last-trade query timed out");
            None
        }
    };

    // Get RPC health from executor
    let rpc_health_result = state.engine.get_rpc_health().await;
    let (rpc_health_status, rpc_latency_ms, rpc_message) = match rpc_health_result {
        Some(health) => {
            let status = if health.healthy {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            };
            let latency = health.latency_ms.unwrap_or(0);
            let is_fallback = state.engine.is_in_fallback();
            let message = if !health.healthy {
                Some("RPC health check failed".to_string())
            } else if is_fallback {
                Some("Running in fallback mode (Spear disabled)".to_string())
            } else {
                None
            };
            (status, latency, message)
        }
        None => {
            // No cached health, perform a quick check (non-blocking if possible)
            // For now, mark as degraded if no health info available
            (
                HealthStatus::Degraded,
                0,
                Some("RPC health not yet checked".to_string()),
            )
        }
    };

    // Get fallback duration (time spent in fallback mode)
    let fallback_duration_secs = state.engine.fallback_duration().await.map(|d| d.num_seconds());

    let rpc_health = ComponentHealth {
        status: rpc_health_status,
        message: rpc_message,
    };

    // Get circuit breaker status — derive `trading_allowed` from the SAME
    // snapshot so the response can never expose contradictory state.
    let cb_status = state.circuit_breaker.status();
    let circuit_breaker_health = CircuitBreakerHealth {
        state: cb_status.state.to_string(),
        trading_allowed: cb_status.state == CircuitBreakerState::Active,
        trip_reason: cb_status.trip_reason,
        cooldown_remaining_secs: cb_status.cooldown_remaining_secs,
    };

    // Get price cache stats
    let price_stats = state.price_cache.stats();
    let price_cache_health = PriceCacheHealth {
        total_entries: price_stats.total_entries,
        tracked_tokens: price_stats.tracked_tokens,
    };

    // Determine overall status. A complete RPC outage makes the node unable to
    // trade, so it must surface as Unhealthy (not just Degraded) to load balancers.
    let overall_status = if matches!(db_health.status, HealthStatus::Unhealthy)
        || matches!(rpc_health.status, HealthStatus::Unhealthy)
    {
        HealthStatus::Unhealthy
    } else if matches!(db_health.status, HealthStatus::Degraded)
        || matches!(rpc_health.status, HealthStatus::Degraded)
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
        rpc_latency_ms,
        last_trade_at,
        database: db_health,
        rpc: rpc_health,
        circuit_breaker: circuit_breaker_health,
        price_cache: price_cache_health,
        trade_mode: state.trade_mode.clone(),
        fallback_duration_secs,
        run_id: state.run_context.as_ref().map(|rc| rc.run_id.clone()),
        code_revision: state
            .run_context
            .as_ref()
            .map(|rc| rc.code_revision.clone()),
        config_hash: state.run_context.as_ref().map(|rc| rc.config_hash.clone()),
    };

    (status_code, Json(response))
}

/// Simple health check (for load balancers)
///
/// GET /health
/// Reuses the full health computation so a dead DB, tripped circuit breaker,
/// or broken RPC actually surfaces as non-200 here too.
pub async fn health_simple(State(state): State<Arc<AppState>>) -> StatusCode {
    let (status_code, _) = health_check(State(state)).await;
    status_code
}

/// Check database health
async fn check_database(db: &dyn Database, last_db_ok: &AtomicU64) -> ComponentHealth {
    match db.get_trade_statistics().await {
        Ok(_) => {
            let now = Utc::now().timestamp() as u64;
            last_db_ok.store(now, Ordering::Relaxed);
            ComponentHealth {
                status: HealthStatus::Healthy,
                message: None,
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Database health check failed");
            let now_epoch = Utc::now().timestamp() as u64;
            let (status, message) = determine_db_grace_status(last_db_ok.load(Ordering::Relaxed), now_epoch);
            ComponentHealth {
                status,
                message: message.or(Some(e.to_string())),
            }
        }
    }
}

/// Determine health status for a DB failure based on the grace window.
/// Returns (status, optional_grace_message).
fn determine_db_grace_status(last_db_ok: u64, now_epoch: u64) -> (HealthStatus, Option<String>) {
    if last_db_ok == 0 {
        (HealthStatus::Unhealthy, None)
    } else {
        if now_epoch.saturating_sub(last_db_ok) < DB_GRACE_WINDOW_SECS {
            (
                HealthStatus::Degraded,
                Some(format!(
                    "DB transient failure (last ok {}s ago)",
                    now_epoch.saturating_sub(last_db_ok)
                )),
            )
        } else {
            (HealthStatus::Unhealthy, None)
        }
    }
}

/// Get the timestamp of the last trade
async fn get_last_trade_time(db: &dyn Database) -> Option<String> {
    db.get_recent_trades(1, 0)
        .await
        .ok()
        .and_then(|trades| trades.first().map(|t| t.created_at.to_rfc3339()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_db_grace_status_cold_start_failure() {
        let (status, msg) = determine_db_grace_status(0, 1_000_000);
        assert_eq!(status, HealthStatus::Unhealthy);
        assert!(msg.is_none());
    }

    #[test]
    fn test_determine_db_grace_status_within_window() {
        let now = 1_000_000u64;
        let (status, msg) = determine_db_grace_status(now - 10, now);
        assert_eq!(status, HealthStatus::Degraded);
        assert!(msg.unwrap().contains("last ok 10s ago"));
    }

    #[test]
    fn test_determine_db_grace_status_after_window() {
        let now = 1_000_000u64;
        let (status, msg) = determine_db_grace_status(now - 120, now);
        assert_eq!(status, HealthStatus::Unhealthy);
        assert!(msg.is_none());
    }

    #[test]
    fn test_determine_db_grace_status_exact_boundary() {
        let now = 1_000_000u64;
        // Just inside the window → Degraded
        let (status, _msg) = determine_db_grace_status(now - (DB_GRACE_WINDOW_SECS - 1), now);
        assert_eq!(status, HealthStatus::Degraded);
        // At the window boundary → Unhealthy
        let (status2, _msg2) = determine_db_grace_status(now - DB_GRACE_WINDOW_SECS, now);
        assert_eq!(status2, HealthStatus::Unhealthy);
    }
}
