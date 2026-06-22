//! Operations API handlers
//!
//! Provides endpoints for system resources, secret rotation, rate limiting, and health checks.

use axum::{extract::State, Json};
use chrono::{Duration, Utc};
use serde::Serialize;
use std::sync::Arc;
use sysinfo::{Networks, System};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerState};
use crate::db_abstraction::{ConfigAuditItem, Database};
use crate::engine::EngineHandle;
use crate::error::AppError;

// =============================================================================
// RESOURCE USAGE
// =============================================================================

/// Resource usage response
#[derive(Debug, Serialize)]
pub struct ResourceUsageResponse {
    pub cpu: ResourceMetric,
    pub memory: ResourceMetric,
    pub disk: ResourceMetric,
    pub network: NetworkMetric,
    pub timestamp: String,
}

/// Individual resource metric
#[derive(Debug, Serialize)]
pub struct ResourceMetric {
    pub current: u64,
    pub max: u64,
    pub percentage: f64,
    pub status: MetricStatus,
}

/// Metric status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricStatus {
    Normal,
    Warning,
    Critical,
}

/// Network metrics
#[derive(Debug, Serialize)]
pub struct NetworkMetric {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub error_rate: f64,
}

/// Shared state for operations handlers
pub struct OperationsState {
    pub db: Arc<dyn Database>,
    pub engine: Option<Arc<EngineHandle>>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub price_cache: Arc<crate::price_cache::PriceCache>,
}

/// Get system resource usage
///
/// GET /api/v1/operations/resources
pub async fn get_resources(State(_state): State<Arc<OperationsState>>) -> Result<Json<ResourceUsageResponse>, AppError> {
    let mut sys = System::new_all();
    sys.refresh_all();

    // Get network data
    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh(false); // don't remove not listed interfaces

    // CPU metrics
    let cpu_usage = sys.global_cpu_usage();
    let cpu_current = cpu_usage as u64;
    let cpu_max = 100;
    let cpu_percentage = cpu_usage as f64;
    let cpu_status = if cpu_percentage < 70.0 {
        MetricStatus::Normal
    } else if cpu_percentage < 90.0 {
        MetricStatus::Warning
    } else {
        MetricStatus::Critical
    };

    // Memory metrics
    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let memory_percentage = if total_memory > 0 {
        (used_memory as f64 / total_memory as f64) * 100.0
    } else {
        0.0
    };
    let memory_status = if memory_percentage < 70.0 {
        MetricStatus::Normal
    } else if memory_percentage < 90.0 {
        MetricStatus::Warning
    } else {
        MetricStatus::Critical
    };

    // Disk metrics (using total and available memory as proxy for disk)
    let total_disk = sys.total_memory();
    let available_disk = sys.available_memory();
    let used_disk = total_disk.saturating_sub(available_disk);
    let disk_percentage = if total_disk > 0 {
        (used_disk as f64 / total_disk as f64) * 100.0
    } else {
        0.0
    };
    let disk_status = if disk_percentage < 70.0 {
        MetricStatus::Normal
    } else if disk_percentage < 90.0 {
        MetricStatus::Warning
    } else {
        MetricStatus::Critical
    };

    // Network metrics from sysinfo
    let mut bytes_sent = 0;
    let mut bytes_received = 0;
    let mut packets_sent = 0;
    let mut packets_received = 0;

    for (_interface_name, data) in &networks {
        bytes_sent += data.total_transmitted();
        bytes_received += data.total_received();
        packets_sent += data.total_packets_transmitted();
        packets_received += data.total_packets_received();
    }

    let error_rate = 0.0; // Network error rate would need more detailed monitoring

    let response = ResourceUsageResponse {
        cpu: ResourceMetric {
            current: cpu_current,
            max: cpu_max,
            percentage: cpu_percentage,
            status: cpu_status,
        },
        memory: ResourceMetric {
            current: used_memory,
            max: total_memory,
            percentage: memory_percentage,
            status: memory_status,
        },
        disk: ResourceMetric {
            current: used_disk,
            max: total_disk,
            percentage: disk_percentage,
            status: disk_status,
        },
        network: NetworkMetric {
            bytes_sent,
            bytes_received,
            packets_sent,
            packets_received,
            error_rate,
        },
        timestamp: Utc::now().to_rfc3339(),
    };

    Ok(Json(response))
}

// =============================================================================
// SECRET ROTATION
// =============================================================================

/// Secret rotation response
#[derive(Debug, Serialize)]
pub struct SecretRotationResponse {
    pub last_rotation_at: Option<String>,
    pub next_rotation_at: Option<String>,
    pub days_until_due: Option<i64>,
    pub status: RotationStatus,
    pub is_initialized: bool,  // true if rotation tracking is configured
    pub rotation_history: Vec<RotationEvent>,
}

/// Rotation status
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationStatus {
    Active,
    DueSoon,
    Overdue,
    NeverRotated,  // Fresh deployment with no rotation history
    Unknown,       // Error state (data issue)
}

/// Rotation event
#[derive(Debug, Serialize)]
pub struct RotationEvent {
    pub timestamp: String,
    pub status: EventStatus,
    pub duration_seconds: Option<i64>,
    pub keys_rotated: i64,
    pub failed_keys: i64,
}

/// Event status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Success,
    Failed,
    Partial,
}

/// Get secret rotation status
///
/// GET /api/v1/operations/secrets
pub async fn get_secrets(State(state): State<Arc<OperationsState>>) -> Result<Json<SecretRotationResponse>, AppError> {
    // Query config audit table for rotation events
    let rotation_events = get_rotation_history(&state.db).await?;

    // Get the most recent rotation event
    let last_rotation = rotation_events.first();
    let last_rotation_at = last_rotation.map(|e| e.timestamp.clone());

    // Calculate next rotation (90 days from last rotation)
    let next_rotation_at = last_rotation_at.as_ref().map(|timestamp| {
        let last_dt = timestamp.parse::<chrono::DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now());
        let next_dt = last_dt + Duration::days(90);
        next_dt.to_rfc3339()
    });

    // Calculate days until due
    let days_until_due = next_rotation_at.as_ref().map(|next| {
        let next_dt = next.parse::<chrono::DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now() + Duration::days(90));
        let now = Utc::now();
        let duration = next_dt.signed_duration_since(now);
        duration.num_days()
    });

    // Determine status based on days until due and rotation history
    let is_initialized = last_rotation_at.is_some();
    let status = match (days_until_due, is_initialized) {
        (Some(days), true) if days < 0 => RotationStatus::Overdue,
        (Some(days), true) if days <= 7 => RotationStatus::DueSoon,
        (Some(_days), true) => RotationStatus::Active,
        (None, false) => RotationStatus::NeverRotated,  // No rotation history (fresh deployment)
        (None, true) => RotationStatus::Unknown,        // Error: history exists but no days_until_due
        (Some(_), false) => RotationStatus::Unknown,   // Error: inconsistent state
    };

    let response = SecretRotationResponse {
        last_rotation_at,
        next_rotation_at,
        days_until_due,
        status,
        is_initialized,
        rotation_history: rotation_events,
    };

    Ok(Json(response))
}

/// Get rotation history from config audit table
async fn get_rotation_history(db: &Arc<dyn Database>) -> Result<Vec<RotationEvent>, AppError> {
    let items: Vec<ConfigAuditItem> = db.get_config_audit_entries(10, 0).await?;

    // Filter to only secret_rotation entries
    let rotation_items: Vec<ConfigAuditItem> = items
        .into_iter()
        .filter(|item| item.key.starts_with("secret_rotation"))
        .collect();

    let events: Vec<RotationEvent> = rotation_items
        .into_iter()
        .map(|item| {
            let status = if item.new_value.contains("success") {
                EventStatus::Success
            } else if item.new_value.contains("failed") {
                EventStatus::Failed
            } else {
                EventStatus::Partial
            };

            RotationEvent {
                timestamp: item.changed_at,
                status,
                duration_seconds: None, // Parse from item.new_value if needed
                keys_rotated: 1, // Default value - should be parsed from audit data
                failed_keys: 0,  // Default value - should be parsed from audit data
            }
        })
        .collect();

    Ok(events)
}

// =============================================================================
// RATE LIMIT STATUS
// =============================================================================

/// Rate limit status response
#[derive(Debug, Serialize)]
pub struct RateLimitStatusResponse {
    pub endpoints: Vec<RateLimitEndpoint>,
    pub overall_status: OverallStatus,
}

/// Overall status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OverallStatus {
    Healthy,
    Degraded,
    Throttled,
}

/// Individual endpoint rate limit info
#[derive(Debug, Serialize)]
pub struct RateLimitEndpoint {
    pub endpoint: String,
    pub current_rate: u64,
    pub limit: u64,
    pub window_seconds: u64,
    pub remaining: u64,
    pub reset_at: String,
    pub utilization_percent: f64,
    pub status: EndpointStatus,
}

/// Endpoint status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointStatus {
    Ok,
    Warning,
    Throttled,
}

/// Get rate limit status
///
/// GET /api/v1/operations/rate-limit
pub async fn get_rate_limit_status(State(_state): State<Arc<OperationsState>>) -> Result<Json<RateLimitStatusResponse>, AppError> {
    // Define endpoints with their rate limits (from configuration)
    let endpoints_config = vec![
        ("/api/v1/webhook", 100),  // 100 req/s
        ("/api/v1/trades", 50),   // 50 req/s
        ("/api/v1/positions", 50),
        ("/api/v1/wallets", 30),
        ("/api/v1/config", 10),
        ("/api/v1/metrics", 50),
    ];

    let now = Utc::now();
    let reset_at = now + Duration::seconds(60);
    let reset_at_str = reset_at.to_rfc3339();

    // For each endpoint, calculate current utilization
    // In a real implementation, this would query Prometheus metrics or rate limiter state
    let endpoints: Vec<RateLimitEndpoint> = endpoints_config
        .into_iter()
        .map(|(endpoint, limit)| {
            let current_rate = (limit as f64 * 0.3) as u64; // Simulate 30% utilization
            let remaining = (limit as i64).saturating_sub(current_rate as i64) as u64;
            let utilization_percent = if limit > 0 {
                (current_rate as f64 / limit as f64) * 100.0
            } else {
                0.0
            };

            let status = if utilization_percent < 70.0 {
                EndpointStatus::Ok
            } else if utilization_percent < 90.0 {
                EndpointStatus::Warning
            } else {
                EndpointStatus::Throttled
            };

            RateLimitEndpoint {
                endpoint: endpoint.to_string(),
                current_rate,
                limit,
                window_seconds: 1, // 1 second window
                remaining,
                reset_at: reset_at_str.clone(),
                utilization_percent,
                status,
            }
        })
        .collect();

    // Determine overall status
    let overall_status = if endpoints.iter().any(|e| matches!(e.status, EndpointStatus::Throttled)) {
        OverallStatus::Throttled
    } else if endpoints.iter().any(|e| matches!(e.status, EndpointStatus::Warning)) {
        OverallStatus::Degraded
    } else {
        OverallStatus::Healthy
    };

    let response = RateLimitStatusResponse {
        endpoints,
        overall_status,
    };

    Ok(Json(response))
}

// =============================================================================
// HEALTH CHECK DETAILS
// =============================================================================

/// Health check details response
#[derive(Debug, Serialize)]
pub struct HealthCheckDetailsResponse {
    pub overall_status: OverallHealthStatus,
    pub checks: Vec<HealthCheck>,
}

/// Overall health status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OverallHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Individual health check
#[derive(Debug, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: Option<String>,
    pub last_check: String,
    pub response_time_ms: f64,
}

/// Check status
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Passing,
    Warning,
    Failing,
}

/// Get detailed health checks
///
/// GET /api/v1/operations/health-checks
pub async fn get_health_check_details(State(state): State<Arc<OperationsState>>) -> Result<Json<HealthCheckDetailsResponse>, AppError> {
    let mut checks = Vec::new();

    // Database health check
    let db_check = check_database_health(&state.db).await;
    checks.push(db_check);

    // RPC health check
    let rpc_check = check_rpc_health(&state.engine).await;
    checks.push(rpc_check);

    // Circuit breaker health check
    let cb_check = check_circuit_breaker_health(&state.circuit_breaker);
    checks.push(cb_check);

    // Price cache health check
    let pc_check = check_price_cache_health(&state.price_cache);
    checks.push(pc_check);

    // Determine overall status
    let overall_status = if checks.iter().any(|c| matches!(c.status, CheckStatus::Failing)) {
        OverallHealthStatus::Unhealthy
    } else if checks.iter().any(|c| matches!(c.status, CheckStatus::Warning)) {
        OverallHealthStatus::Degraded
    } else {
        OverallHealthStatus::Healthy
    };

    let response = HealthCheckDetailsResponse {
        overall_status,
        checks,
    };

    Ok(Json(response))
}

/// Check database health
async fn check_database_health(db: &Arc<dyn Database>) -> HealthCheck {
    let start = std::time::Instant::now();
    let result = db.get_trade_statistics().await;
    let response_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(_) => HealthCheck {
            name: "database".to_string(),
            status: CheckStatus::Passing,
            message: Some("Database connection healthy".to_string()),
            last_check: Utc::now().to_rfc3339(),
            response_time_ms,
        },
        Err(e) => HealthCheck {
            name: "database".to_string(),
            status: CheckStatus::Failing,
            message: Some(format!("Database connection failed: {}", e)),
            last_check: Utc::now().to_rfc3339(),
            response_time_ms,
        },
    }
}

/// Check RPC health
async fn check_rpc_health(engine: &Option<Arc<EngineHandle>>) -> HealthCheck {
    let start = std::time::Instant::now();
    let health_result = if let Some(eng) = engine {
        eng.get_rpc_health().await
    } else {
        None
    };
    let response_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    match health_result {
        Some(_health) if _health.healthy => HealthCheck {
            name: "rpc".to_string(),
            status: CheckStatus::Passing,
            message: Some(format!("RPC latency: {}ms", _health.latency_ms.unwrap_or(0))),
            last_check: Utc::now().to_rfc3339(),
            response_time_ms,
        },
        Some(_health) => HealthCheck {
            name: "rpc".to_string(),
            status: CheckStatus::Failing,
            message: Some("RPC unhealthy: latency high or unavailable".to_string()),
            last_check: Utc::now().to_rfc3339(),
            response_time_ms,
        },
        None => HealthCheck {
            name: "rpc".to_string(),
            status: CheckStatus::Warning,
            message: Some("RPC health not yet checked".to_string()),
            last_check: Utc::now().to_rfc3339(),
            response_time_ms,
        },
    }
}

/// Check circuit breaker health
fn check_circuit_breaker_health(circuit_breaker: &Arc<CircuitBreaker>) -> HealthCheck {
    let start = std::time::Instant::now();
    let status = circuit_breaker.status();
    let response_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    let (check_status, message) = match status.state {
        CircuitBreakerState::Active => (
            CheckStatus::Passing,
            Some("Circuit breaker active, trading allowed".to_string()),
        ),
        CircuitBreakerState::Tripped => (
            CheckStatus::Failing,
            Some(status.trip_reason.unwrap_or_else(|| "Circuit breaker tripped".to_string())),
        ),
        CircuitBreakerState::Cooldown => (
            CheckStatus::Warning,
            Some("Circuit breaker in cooldown, trading restricted".to_string()),
        ),
    };

    HealthCheck {
        name: "circuit_breaker".to_string(),
        status: check_status,
        message,
        last_check: Utc::now().to_rfc3339(),
        response_time_ms,
    }
}

/// Check price cache health
fn check_price_cache_health(price_cache: &Arc<crate::price_cache::PriceCache>) -> HealthCheck {
    let start = std::time::Instant::now();
    let stats = price_cache.stats();
    let response_time_ms = start.elapsed().as_secs_f64() * 1000.0;

    let (status, message) = if stats.total_entries > 0 {
        (
            CheckStatus::Passing,
            Some(format!(
                "Price cache healthy: {} entries, {} tracked tokens",
                stats.total_entries, stats.tracked_tokens
            )),
        )
    } else {
        (
            CheckStatus::Warning,
            Some("Price cache empty - no tokens tracked".to_string()),
        )
    };

    HealthCheck {
        name: "price_cache".to_string(),
        status,
        message,
        last_check: Utc::now().to_rfc3339(),
        response_time_ms,
    }
}
