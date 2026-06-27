//! Risk Management API handlers
//!
//! Provides endpoints for portfolio risk analysis including:
//! - Portfolio heat, concentration, exposure, and drawdown metrics
//! - Stop loss activation metrics
//! - Profit target hit metrics
//! - Position size analysis

use axum::{
    extract::{Query, State},
    Json,
};
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db_abstraction::{Database, DbPool};
use crate::error::{AppError, AppResult};
use crate::handlers::api::ApiState;

// =============================================================================
// QUERY PARAMETERS
// =============================================================================

/// Time range query parameter for metrics
#[derive(Debug, Deserialize)]
pub struct TimeRangeQuery {
    /// Number of days to look back (default: 30)
    #[serde(default = "default_days")]
    days: u32,
}

fn default_days() -> u32 {
    30
}

// =============================================================================
// PORTFOLIO RISK RESPONSE TYPES
// =============================================================================

/// Portfolio risk response
#[derive(Debug, Serialize)]
pub struct PortfolioRiskResponse {
    pub portfolio_heat_percent: f64,
    pub heat_threshold: f64,
    pub heat_status: String, // 'normal' | 'elevated' | 'high' | 'critical'
    pub concentration: ConcentrationData,
    pub exposure: ExposureData,
    pub drawdown: DrawdownData,
    pub total_capital_sol: f64, // Current wallet balance
}

/// Concentration data (by token and sector)
#[derive(Debug, Serialize)]
pub struct ConcentrationData {
    pub by_token: Vec<TokenConcentration>,
    pub by_sector: Vec<SectorConcentration>,
    pub max_concentration_percent: f64,
    pub hhi: f64, // Herfindahl-Hirschman Index
}

/// Token concentration breakdown
#[derive(Debug, Serialize)]
pub struct TokenConcentration {
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub position_count: i64,
    pub total_value_sol: f64,
    pub percentage: f64,
}

/// Sector concentration breakdown
#[derive(Debug, Serialize)]
pub struct SectorConcentration {
    pub sector: String,
    pub position_count: i64,
    pub total_value_sol: f64,
    pub percentage: f64,
}

/// Exposure data
#[derive(Debug, Serialize)]
pub struct ExposureData {
    pub total_exposure_sol: f64,
    pub long_exposure_sol: f64,
    pub short_exposure_sol: f64,
    pub net_exposure_sol: f64,
    pub max_drawdown_percent: f64,
    pub current_drawdown_percent: f64,
}

/// Drawdown data
#[derive(Debug, Serialize)]
pub struct DrawdownData {
    pub current_drawdown_percent: f64,
    pub max_drawdown_percent: f64,
    pub drawdown_duration_days: f64,
    pub recovery_percent: f64,
}

// =============================================================================
// STOP LOSS METRICS RESPONSE TYPES
// =============================================================================

/// Stop loss metrics response
#[derive(Debug, Serialize)]
pub struct StopLossMetricsResponse {
    pub activation_rate: f64,
    pub total_activations: i64,
    pub loss_prevented_sol: f64,
    pub average_loss_prevented_sol: f64,
    pub activations_by_strategy: Vec<StrategyStopLossData>,
    pub recent_activations: Vec<StopLossActivation>,
}

/// Stop loss data by strategy
#[derive(Debug, Serialize)]
pub struct StrategyStopLossData {
    #[serde(rename = "strategy")]
    pub strategy_name: String, // 'SHIELD' | 'SPEAR'
    pub activations: i64,
    pub loss_prevented_sol: f64,
}

/// Individual stop loss activation
#[derive(Debug, Serialize)]
pub struct StopLossActivation {
    pub timestamp: String,
    pub trade_uuid: String,
    pub token_symbol: Option<String>,
    pub entry_price: f64,
    pub stop_price: f64,
    pub loss_prevented_sol: f64,
    #[serde(rename = "strategy")]
    pub strategy_name: String, // 'SHIELD' | 'SPEAR'
}

// =============================================================================
// PROFIT TARGET METRICS RESPONSE TYPES
// =============================================================================

/// Profit target metrics response
#[derive(Debug, Serialize)]
pub struct ProfitTargetMetricsResponse {
    pub hit_rate: f64,
    pub total_hits: i64,
    pub total_targets: i64,
    pub trailing_stop_activations: i64,
    pub average_realized_gain_sol: f64,
    pub targets_by_strategy: Vec<StrategyProfitTargetData>,
    pub recent_hits: Vec<ProfitTargetHit>,
}

/// Profit target data by strategy
#[derive(Debug, Serialize)]
pub struct StrategyProfitTargetData {
    #[serde(rename = "strategy")]
    pub strategy_name: String, // 'SHIELD' | 'SPEAR'
    pub hit_rate: f64,
    pub total_hits: i64,
    pub average_gain_sol: f64,
}

/// Individual profit target hit
#[derive(Debug, Serialize)]
pub struct ProfitTargetHit {
    pub timestamp: String,
    pub trade_uuid: String,
    pub token_symbol: Option<String>,
    pub target_level: i32,
    pub realized_gain_sol: f64,
    #[serde(rename = "strategy")]
    pub strategy_name: String, // 'SHIELD' | 'SPEAR'
}

// =============================================================================
// POSITION SIZE ANALYSIS RESPONSE TYPES
// =============================================================================

/// Position size analysis response
#[derive(Debug, Serialize)]
pub struct PositionSizeAnalysisResponse {
    pub average_position_sol: f64,
    pub median_position_sol: f64,
    pub max_position_sol: f64,
    pub min_position_sol: f64,
    pub position_size_distribution: Vec<SizeBucket>,
    pub kelly_criterion_usage: f64,
}

/// Size bucket for distribution
#[derive(Debug, Serialize)]
pub struct SizeBucket {
    #[serde(rename = "range")]
    pub size_range: String,
    pub count: i64,
    pub percentage: f64,
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Classify token into sector based on address hash (MVP approach)
///
/// This is a simple hash-based classification for the MVP.
/// Future enhancement: Use token metadata API for accurate classification.
fn classify_token_sector(token_address: &str) -> &'static str {
    let first_char = token_address.chars().next().unwrap_or('0');
    match first_char {
        '0'..='3' => "DeFi",
        '4'..='6' => "NFT/Gaming",
        '7'..='9' => "Meme",
        'a'..='c' => "Stablecoin",
        'd'..='f' => "Exchange",
        'g'..='z' => "Other",
        _ => "Unknown",
    }
}

/// Calculate Herfindahl-Hirschman Index (HHI)
///
/// HHI = sum of squared market shares (as percentages, 0-10000)
/// - HHI < 1500: Competitive
/// - HHI 1500-2500: Moderately concentrated
/// - HHI > 2500: Highly concentrated
fn calculate_hhi(concentrations: &[TokenConcentration]) -> f64 {
    concentrations
        .iter()
        .map(|c| {
            let share = c.percentage / 100.0;
            share * share
        })
        .sum::<f64>()
        * 10000.0
}

/// Determine heat status based on exposure vs threshold
fn determine_heat_status(exposure: f64, threshold: f64) -> &'static str {
    let ratio = exposure / threshold.max(0.01);
    match ratio {
        r if r < 0.7 => "normal",
        r if r < 0.9 => "elevated",
        r if r < 1.1 => "high",
        _ => "critical",
    }
}

// =============================================================================
// DATABASE QUERY FUNCTIONS
// =============================================================================

fn sqlite_pool(db: &Arc<dyn Database>) -> AppResult<sqlx::Pool<sqlx::Sqlite>> {
    match db.pool() {
        DbPool::SQLite(p) => Ok(p),
        _ => Err(AppError::Internal(
            "Only SQLite backend supported".to_string(),
        )),
    }
}

/// Get position concentrations grouped by token
async fn get_position_concentrations(
    db: &Arc<dyn Database>,
) -> AppResult<(Vec<TokenConcentration>, Vec<SectorConcentration>, f64)> {
    let pool = sqlite_pool(db)?;
    let rows = sqlx::query_as::<_, (String, Option<String>, i64, f64)>(
        r#"
        SELECT token_address, token_symbol, COUNT(*) as position_count,
               SUM(entry_amount_sol) as total_value_sol
        FROM positions
        WHERE state IN ('ACTIVE', 'EXITING')
        GROUP BY token_address, token_symbol
        ORDER BY total_value_sol DESC
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let total_exposure: f64 = rows.iter().map(|r| r.3).sum();

    let by_token: Vec<TokenConcentration> = rows
        .iter()
        .map(|(addr, symbol, count, value)| TokenConcentration {
            token_address: addr.clone(),
            token_symbol: symbol.clone(),
            position_count: *count,
            total_value_sol: *value,
            percentage: if total_exposure > 0.0 {
                (*value / total_exposure) * 100.0
            } else {
                0.0
            },
        })
        .collect();

    // Calculate sector concentrations
    let mut sector_map: HashMap<&str, f64> = HashMap::new();
    let mut sector_counts: HashMap<&str, i64> = HashMap::new();

    for (addr, _symbol, count, value) in &rows {
        let sector = classify_token_sector(addr);
        *sector_map.entry(sector).or_insert(0.0) += value;
        *sector_counts.entry(sector).or_insert(0) += count;
    }

    let by_sector: Vec<SectorConcentration> = sector_map
        .into_iter()
        .map(|(sector, value)| SectorConcentration {
            sector: sector.to_string(),
            position_count: sector_counts.get(sector).copied().unwrap_or(0),
            total_value_sol: value,
            percentage: if total_exposure > 0.0 {
                (value / total_exposure) * 100.0
            } else {
                0.0
            },
        })
        .collect();

    let max_concentration = by_token
        .iter()
        .map(|c| c.percentage)
        .fold(0.0_f64, f64::max);

    Ok((by_token, by_sector, max_concentration))
}

/// Get portfolio exposure data
async fn get_portfolio_exposure(db: &Arc<dyn Database>) -> AppResult<ExposureData> {
    let pool = sqlite_pool(db)?;
    let total: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(entry_amount_sol), 0.0)
        FROM positions
        WHERE state IN ('ACTIVE', 'EXITING')
        "#,
    )
    .fetch_one(&pool)
    .await?;

    let total_exposure_sol = total.0.unwrap_or(0.0);

    // For now, all positions are long (no short selling in this system)
    let long_exposure_sol = total_exposure_sol;
    let short_exposure_sol = 0.0;
    let net_exposure_sol = total_exposure_sol;

    Ok(ExposureData {
        total_exposure_sol,
        long_exposure_sol,
        short_exposure_sol,
        net_exposure_sol,
        max_drawdown_percent: 0.0,     // Will be filled in handler
        current_drawdown_percent: 0.0, // Will be filled in handler
    })
}

/// Get stop loss metrics (activations where exit was near stop price)
async fn get_stop_loss_metrics_db(
    db: &Arc<dyn Database>,
    days: u32,
) -> AppResult<StopLossMetricsResponse> {
    let pool = sqlite_pool(db)?;
    let days_str = format!("-{} days", days);

    // Get total activations and loss prevented
    let total_result: (Option<i64>, Option<f64>) = sqlx::query_as(
        r#"
        SELECT COUNT(*) as total_activations,
               COALESCE(SUM((et.stop_loss_price - p.exit_price) * p.entry_amount_sol / p.entry_price), 0.0) as loss_prevented_sol
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND p.exit_price IS NOT NULL
          AND p.entry_price > 0
          AND et.stop_loss_price IS NOT NULL
          AND p.exit_price <= et.stop_loss_price * 1.01
          AND p.closed_at >= datetime('now', ?)
        "#,
    )
    .bind(&days_str)
    .fetch_one(&pool)
    .await?;

    let total_activations = total_result.0.unwrap_or(0);
    let loss_prevented_sol = total_result.1.unwrap_or(0.0);
    let average_loss_prevented_sol = if total_activations > 0 {
        loss_prevented_sol / total_activations as f64
    } else {
        0.0
    };

    // Get activations by strategy
    let by_strategy_rows = sqlx::query_as::<_, (String, i64, Option<f64>)>(
        r#"
        SELECT p.strategy,
               COUNT(*) as activations,
               COALESCE(SUM((et.stop_loss_price - p.exit_price) * p.entry_amount_sol / p.entry_price), 0.0) as loss_prevented
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND p.exit_price IS NOT NULL
          AND p.entry_price > 0
          AND et.stop_loss_price IS NOT NULL
          AND p.exit_price <= et.stop_loss_price * 1.01
          AND p.closed_at >= datetime('now', ?)
        GROUP BY p.strategy
        "#,
    )
    .bind(&days_str)
    .fetch_all(&pool)
    .await?;

    let activations_by_strategy: Vec<StrategyStopLossData> = by_strategy_rows
        .iter()
        .map(|(strategy, activations, loss)| StrategyStopLossData {
            strategy_name: strategy.clone(),
            activations: *activations,
            loss_prevented_sol: loss.unwrap_or(0.0),
        })
        .collect();

    // Get recent activations (last 10)
    let recent_rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            f64,
            f64,
            f64,
            String,
        ),
    >(
        r#"
        SELECT p.trade_uuid, p.token_symbol, p.closed_at,
               p.entry_price, et.stop_loss_price, p.exit_price, p.strategy
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND p.exit_price IS NOT NULL
          AND p.entry_price > 0
          AND et.stop_loss_price IS NOT NULL
          AND p.exit_price <= et.stop_loss_price * 1.01
          AND p.closed_at >= datetime('now', ?)
        ORDER BY p.closed_at DESC
        LIMIT 10
        "#,
    )
    .bind(&days_str)
    .fetch_all(&pool)
    .await?;

    let recent_activations: Vec<StopLossActivation> = recent_rows
        .iter()
        .map(|(uuid, symbol, closed_at, entry, stop, exit, strategy)| {
            let loss_prevented = (stop - exit) * 0.01; // Approximate
            StopLossActivation {
                timestamp: closed_at.clone().unwrap_or_default(),
                trade_uuid: uuid.clone(),
                token_symbol: symbol.clone(),
                entry_price: *entry,
                stop_price: *stop,
                loss_prevented_sol: loss_prevented,
                strategy_name: strategy.clone(),
            }
        })
        .collect();

    // Calculate activation rate (activations per total closed positions in period)
    let total_closed: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM positions
        WHERE state = 'CLOSED'
          AND closed_at >= datetime('now', ?)
        "#,
    )
    .bind(&days_str)
    .fetch_one(&pool)
    .await?;

    let total_closed_count = total_closed.0.unwrap_or(0);
    let activation_rate = if total_closed_count > 0 {
        (total_activations as f64 / total_closed_count as f64) * 100.0
    } else {
        0.0
    };

    Ok(StopLossMetricsResponse {
        activation_rate,
        total_activations,
        loss_prevented_sol,
        average_loss_prevented_sol,
        activations_by_strategy,
        recent_activations,
    })
}

/// Get profit target metrics
async fn get_profit_target_metrics_db(
    db: &Arc<dyn Database>,
    days: u32,
) -> AppResult<ProfitTargetMetricsResponse> {
    let pool = sqlite_pool(db)?;
    let days_str = format!("-{} days", days);

    // Get total hits (positions with targets_hit > 0)
    let total_hits_result: (Option<i64>, Option<f64>) = sqlx::query_as(
        r#"
        SELECT COUNT(*) as total_hits,
               COALESCE(SUM(et.peak_profit_percent * p.entry_amount_sol / 100.0), 0.0) as total_gain_sol
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND et.targets_hit IS NOT NULL
          AND json_array_length(et.targets_hit) > 0
          AND p.closed_at >= datetime('now', ?)
        "#,
    )
    .bind(&days_str)
    .fetch_one(&pool)
    .await?;

    let total_hits = total_hits_result.0.unwrap_or(0);
    let total_gain_sol = total_hits_result.1.unwrap_or(0.0);
    let average_realized_gain_sol = if total_hits > 0 {
        total_gain_sol / total_hits as f64
    } else {
        0.0
    };

    // Count trailing stop activations (where trailing_stop_active = true)
    let trailing_result: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND et.trailing_stop_active = 1
          AND p.closed_at >= datetime('now', ?)
        "#,
    )
    .bind(&days_str)
    .fetch_one(&pool)
    .await?;

    let trailing_stop_activations = trailing_result.0.unwrap_or(0);

    // Get by strategy
    let by_strategy_rows = sqlx::query_as::<_, (String, i64, Option<f64>, Option<i64>)>(
        r#"
        SELECT p.strategy,
               COUNT(*) as hits,
               COALESCE(AVG(et.peak_profit_percent * p.entry_amount_sol / 100.0), 0.0) as avg_gain_sol,
               json_array_length(et.targets_hit) as targets_count
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND et.targets_hit IS NOT NULL
          AND json_array_length(et.targets_hit) > 0
          AND p.closed_at >= datetime('now', ?)
        GROUP BY p.strategy
        "#,
    )
    .bind(&days_str)
    .fetch_all(&pool)
    .await?;

    let targets_by_strategy: Vec<StrategyProfitTargetData> = by_strategy_rows
        .iter()
        .map(|(strategy, hits, avg_gain, _)| StrategyProfitTargetData {
            strategy_name: strategy.clone(),
            hit_rate: 100.0, // All rows are hits by definition
            total_hits: *hits,
            average_gain_sol: avg_gain.unwrap_or(0.0),
        })
        .collect();

    // Get recent hits
    let recent_rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            f64,
            String,
        ),
    >(
        r#"
        SELECT p.trade_uuid, p.token_symbol, p.closed_at,
               json_array_length(et.targets_hit) as targets_count,
               et.peak_profit_percent * p.entry_amount_sol / 100.0 as gain_sol,
               p.strategy
        FROM positions p
        JOIN exit_targets et ON p.trade_uuid = et.trade_uuid
        WHERE p.state = 'CLOSED'
          AND et.targets_hit IS NOT NULL
          AND json_array_length(et.targets_hit) > 0
          AND p.closed_at >= datetime('now', ?)
        ORDER BY p.closed_at DESC
        LIMIT 10
        "#,
    )
    .bind(&days_str)
    .fetch_all(&pool)
    .await?;

    let recent_hits: Vec<ProfitTargetHit> = recent_rows
        .iter()
        .map(
            |(uuid, symbol, closed_at, targets_count, gain, strategy)| ProfitTargetHit {
                timestamp: closed_at.clone().unwrap_or_default(),
                trade_uuid: uuid.clone(),
                token_symbol: symbol.clone(),
                target_level: targets_count.unwrap_or(1) as i32,
                realized_gain_sol: *gain,
                strategy_name: strategy.clone(),
            },
        )
        .collect();

    // Calculate hit rate (hits with targets / total closed)
    let total_closed: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM positions
        WHERE state = 'CLOSED'
          AND closed_at >= datetime('now', ?)
        "#,
    )
    .bind(&days_str)
    .fetch_one(&pool)
    .await?;

    let total_closed_count = total_closed.0.unwrap_or(0);
    let hit_rate = if total_closed_count > 0 {
        (total_hits as f64 / total_closed_count as f64) * 100.0
    } else {
        0.0
    };

    Ok(ProfitTargetMetricsResponse {
        hit_rate,
        total_hits,
        total_targets: total_closed_count,
        trailing_stop_activations,
        average_realized_gain_sol,
        targets_by_strategy,
        recent_hits,
    })
}

/// Get position size analysis
async fn get_position_size_analysis_db(
    db: &Arc<dyn Database>,
) -> AppResult<PositionSizeAnalysisResponse> {
    let pool = sqlite_pool(db)?;
    // Get statistics
    let stats: (Option<f64>, Option<f64>, Option<f64>) = sqlx::query_as(
        r#"
        SELECT
            AVG(entry_amount_sol) as avg_position,
            MAX(entry_amount_sol) as max_position,
            MIN(entry_amount_sol) as min_position
        FROM positions
        WHERE state IN ('ACTIVE', 'EXITING')
        "#,
    )
    .fetch_one(&pool)
    .await?;

    let average_position_sol = stats.0.unwrap_or(0.0);
    let max_position_sol = stats.1.unwrap_or(0.0);
    let min_position_sol = stats.2.unwrap_or(0.0);

    // Get median using OFFSET (handle empty case)
    let median_result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT entry_amount_sol
        FROM positions
        WHERE state IN ('ACTIVE', 'EXITING')
        ORDER BY entry_amount_sol
        LIMIT 1
        OFFSET (SELECT COUNT(*) / 2 FROM positions WHERE state IN ('ACTIVE', 'EXITING'))
        "#,
    )
    .fetch_optional(&pool)
    .await?
    .unwrap_or((None,));

    let median_position_sol = median_result.0.unwrap_or(0.0);

    // Get distribution buckets
    let bucket_rows = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT size_bucket, COUNT(*) as count
        FROM (
            SELECT CASE
                WHEN entry_amount_sol < 0.1 THEN '0-0.1 SOL'
                WHEN entry_amount_sol < 0.5 THEN '0.1-0.5 SOL'
                WHEN entry_amount_sol < 1.0 THEN '0.5-1.0 SOL'
                WHEN entry_amount_sol < 5.0 THEN '1-5 SOL'
                WHEN entry_amount_sol < 10.0 THEN '5-10 SOL'
                ELSE '10+ SOL'
            END as size_bucket
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
        ) GROUP BY size_bucket
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let total_count: i64 = bucket_rows.iter().map(|r| r.1).sum();

    let position_size_distribution: Vec<SizeBucket> = bucket_rows
        .iter()
        .map(|(range, count)| SizeBucket {
            size_range: range.clone(),
            count: *count,
            percentage: if total_count > 0 {
                (*count as f64 / total_count as f64) * 100.0
            } else {
                0.0
            },
        })
        .collect();

    Ok(PositionSizeAnalysisResponse {
        average_position_sol,
        median_position_sol,
        max_position_sol,
        min_position_sol,
        position_size_distribution,
        kelly_criterion_usage: 0.0, // Placeholder - not calculated yet
    })
}

// =============================================================================
// API HANDLERS
// =============================================================================

/// Get portfolio risk metrics
///
/// GET /api/v1/risk/portfolio
///
/// Returns portfolio heat, concentration (by token and sector), exposure,
/// and drawdown metrics.
pub async fn get_portfolio_risk(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<PortfolioRiskResponse>, AppError> {
    let config = state.config.read().await;
    let heat_threshold = config
        .position_sizing
        .total_capital_sol
        .to_f64()
        .unwrap_or(100.0);
    let total_capital = config.position_sizing.total_capital_sol;
    drop(config);

    // Get concentrations
    let (by_token, by_sector, max_concentration) = get_position_concentrations(&state.db).await?;

    // Get exposure
    let mut exposure = get_portfolio_exposure(&state.db).await?;

    // Get drawdown
    let current_drawdown = state.db.get_max_drawdown_percent(total_capital).await?;
    let current_drawdown_f64 = current_drawdown.to_f64().unwrap_or(0.0);

    exposure.max_drawdown_percent = current_drawdown_f64.max(exposure.max_drawdown_percent);
    exposure.current_drawdown_percent = current_drawdown_f64;

    // Calculate HHI
    let hhi = calculate_hhi(&by_token);

    // Determine heat status
    let heat_status = determine_heat_status(exposure.total_exposure_sol, heat_threshold);

    // Drawdown data (simplified - duration and recovery not tracked in detail)
    let drawdown = DrawdownData {
        current_drawdown_percent: current_drawdown_f64,
        max_drawdown_percent: exposure.max_drawdown_percent,
        drawdown_duration_days: 0.0, // Would need timestamp tracking
        recovery_percent: if current_drawdown_f64 > 0.0 {
            ((exposure.max_drawdown_percent - current_drawdown_f64)
                / exposure.max_drawdown_percent.max(0.01))
                * 100.0
        } else {
            100.0
        },
    };

    Ok(Json(PortfolioRiskResponse {
        portfolio_heat_percent: exposure.total_exposure_sol,
        heat_threshold,
        heat_status: heat_status.to_string(),
        concentration: ConcentrationData {
            by_token,
            by_sector,
            max_concentration_percent: max_concentration,
            hhi,
        },
        exposure,
        drawdown,
        total_capital_sol: total_capital.to_f64().unwrap_or(0.0),
    }))
}

/// Get stop loss metrics
///
/// GET /api/v1/risk/stop-loss?days=30
///
/// Returns stop loss activation rate, loss prevented, and recent activations.
pub async fn get_stop_loss_metrics(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<StopLossMetricsResponse>, AppError> {
    let metrics = get_stop_loss_metrics_db(&state.db, params.days).await?;
    Ok(Json(metrics))
}

/// Get profit target metrics
///
/// GET /api/v1/risk/profit-target?days=30
///
/// Returns profit target hit rate and recent hits.
pub async fn get_profit_target_metrics(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<ProfitTargetMetricsResponse>, AppError> {
    let metrics = get_profit_target_metrics_db(&state.db, params.days).await?;
    Ok(Json(metrics))
}

/// Get position size analysis
///
/// GET /api/v1/risk/position-size
///
/// Returns position size statistics and distribution.
pub async fn get_position_size_analysis(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<PositionSizeAnalysisResponse>, AppError> {
    let analysis = get_position_size_analysis_db(&state.db).await?;
    Ok(Json(analysis))
}
