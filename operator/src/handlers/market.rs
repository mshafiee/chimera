//! Market data API handlers
//!
//! Provides endpoints for market regime detection and conditions analysis.

use axum::{extract::State, Json};
use chrono::Utc;
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
    /// Confidence score (0-1)
    pub confidence: f64,
    /// Volatility index
    pub volatility_index: f64,
    /// Trend strength
    pub trend_strength: f64,
    /// ISO timestamp of last regime change
    pub last_regime_change: String,
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
    /// Volatility index
    pub volatility_index: f64,
    /// Trend strength
    pub trend_strength: f64,
    /// Liquidity index
    pub liquidity_index: f64,
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

    // Detect current regime
    let regime = detector.detect_regime();
    let current_regime = match regime {
        crate::engine::MarketRegime::Bull => "bull",
        crate::engine::MarketRegime::Bear => "bear",
        crate::engine::MarketRegime::Sideways => "neutral",
    };

    // Get price history and calculate metrics
    let price_history = detector.get_price_history();
    let history = price_history.read();
    let volatility_index = calculate_volatility(&history);
    let trend_strength = calculate_trend_strength(&history);
    drop(history); // Release lock

    // Fixed confidence for now (placeholder)
    let confidence = 0.75;

    // Current timestamp for last_regime_change (placeholder)
    let last_regime_change = Utc::now().to_rfc3339();

    // Empty regime history for now (placeholder - would need persistence)
    let regime_history = vec![];

    // Empty performance_by_regime for now (placeholder - would need analytics)
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

    // Detect current regime
    let regime = detector.detect_regime();

    // Get price history and calculate metrics
    let price_history = detector.get_price_history();
    let history = price_history.read();
    let volatility_index = calculate_volatility(&history);
    let trend_strength = calculate_trend_strength(&history);
    drop(history); // Release lock

    // Market sentiment derived from regime
    let market_sentiment = match regime {
        crate::engine::MarketRegime::Bull => "bullish",
        crate::engine::MarketRegime::Bear => "bearish",
        crate::engine::MarketRegime::Sideways => "neutral",
    };

    // Risk level based on volatility
    let risk_level = if volatility_index < 20.0 {
        "low"
    } else if volatility_index < 40.0 {
        "medium"
    } else {
        "high"
    };

    // Liquidity index (placeholder - would need DEX aggregation)
    let liquidity_index = 50.0;

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
fn calculate_volatility(
    price_history: &std::collections::VecDeque<(chrono::DateTime<chrono::Utc>, rust_decimal::Decimal)>,
) -> f64 {
    if price_history.len() < 2 {
        return 0.0;
    }

    let prices: Vec<f64> = price_history
        .iter()
        .map(|(_, p)| p.to_f64().unwrap_or(0.0))
        .collect();

    let mean = prices.iter().sum::<f64>() / prices.len() as f64;
    if mean == 0.0 {
        return 0.0;
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
    (std_dev / mean) * 100.0 // As percentage
}

/// Calculate trend strength from price history
///
/// Returns the percentage change from the oldest to newest price.
fn calculate_trend_strength(
    price_history: &std::collections::VecDeque<(chrono::DateTime<chrono::Utc>, rust_decimal::Decimal)>,
) -> f64 {
    if price_history.len() < 2 {
        return 0.0;
    }

    let first_price = price_history
        .front()
        .and_then(|(_, p)| p.to_f64())
        .unwrap_or(0.0);
    let last_price = price_history
        .back()
        .and_then(|(_, p)| p.to_f64())
        .unwrap_or(0.0);

    if first_price == 0.0 {
        return 0.0;
    }

    ((last_price - first_price) / first_price) * 100.0
}
