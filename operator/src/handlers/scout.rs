//! Scout API handlers
//!
//! Provides endpoints for Scout intelligence data:
//! - Scout status and run information
//! - WQS score distribution
//! - Scout metrics and statistics
//! - Manual Scout run triggering

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db_abstraction::{Database, DbPool};
use crate::error::{AppError, AppResult};

// Import ApiState for shared state
use crate::handlers::ApiState;

// =============================================================================
// RESPONSE TYPES
// =============================================================================

/// Scout status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutStatusResponse {
    /// Last run timestamp; None when no analysis history exists.
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub wallets_analyzed: i64,
    pub analysis_duration_seconds: f64,
    pub status: String, // "running" | "completed" | "failed" | "idle"
    pub wqs_distribution: Vec<WQSBucket>,
    pub promotion_queue: Vec<PromotionItem>,
    pub rejection_queue: Vec<RejectionItem>,
}

/// WQS score distribution bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WQSBucket {
    pub range: String,
    pub count: i64,
    pub percentage: f64,
}

/// Promotion queue item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionItem {
    pub address: String,
    pub wqs_score: f64,
    pub reason: String,
    pub backtest_success: bool,
    pub validated_at: DateTime<Utc>,
}

/// Rejection queue item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectionItem {
    pub address: String,
    pub wqs_score: f64,
    pub reason: String,
    pub rejected_at: DateTime<Utc>,
}

/// WQS distribution response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WQSDistributionResponse {
    pub distribution: Vec<WQSBucket>,
    pub average_score: f64,
    pub median_score: f64,
    pub total_wallets: i64,
}

/// Scout metrics response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutMetricsResponse {
    pub total_analyzed: i64,
    pub rug_check_rejections: i64,
    pub backtest_success_rate: f64,
    pub validation_pass_rate: f64,
    pub avg_analysis_time_seconds: f64,
    pub liquidity_validation_rate: f64,
}

/// Scout run trigger response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutRunResponse {
    pub run_id: String,
    pub scheduled_at: String,
}

// =============================================================================
// INTEGRATION FEATURE RESPONSE TYPES
// =============================================================================

/// Budget status and forecasting response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatusResponse {
    pub credits_used: i64,
    pub credits_remaining: i64,
    pub total_monthly_credits: i64,
    pub daily_target: i64,
    pub usage_percentage: f64,
    pub daily_usage_percentage: f64,
    pub alert_level: String,
    pub forecast_24h: BudgetForecast,
    pub optimization_suggestions: Vec<OptimizationSuggestion>,
}

/// Budget forecast
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetForecast {
    pub horizon_hours: i32,
    pub projected_usage: i64,
    pub projected_remaining: i64,
    pub confidence: f64,
    pub trend: String,
    pub recommendations: Vec<String>,
}

/// Optimization suggestion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationSuggestion {
    pub action_type: String,
    pub description: String,
    pub expected_savings: i64,
    pub priority: String,
}

/// Cache statistics response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatsResponse {
    pub hit_rate: f64,
    pub miss_rate: f64,
    pub total_hits: i64,
    pub total_misses: i64,
    pub total_entries: i64,
    pub max_size: i64,
    pub activity_distribution: ActivityDistribution,
    pub cache_efficiency: f64,
}

/// Activity distribution for cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityDistribution {
    pub very_high: i64,
    pub high: i64,
    pub medium: i64,
    pub low: i64,
    pub inactive: i64,
}

/// Conviction allocation response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvictionAllocationResponse {
    pub total_wallets_analyzed: i64,
    pub high_conviction_count: i64,
    pub budget_remaining: BudgetBreakdown,
    pub wallets_analyzed: WalletAnalysisBreakdown,
    pub allocation_summary: AllocationSummary,
}

/// Budget breakdown by conviction level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetBreakdown {
    pub high_conviction: i64,
    pub emerging: i64,
    pub reserve: i64,
}

/// Wallet analysis breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAnalysisBreakdown {
    pub very_high: WalletLevelStats,
    pub high: WalletLevelStats,
    pub medium: WalletLevelStats,
    pub emerging: WalletLevelStats,
    pub low: WalletLevelStats,
}

/// Statistics for a conviction level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletLevelStats {
    pub count: i64,
    pub credits_used: i64,
    pub average_wqs: f64,
    pub roi_score: f64,
}

/// Overall allocation summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationSummary {
    pub total_credits_allocated: i64,
    pub high_conviction_percentage: f64,
    pub emerging_percentage: f64,
    pub average_credits_per_wallet: f64,
}

// =============================================================================
// QUERY PARAMETERS
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ScoutTimeRangeQuery {
    pub range: Option<String>,
}

// =============================================================================
// HANDLERS
// =============================================================================

/// Get Scout status and queue information
pub async fn get_scout_status(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<ScoutStatusResponse>, AppError> {
    // Get wallet statistics from database
    let wallet_stats = get_wallet_statistics(&state.db).await?;

    // Calculate WQS distribution
    let wqs_distribution = calculate_wqs_distribution(&state.db).await?;

    // Get promotion queue (ACTIVE wallets with notes indicating recent promotion)
    let promotion_queue = get_promotion_queue(&state.db).await?;

    // Get rejection queue (REJECTED wallets with recent notes)
    let rejection_queue = get_rejection_queue(&state.db).await?;

    // Scout process state is not tracked by the operator; report "idle" unless
    // analysis history exists (a run has at least produced data). "completed"
    // means "a run completed at least once", never a live process claim.
    let status = if wallet_stats.last_analysis_time.is_some() {
        "completed".to_string()
    } else {
        "idle".to_string()
    };

    let response = ScoutStatusResponse {
        last_run_at: wallet_stats.last_analysis_time,
        next_run_at: None, // Would be calculated from cron schedule
        wallets_analyzed: wallet_stats.total_wallets,
        analysis_duration_seconds: wallet_stats.avg_analysis_time,
        status,
        wqs_distribution,
        promotion_queue,
        rejection_queue,
    };

    Ok(Json(response))
}

/// Get WQS score distribution
pub async fn get_wqs_distribution(
    State(state): State<Arc<ApiState>>,
    Query(_params): Query<ScoutTimeRangeQuery>,
) -> Result<Json<WQSDistributionResponse>, AppError> {
    let distribution = calculate_wqs_distribution(&state.db).await?;

    // Calculate average and median scores
    let stats = calculate_wqs_statistics(&state.db).await?;

    let response = WQSDistributionResponse {
        distribution,
        average_score: stats.average,
        median_score: stats.median,
        total_wallets: stats.total_count,
    };

    Ok(Json(response))
}

/// Get Scout metrics and performance statistics
pub async fn get_scout_metrics(
    State(state): State<Arc<ApiState>>,
    Query(_params): Query<ScoutTimeRangeQuery>,
) -> Result<Json<ScoutMetricsResponse>, AppError> {
    let metrics = calculate_scout_metrics(&state.db).await?;

    Ok(Json(metrics))
}

/// Trigger a manual Scout run
pub async fn trigger_scout_run(
    State(_state): State<Arc<ApiState>>,
) -> Result<Json<ScoutRunResponse>, AppError> {
    // Real scheduling (enqueue a run request and signal the Scout process) is
    // not implemented. Returning a fabricated run_id would make callers believe
    // a run was triggered when nothing happened.
    Err(AppError::ServiceUnavailable(
        "Scout run triggering is not implemented — no run was scheduled".to_string(),
    ))
}

// =============================================================================
// INTEGRATION FEATURE HANDLERS
// =============================================================================

/// Get PredictiveBudgetManager status and forecasting
pub async fn get_budget_status(
    State(_state): State<Arc<ApiState>>,
) -> Result<Json<BudgetStatusResponse>, AppError> {
    // Simulated budget figures are not wired to a real budget manager —
    // returning plausible-but-fake data as live production numbers would be
    // worse than an explicit not-implemented error.
    Err(AppError::ServiceUnavailable(
        "Budget status is not implemented — no budget manager is wired to this endpoint"
            .to_string(),
    ))
}

/// Get ActivityBasedCache statistics
pub async fn get_cache_stats(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<CacheStatsResponse>, AppError> {
    let pool = pg_pool(&state.db)?;

    // Activity buckets are mutually exclusive (single conditional-aggregation
    // query). Previously `very_high` was a strict subset of `high` and
    // `inactive` overlapped `low`/`medium`, double-counting totals.
    let (very_high, high, medium, low, inactive): (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status = 'ACTIVE' AND updated_at > NOW() - INTERVAL '1 hour') AS very_high,
            COUNT(*) FILTER (WHERE status = 'ACTIVE' AND updated_at <= NOW() - INTERVAL '1 hour' AND updated_at > NOW() - INTERVAL '24 hours') AS high,
            COUNT(*) FILTER (WHERE status = 'CANDIDATE' AND updated_at > NOW() - INTERVAL '7 days') AS medium,
            COUNT(*) FILTER (WHERE status = 'CANDIDATE' AND updated_at <= NOW() - INTERVAL '7 days' AND updated_at > NOW() - INTERVAL '30 days') AS low,
            COUNT(*) FILTER (WHERE status = 'REJECTED' OR updated_at <= NOW() - INTERVAL '30 days') AS inactive
        FROM wallets
        "#,
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let total_entries = very_high + high + medium + low + inactive;

    // Simulated cache metrics (in production, get from Scout's cache manager)
    let total_hits = very_high.saturating_mul(10).saturating_add(high.saturating_mul(5));
    let total_misses = medium.saturating_add(low);
    let max_size = 10000;

    let hit_rate = if total_hits + total_misses > 0 {
        (total_hits as f64 / (total_hits + total_misses) as f64) * 100.0
    } else {
        0.0
    };

    let miss_rate = 100.0 - hit_rate;

    let cache_efficiency = if total_entries > 0 {
        hit_rate * (total_entries as f64 / max_size as f64)
    } else {
        0.0
    };

    let response = CacheStatsResponse {
        hit_rate,
        miss_rate,
        total_hits,
        total_misses,
        total_entries,
        max_size,
        activity_distribution: ActivityDistribution {
            very_high,
            high,
            medium,
            low,
            inactive,
        },
        cache_efficiency,
    };

    Ok(Json(response))
}

/// Get HighConvictionAllocator status and allocation
pub async fn get_conviction_allocation(
    State(_state): State<Arc<ApiState>>,
) -> Result<Json<ConvictionAllocationResponse>, AppError> {
    // The budget split, ROI scores and credit multipliers are hard-coded
    // simulations — there is no real allocator wired to this endpoint.
    // Return an explicit not-implemented error instead of fabricated numbers.
    Err(AppError::ServiceUnavailable(
        "Conviction allocation is not implemented — no allocator is wired to this endpoint"
            .to_string(),
    ))
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

fn pg_pool(db: &Arc<dyn Database>) -> AppResult<sqlx::Pool<sqlx::Postgres>> {
    match db.pool() {
        DbPool::PostgreSQL(p) => Ok(p),
    }
}

struct WalletStatistics {
    total_wallets: i64,
    last_analysis_time: Option<String>,
    avg_analysis_time: f64,
}

async fn get_wallet_statistics(db: &Arc<dyn Database>) -> Result<WalletStatistics, AppError> {
    let pool = pg_pool(db)?;

    let total_wallets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Get last update time from the most recently updated wallet
    let last_time: Option<String> =
        sqlx::query_scalar("SELECT MAX(updated_at)::TEXT FROM wallets WHERE updated_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    Ok(WalletStatistics {
        total_wallets,
        last_analysis_time: last_time,
        avg_analysis_time: 0.0, // Would be calculated from actual run times
    })
}

async fn calculate_wqs_distribution(db: &Arc<dyn Database>) -> Result<Vec<WQSBucket>, AppError> {
    let pool = pg_pool(db)?;

    // Compute all buckets in a single GROUP BY query (no N+1 round trips).
    // The final bucket is inclusive of 100 so no score is dropped.
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT
            CASE
                WHEN wqs_score < 20 THEN '0-20'
                WHEN wqs_score < 40 THEN '20-40'
                WHEN wqs_score < 60 THEN '40-60'
                WHEN wqs_score < 80 THEN '60-80'
                ELSE '80-100'
            END AS bucket,
            COUNT(*) AS count
        FROM wallets
        WHERE wqs_score IS NOT NULL
        GROUP BY bucket
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let total_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE wqs_score IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    let mut counts: std::collections::HashMap<String, i64> =
        rows.into_iter().collect();
    let distribution = vec!["0-20", "20-40", "40-60", "60-80", "80-100"]
        .into_iter()
        .map(|range_name| {
            let count = counts.remove(range_name).unwrap_or(0);
            let percentage = if total_count > 0 {
                (count as f64 / total_count as f64) * 100.0
            } else {
                0.0
            };
            WQSBucket {
                range: range_name.to_string(),
                count,
                percentage,
            }
        })
        .collect();

    Ok(distribution)
}

struct WQSStatistics {
    average: f64,
    median: f64,
    total_count: i64,
}

async fn calculate_wqs_statistics(db: &Arc<dyn Database>) -> Result<WQSStatistics, AppError> {
    let pool = pg_pool(db)?;

    let total_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE wqs_score IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    if total_count == 0 {
        return Ok(WQSStatistics {
            average: 0.0,
            median: 0.0,
            total_count: 0,
        });
    }

    // Calculate average
    let avg: Option<f64> =
        sqlx::query_scalar("SELECT AVG(wqs_score) FROM wallets WHERE wqs_score IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    // Calculate median using OFFSET
    let median = if total_count % 2 == 0 {
        // Even number of rows - average of two middle values
        let mid1: f64 = sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET $1"
        )
        .bind(total_count / 2 - 1)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?;

        let mid2: f64 = sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET $1"
        )
        .bind(total_count / 2)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?;

        (mid1 + mid2) / 2.0
    } else {
        // Odd number of rows - middle value
        sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET $1"
        )
        .bind(total_count / 2)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?
    };

    Ok(WQSStatistics {
        average: avg.unwrap_or(0.0),
        median,
        total_count,
    })
}

async fn calculate_scout_metrics(db: &Arc<dyn Database>) -> Result<ScoutMetricsResponse, AppError> {
    let pool = pg_pool(db)?;

    let total_analyzed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Get rejected wallets (rug check equivalent)
    let rug_check_rejections: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE status = 'REJECTED'")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    // Calculate backtest success rate (from wallets that actually have a
    // backtest result — REJECTED/CANDIDATE wallets were never backtested and
    // must not dilute the denominator).
    let backtest_passed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND notes LIKE '%Backtest: PASSED%'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let backtest_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE notes LIKE '%Backtest:%'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let backtest_success_rate = if backtest_total > 0 {
        (backtest_passed as f64 / backtest_total as f64) * 100.0
    } else {
        0.0
    };

    // Validation pass rate: ACTIVE wallets over the population that underwent
    // validation (ACTIVE + CANDIDATE), not over REJECTED wallets too.
    let validation_passed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE'")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    let validation_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let validation_pass_rate = if validation_total > 0 {
        (validation_passed as f64 / validation_total as f64) * 100.0
    } else {
        0.0
    };

    Ok(ScoutMetricsResponse {
        total_analyzed,
        rug_check_rejections,
        backtest_success_rate,
        validation_pass_rate,
        avg_analysis_time_seconds: 0.0,
        liquidity_validation_rate: 0.0,
    })
}

async fn get_promotion_queue(db: &Arc<dyn Database>) -> Result<Vec<PromotionItem>, AppError> {
    let pool = pg_pool(db)?;

    let rows = sqlx::query_as::<_, (String, f64, String, DateTime<Utc>)>(
        "SELECT address, COALESCE(wqs_score, 0.0), COALESCE(notes, ''), promoted_at FROM wallets
         WHERE status = 'ACTIVE' AND promoted_at IS NOT NULL
         ORDER BY promoted_at DESC LIMIT 20",
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let items = rows
        .into_iter()
        .map(|(address, wqs_score, notes, promoted_at)| {
            let backtest_success = notes.contains("Backtest: PASSED");

            PromotionItem {
                address,
                wqs_score,
                reason: notes,
                backtest_success,
                validated_at: promoted_at,
            }
        })
        .collect();

    Ok(items)
}

async fn get_rejection_queue(db: &Arc<dyn Database>) -> Result<Vec<RejectionItem>, AppError> {
    let pool = pg_pool(db)?;

    let rows = sqlx::query_as::<_, (String, f64, String, DateTime<Utc>)>(
        "SELECT address, COALESCE(wqs_score, 0.0), COALESCE(notes, ''), updated_at FROM wallets
         WHERE status = 'REJECTED'
         ORDER BY updated_at DESC LIMIT 20",
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let items = rows
        .into_iter()
        .map(|(address, wqs_score, notes, updated_at)| RejectionItem {
            address,
            wqs_score,
            reason: notes,
            rejected_at: updated_at,
        })
        .collect();

    Ok(items)
}
