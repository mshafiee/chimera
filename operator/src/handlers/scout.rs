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

use crate::db::DbPool;
use crate::error::AppError;

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
// QUERY PARAMETERS
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TimeRangeQuery {
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
        last_run_at: wallet_stats.last_analysis_time.unwrap_or_else(|| {
            chrono::Utc::now().to_rfc3339()
        }),
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
    Query(_params): Query<TimeRangeQuery>,
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
    Query(_params): Query<TimeRangeQuery>,
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
// HELPER FUNCTIONS
// =============================================================================

struct WalletStatistics {
    total_wallets: i64,
    last_analysis_time: Option<String>,
    avg_analysis_time: f64,
}

async fn get_wallet_statistics(db: &DbPool) -> Result<WalletStatistics, AppError> {
    let total_wallets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    // Get last update time from the most recently updated wallet
    let last_time: Option<String> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM wallets WHERE updated_at IS NOT NULL"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    Ok(WalletStatistics {
        total_wallets,
        last_analysis_time: last_time,
        avg_analysis_time: 0.0, // Would be calculated from actual run times
    })
}

async fn calculate_wqs_distribution(db: &DbPool) -> Result<Vec<WQSBucket>, AppError> {
    let ranges = vec![
        ("0-20", 0.0, 20.0),
        ("20-40", 20.0, 40.0),
        ("40-60", 40.0, 60.0),
        ("60-80", 60.0, 80.0),
        ("80-100", 80.0, 100.0),
    ];

    let mut distribution = Vec::new();
    let total_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score IS NOT NULL"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    for (range_name, min, max) in ranges {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wallets WHERE wqs_score >= ? AND wqs_score < ?"
        )
        .bind(min)
        .bind(max)
        .fetch_one(db)
        .await
        .map_err(|e| AppError::Database(e))?;

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

async fn calculate_wqs_statistics(db: &DbPool) -> Result<WQSStatistics, AppError> {
    let total_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE wqs_score IS NOT NULL"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    if total_count == 0 {
        return Ok(WQSStatistics {
            average: 0.0,
            median: 0.0,
            total_count: 0,
        });
    }

    // Calculate average
    let avg: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(wqs_score) FROM wallets WHERE wqs_score IS NOT NULL"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    // Calculate median using OFFSET
    let median = if total_count % 2 == 0 {
        // Even number of rows - average of two middle values
        let mid1: f64 = sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
        )
        .bind(total_count / 2 - 1)
        .fetch_one(db)
        .await
        .map_err(|e| AppError::Database(e))?;

        let mid2: f64 = sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
        )
        .bind(total_count / 2)
        .fetch_one(db)
        .await
        .map_err(|e| AppError::Database(e))?;

        (mid1 + mid2) / 2.0
    } else {
        // Odd number of rows - middle value
        sqlx::query_scalar(
            "SELECT wqs_score FROM wallets WHERE wqs_score IS NOT NULL ORDER BY wqs_score LIMIT 1 OFFSET ?"
        )
        .bind(total_count / 2)
        .fetch_one(db)
        .await
        .map_err(|e| AppError::Database(e))?
    };

    Ok(WQSStatistics {
        average: avg.unwrap_or(0.0),
        median,
        total_count,
    })
}

async fn calculate_scout_metrics(db: &DbPool) -> Result<ScoutMetricsResponse, AppError> {
    let total_analyzed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    // Get rejected wallets (rug check equivalent)
    let rug_check_rejections: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'REJECTED'"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    // Calculate backtest success rate (from ACTIVE wallets that passed validation)
    let backtest_passed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE' AND notes LIKE '%Backtest: PASSED%'"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    let backtest_success_rate = if total_analyzed > 0 {
        (backtest_passed as f64 / total_analyzed as f64) * 100.0
    } else {
        0.0
    };

    // Validation pass rate (wallets that met promotion criteria)
    let validation_passed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE'"
    )
    .fetch_one(db)
    .await
    .map_err(|e| AppError::Database(e))?;

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
        avg_analysis_time_seconds: 0.0, // Would be calculated from actual run data
        liquidity_validation_rate: 0.0, // Would be calculated from validation data
    })
}

async fn get_promotion_queue(db: &DbPool) -> Result<Vec<PromotionItem>, AppError> {
    let rows = sqlx::query_as::<_, (String, f64, String, String)>(
        "SELECT address, wqs_score, notes, promoted_at FROM wallets
         WHERE status = 'ACTIVE' AND promoted_at IS NOT NULL
         ORDER BY promoted_at DESC LIMIT 20"
    )
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    let items = rows.into_iter().map(|(address, wqs_score, notes, promoted_at)| {
        // Determine backtest success from notes
        let backtest_success = notes.contains("Backtest: PASSED");

        PromotionItem {
            address,
            wqs_score,
            reason: notes,
            backtest_success,
            validated_at: promoted_at,
        }
    }).collect();

    Ok(items)
}

async fn get_rejection_queue(db: &DbPool) -> Result<Vec<RejectionItem>, AppError> {
    let rows = sqlx::query_as::<_, (String, f64, String, String)>(
        "SELECT address, wqs_score, notes, updated_at FROM wallets
         WHERE status = 'REJECTED'
         ORDER BY updated_at DESC LIMIT 20"
    )
    .fetch_all(db)
    .await
    .map_err(|e| AppError::Database(e))?;

    let items = rows.into_iter().map(|(address, wqs_score, notes, updated_at)| {
        RejectionItem {
            address,
            wqs_score,
            reason: notes,
            rejected_at: updated_at,
        }
    }).collect();

    Ok(items)
}