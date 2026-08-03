//! Market data API handlers
//!
//! Provides endpoints for market regime detection and conditions analysis.

use axum::{extract::State, Json};
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;
use std::sync::Arc;

use crate::error::AppError;
use crate::handlers::ApiState;

// =============================================================================
// RESPONSE STRUCTS
// =============================================================================

/// Market regime response
#[derive(Debug, Serialize)]
pub struct MarketRegimeResponse {
    /// Current market regime
    pub current_regime: String,
    /// Confidence score (0-1); None while analytics are not computed
    pub confidence: Option<f64>,
    /// Volatility index; None when there is insufficient price data
    pub volatility_index: Option<f64>,
    /// Trend strength; None when there is insufficient price data
    pub trend_strength: Option<f64>,
    /// ISO timestamp of last regime change; None while not tracked
    pub last_regime_change: Option<String>,
    /// Historical regime data points
    pub regime_history: Vec<RegimeHistoryPoint>,
    /// Performance metrics by regime
    pub performance_by_regime: Vec<PerformanceByRegime>,
}

/// Individual regime history point
#[derive(Debug, Serialize)]
pub struct RegimeHistoryPoint {
    /// ISO timestamp
    pub timestamp: String,
    /// Regime at this point
    pub regime: String,
    /// Volatility index at this point
    pub volatility_index: f64,
}

/// Performance metrics for a specific regime
#[derive(Debug, Serialize)]
pub struct PerformanceByRegime {
    /// Regime type
    pub regime: String,
    /// Total trades in this regime
    pub total_trades: u32,
    /// Win rate (0-100)
    pub win_rate: f64,
    /// Average return per trade
    pub avg_return: f64,
    /// Total PnL in this regime
    pub total_pnl: f64,
    /// Sharpe ratio
    pub sharpe_ratio: f64,
}

/// Market conditions response
#[derive(Debug, Serialize)]
pub struct MarketConditionsResponse {
    /// Volatility index; None when there is insufficient price data
    pub volatility_index: Option<f64>,
    /// Trend strength; None when there is insufficient price data
    pub trend_strength: Option<f64>,
    /// Liquidity index; None until real DEX aggregation is implemented
    pub liquidity_index: Option<f64>,
    /// Market sentiment
    pub market_sentiment: String,
    /// Risk level
    pub risk_level: String,
    /// Recommended allocation
    pub recommended_allocation: RecommendedAllocation,
}

/// Recommended allocation split
#[derive(Debug, Serialize)]
pub struct RecommendedAllocation {
    /// Shield percentage
    pub shield_percent: u32,
    /// Spear percentage
    pub spear_percent: u32,
}

// =============================================================================
// HANDLERS
// =============================================================================

/// Get market regime data
///
/// GET /api/v1/market/regime
/// Public access (no authentication required)
pub async fn get_market_regime(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<MarketRegimeResponse>, AppError> {
    // Get regime detector from state
    let detector = state
        .market_regime_detector
        .as_ref()
        .ok_or_else(|| AppError::Internal("Market regime detector not initialized".to_string()))?;

    // Read the price history snapshot once and derive the regime and the
    // volatility/trend metrics from the same snapshot so they cannot contradict
    // each other (a price update landing in between would otherwise mix snapshots).
    let history = detector.get_price_history();
    let regime = detector.detect_regime_from_history(&history);
    let current_regime = match regime {
        crate::engine::MarketRegime::Bull => "bull",
        crate::engine::MarketRegime::Bear => "bear",
        crate::engine::MarketRegime::Sideways => "neutral",
    };

    let volatility_index = calculate_volatility(&history);
    let trend_strength = calculate_trend_strength(&history);

    // Placeholder analytics: expose None rather than fabricated values so
    // consumers can distinguish unavailable from real data.
    let confidence = None;
    let last_regime_change = None;
    let regime_history = vec![];
    let performance_by_regime = vec![];

    Ok(Json(MarketRegimeResponse {
        current_regime: current_regime.to_string(),
        confidence,
        volatility_index,
        trend_strength,
        last_regime_change,
        regime_history,
        performance_by_regime,
    }))
}

/// Get current market conditions
///
/// GET /api/v1/market/conditions
/// Public access (no authentication required)
pub async fn get_market_conditions(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<MarketConditionsResponse>, AppError> {
    // Get regime detector from state
    let detector = state
        .market_regime_detector
        .as_ref()
        .ok_or_else(|| AppError::Internal("Market regime detector not initialized".to_string()))?;

    // Read the price history snapshot once and derive both regime and metrics
    // from the same snapshot for internal consistency.
    let history = detector.get_price_history();
    let regime = detector.detect_regime_from_history(&history);
    let volatility_index = calculate_volatility(&history);
    let trend_strength = calculate_trend_strength(&history);

    // Market sentiment derived from regime
    let market_sentiment = match regime {
        crate::engine::MarketRegime::Bull => "bullish",
        crate::engine::MarketRegime::Bear => "bearish",
        crate::engine::MarketRegime::Sideways => "neutral",
    };

    // Risk level based on volatility. Unknown when there is no data — reporting
    // "low" risk for an empty price history would be a dangerously reassuring verdict.
    let risk_level = match volatility_index {
        Some(v) if v < 20.0 => "low",
        Some(v) if v < 40.0 => "medium",
        Some(_) => "high",
        None => "unknown",
    };

    // Liquidity index: unavailable until real DEX aggregation exists.
    let liquidity_index = None;

    // Recommended allocation based on regime
    let (shield_percent, spear_percent) = match regime {
        crate::engine::MarketRegime::Bull => (60, 40),
        crate::engine::MarketRegime::Bear => (80, 20),
        crate::engine::MarketRegime::Sideways => (70, 30),
    };

    Ok(Json(MarketConditionsResponse {
        volatility_index,
        trend_strength,
        liquidity_index,
        market_sentiment: market_sentiment.to_string(),
        risk_level: risk_level.to_string(),
        recommended_allocation: RecommendedAllocation {
            shield_percent,
            spear_percent,
        },
    }))
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Calculate volatility index from price history
///
/// Uses standard deviation of prices as a percentage of the mean price.
/// Returns `None` when there is insufficient data or a price cannot be
/// represented as f64, so callers can distinguish "no data" from a flat market.
fn calculate_volatility(
    price_history: &std::collections::VecDeque<(
        chrono::DateTime<chrono::Utc>,
        rust_decimal::Decimal,
    )>,
) -> Option<f64> {
    if price_history.len() < 2 {
        return None;
    }

    let prices: Vec<f64> = price_history
        .iter()
        .map(|(_, p)| p.to_f64())
        .collect::<Option<Vec<f64>>>()?;

    let mean = prices.iter().sum::<f64>() / prices.len() as f64;
    if mean == 0.0 {
        return None;
    }

    let variance = prices
        .iter()
        .map(|p| {
            let diff = p - mean;
            diff * diff
        })
        .sum::<f64>()
        / prices.len() as f64;

    let std_dev = variance.sqrt();
    Some((std_dev / mean) * 100.0) // As percentage
}

/// Calculate trend strength from price history
///
/// Returns the percentage change from the oldest to newest price, or `None`
/// when there is insufficient data or the reference price is unavailable/zero.
fn calculate_trend_strength(
    price_history: &std::collections::VecDeque<(
        chrono::DateTime<chrono::Utc>,
        rust_decimal::Decimal,
    )>,
) -> Option<f64> {
    if price_history.len() < 2 {
        return None;
    }

    let first_price = price_history.front().and_then(|(_, p)| p.to_f64())?;
    let last_price = price_history.back().and_then(|(_, p)| p.to_f64())?;

    if first_price == 0.0 {
        return None;
    }

    Some(((last_price - first_price) / first_price) * 100.0)
}
