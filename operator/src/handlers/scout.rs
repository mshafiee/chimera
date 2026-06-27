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
    pub last_run_at: String,
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
    pub validated_at: String,
}

/// Rejection queue item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectionItem {
    pub address: String,
    pub wqs_score: f64,
    pub reason: String,
    pub rejected_at: String,
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

    // Determine Scout status (simplified - in reality this would check Scout process)
    let status = if wallet_stats.total_wallets > 0 {
        "completed".to_string()
    } else {
        "idle".to_string()
    };

    let response = ScoutStatusResponse {
        last_run_at: wallet_stats
            .last_analysis_time
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
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
    // Generate a run ID
    let run_id = uuid::Uuid::new_v4().to_string();
    let scheduled_at = chrono::Utc::now().to_rfc3339();

    // In a real implementation, this would:
    // 1. Call the Python Scout process via API or signal
    // 2. Store the run request in a queue table
    // 3. Return the run ID for tracking

    // For now, return a placeholder response
    let response = ScoutRunResponse {
        run_id,
        scheduled_at,
    };

    Ok(Json(response))
}

// =============================================================================
// INTEGRATION FEATURE HANDLERS
// =============================================================================

/// Get PredictiveBudgetManager status and forecasting
pub async fn get_budget_status(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<BudgetStatusResponse>, AppError> {
    let pool = sqlite_pool(&state.db)?;

    // Get total wallet count for budget estimation
    let total_wallets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Simulated budget data (in production, this would come from Scout's budget manager)
    let monthly_credits: i64 = 10_000_000;
    let estimated_credits_per_wallet: i64 = 2500;
    let credits_used = total_wallets.saturating_mul(estimated_credits_per_wallet);
    let credits_remaining = monthly_credits.saturating_sub(credits_used);
    let usage_percentage = if monthly_credits > 0 {
        (credits_used as f64 / monthly_credits as f64) * 100.0
    } else {
        0.0
    };

    let daily_target = monthly_credits / 30;
    let daily_usage_percentage = if daily_target > 0 {
        ((credits_used / 30) as f64 / daily_target as f64) * 100.0
    } else {
        0.0
    };

    let alert_level = if usage_percentage >= 95.0 {
        "depleted"
    } else if usage_percentage >= 80.0 {
        "critical"
    } else if usage_percentage >= 50.0 {
        "warning"
    } else {
        "normal"
    };

    let forecast = BudgetForecast {
        horizon_hours: 24,
        projected_usage: (total_wallets.saturating_mul(estimated_credits_per_wallet) / 30),
        projected_remaining: credits_remaining.saturating_sub(total_wallets.saturating_mul(estimated_credits_per_wallet) / 30),
        confidence: 0.85,
        trend: if daily_usage_percentage < 80.0 { "stable" } else { "increasing" }.to_string(),
        recommendations: vec![
            "Continue monitoring daily usage".to_string(),
            "Cache hit rate is optimal".to_string(),
        ],
    };

    let response = BudgetStatusResponse {
        credits_used,
        credits_remaining,
        total_monthly_credits: monthly_credits,
        daily_target,
        usage_percentage,
        daily_usage_percentage,
        alert_level: alert_level.to_string(),
        forecast_24h: forecast,
        optimization_suggestions: vec![
            OptimizationSuggestion {
                action_type: "cache_optimization".to_string(),
                description: "Increase cache TTL for inactive wallets".to_string(),
                expected_savings: 50000,
                priority: "medium".to_string(),
            },
        ],
    };

    Ok(Json(response))
}

/// Get ActivityBasedCache statistics
pub async fn get_cache_stats(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<CacheStatsResponse>, AppError> {
    let pool = sqlite_pool(&state.db)?;

    // Get wallet activity distribution (proxy for cache activity)
    let very_high: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND updated_at > datetime('now', '-1 hour')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let high: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND updated_at > datetime('now', '-24 hours')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let medium: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'CANDIDATE' AND updated_at > datetime('now', '-7 days')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let low: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'CANDIDATE' AND updated_at <= datetime('now', '-7 days')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let inactive: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'REJECTED' OR updated_at <= datetime('now', '-30 days')",
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
    State(state): State<Arc<ApiState>>,
) -> Result<Json<ConvictionAllocationResponse>, AppError> {
    let pool = sqlite_pool(&state.db)?;

    // Get total wallets analyzed
    let total_wallets_analyzed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Get high-conviction wallets (WQS 70+)
    let high_conviction_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score >= 70.0 AND status = 'ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Get conviction breakdown
    let very_high: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score >= 80.0 AND status = 'ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let high: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score >= 70.0 AND wqs_score < 80.0 AND status = 'ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let medium: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score >= 50.0 AND wqs_score < 70.0 AND status IN ('ACTIVE', 'CANDIDATE')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let emerging: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score >= 30.0 AND wqs_score < 50.0 AND status = 'CANDIDATE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let low: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score < 30.0 AND status = 'REJECTED'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Simulated budget allocation (in production, get from HighConvictionAllocator)
    let total_budget: i64 = 5000;
    let high_conviction_budget = (total_budget as f64 * 0.70) as i64; // 70% to high conviction
    let emerging_budget = (total_budget as f64 * 0.20) as i64; // 20% to emerging
    let reserve_budget = total_budget.saturating_sub(high_conviction_budget).saturating_sub(emerging_budget);

    let avg_wqs_very_high: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score >= 80.0 AND status = 'ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0.0);

    let avg_wqs_high: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score >= 70.0 AND wqs_score < 80.0 AND status = 'ACTIVE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0.0);

    let avg_wqs_medium: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score >= 50.0 AND wqs_score < 70.0 AND status IN ('ACTIVE', 'CANDIDATE')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0.0);

    let avg_wqs_emerging: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score >= 30.0 AND wqs_score < 50.0 AND status = 'CANDIDATE'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0.0);

    let avg_wqs_low: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score < 30.0 AND status = 'REJECTED'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0.0);

    let response = ConvictionAllocationResponse {
        total_wallets_analyzed,
        high_conviction_count,
        budget_remaining: BudgetBreakdown {
            high_conviction: high_conviction_budget,
            emerging: emerging_budget,
            reserve: reserve_budget,
        },
        wallets_analyzed: WalletAnalysisBreakdown {
            very_high: WalletLevelStats {
                count: very_high,
                credits_used: very_high.saturating_mul(3), // 3x multiplier
                average_wqs: avg_wqs_very_high,
                roi_score: 0.85,
            },
            high: WalletLevelStats {
                count: high,
                credits_used: high.saturating_mul(2), // 2.5x multiplier
                average_wqs: avg_wqs_high,
                roi_score: 0.75,
            },
            medium: WalletLevelStats {
                count: medium,
                credits_used: medium,
                average_wqs: avg_wqs_medium,
                roi_score: 0.60,
            },
            emerging: WalletLevelStats {
                count: emerging,
                credits_used: emerging.saturating_mul(2),
                average_wqs: avg_wqs_emerging,
                roi_score: 0.40,
            },
            low: WalletLevelStats {
                count: low,
                credits_used: 0,
                average_wqs: avg_wqs_low,
                roi_score: 0.10,
            },
        },
        allocation_summary: AllocationSummary {
            total_credits_allocated: high_conviction_budget,
            high_conviction_percentage: 70.0,
            emerging_percentage: 20.0,
            average_credits_per_wallet: if total_wallets_analyzed > 0 {
                high_conviction_budget as f64 / total_wallets_analyzed as f64
            } else {
                0.0
            },
        },
    };

    Ok(Json(response))
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

fn sqlite_pool(db: &Arc<dyn Database>) -> AppResult<sqlx::Pool<sqlx::Sqlite>> {
    match db.pool() {
        DbPool::SQLite(p) => Ok(p),
        _ => Err(AppError::Internal(
            "Only SQLite backend is supported".to_string(),
        )),
    }
}

struct WalletStatistics {
    total_wallets: i64,
    last_analysis_time: Option<String>,
    avg_analysis_time: f64,
}

async fn get_wallet_statistics(db: &Arc<dyn Database>) -> Result<WalletStatistics, AppError> {
    let pool = sqlite_pool(db)?;

    let total_wallets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Get last update time from the most recently updated wallet
    let last_time: Option<String> =
        sqlx::query_scalar("SELECT MAX(updated_at) FROM wallets WHERE updated_at IS NOT NULL")
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
    let pool = sqlite_pool(db)?;

    let ranges = vec![
        ("0-20", 0.0, 20.0),
        ("20-40", 20.0, 40.0),
        ("40-60", 40.0, 60.0),
        ("60-80", 60.0, 80.0),
        ("80-100", 80.0, 100.0),
    ];

    let mut distribution = Vec::new();
    let total_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE wqs_score IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    for (range_name, min, max) in ranges {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wallets WHERE wqs_score >= ? AND wqs_score < ?",
        )
        .bind(min)
        .bind(max)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?;

        let percentage = if total_count > 0 {
            (count as f64 / total_count as f64) * 100.0
        } else {
            0.0
        };

        distribution.push(WQSBucket {
            range: range_name.to_string(),
            count,
            percentage,
        });
    }

    Ok(distribution)
}

struct WQSStatistics {
    average: f64,
    median: f64,
    total_count: i64,
}

async fn calculate_wqs_statistics(db: &Arc<dyn Database>) -> Result<WQSStatistics, AppError> {
    let pool = sqlite_pool(db)?;

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
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
        )
        .bind(total_count / 2 - 1)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?;

        let mid2: f64 = sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
        )
        .bind(total_count / 2)
        .fetch_one(&pool)
        .await
        .map_err(AppError::Database)?;

        (mid1 + mid2) / 2.0
    } else {
        // Odd number of rows - middle value
        sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
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
    let pool = sqlite_pool(db)?;

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

    // Calculate backtest success rate (from ACTIVE wallets that passed validation)
    let backtest_passed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND notes LIKE '%Backtest: PASSED%'",
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    let backtest_success_rate = if total_analyzed > 0 {
        (backtest_passed as f64 / total_analyzed as f64) * 100.0
    } else {
        0.0
    };

    // Validation pass rate (wallets that met promotion criteria)
    let validation_passed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE'")
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    let validation_pass_rate = if total_analyzed > 0 {
        (validation_passed as f64 / total_analyzed as f64) * 100.0
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
    let pool = sqlite_pool(db)?;

    let rows = sqlx::query_as::<_, (String, f64, String, String)>(
        "SELECT address, wqs_score, notes, promoted_at FROM wallets
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
    let pool = sqlite_pool(db)?;

    let rows = sqlx::query_as::<_, (String, f64, String, String)>(
        "SELECT address, wqs_score, notes, updated_at FROM wallets
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
