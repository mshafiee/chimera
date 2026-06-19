//! Market Regime Detection
//!
//! Detects market regime (bull/bear/sideways) from SOL price trends
//! and adjusts profit targets accordingly.

use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::collections::VecDeque;
use std::sync::Arc;

/// Market regime type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketRegime {
    /// Bull market (upward trend)
    Bull,
    /// Bear market (downward trend)
    Bear,
    /// Sideways market (no clear trend)
    Sideways,
}

impl std::fmt::Display for MarketRegime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketRegime::Bull => write!(f, "BULL"),
            MarketRegime::Bear => write!(f, "BEAR"),
            MarketRegime::Sideways => write!(f, "SIDEWAYS"),
        }
    }
}

/// Market regime detector
#[allow(clippy::type_complexity)]
pub struct MarketRegimeDetector {
    price_cache: Arc<PriceCache>,
    /// Price history for trend analysis (last 24 hours)
    price_history:
        Arc<parking_lot::RwLock<VecDeque<(chrono::DateTime<chrono::Utc>, rust_decimal::Decimal)>>>,
    /// Volume history for trend analysis (weekly snapshots of total Solana DEX volume)
    volume_history: Arc<parking_lot::RwLock<VecDeque<(chrono::DateTime<chrono::Utc>, Decimal)>>>,
    /// SOL mint address
    sol_mint: String,
}

impl MarketRegimeDetector {
    /// Create a new market regime detector
    pub fn new(price_cache: Arc<PriceCache>) -> Self {
        Self {
            price_cache,
            price_history: Arc::new(parking_lot::RwLock::new(VecDeque::new())),
            volume_history: Arc::new(parking_lot::RwLock::new(VecDeque::new())),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
        }
    }

    pub async fn update_price_history(&self) {
        if let Some(price_entry) = self.price_cache.get_price(&self.sol_mint) {
            let mut history = self.price_history.write();

            // Add current price with its actual fetch time
            history.push_back((price_entry.fetched_at, price_entry.price_usd));

            // Keep only last 24 hours based on the latest fetched time
            let cutoff = price_entry.fetched_at - chrono::Duration::hours(24);
            while let Some(front) = history.front() {
                if front.0 < cutoff {
                    history.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    /// Detect current market regime
    ///
    /// # Returns
    /// MarketRegime based on price trend
    pub fn detect_regime(&self) -> MarketRegime {
        let history = self.price_history.read();

        if history.len() < 3 {
            // Not enough data, default to sideways
            return MarketRegime::Sideways;
        }

        // Enforce a minimum history span of 12 hours to avoid treating short-term noise as regime changes
        let first_time = history.front().map(|(t, _)| *t).unwrap_or_else(|| {
            tracing::error!("Empty price history despite len() check - should not happen");
            chrono::Utc::now()
        });
        let last_time = history.back().map(|(t, _)| *t).unwrap_or_else(|| {
            tracing::error!("Empty price history despite len() check - should not happen");
            chrono::Utc::now()
        });
        if last_time.signed_duration_since(first_time) < chrono::Duration::hours(12) {
            return MarketRegime::Sideways;
        }

        // Calculate price change over last 24 hours using Decimal for precision
        let prices: Vec<rust_decimal::Decimal> = history.iter().map(|(_, price)| *price).collect();
        let first_price = prices.first().unwrap_or(&rust_decimal::Decimal::ZERO);
        let last_price = prices.last().unwrap_or(&rust_decimal::Decimal::ZERO);

        if first_price.is_zero() || last_price.is_zero() {
            return MarketRegime::Sideways;
        }

        // Calculate percentage change using Decimal to avoid floating-point precision errors
        let price_change_percent = if !first_price.is_zero() {
            let diff = last_price - first_price;
            let ratio = diff / first_price;
            ratio * rust_decimal::Decimal::from(100)
        } else {
            rust_decimal::Decimal::ZERO
        };

        // Classify regime based on price change (using Decimal comparisons)
        let five_percent =
            rust_decimal::Decimal::from_str("5.0").unwrap_or(rust_decimal::Decimal::ZERO);
        let neg_five_percent =
            rust_decimal::Decimal::from_str("-5.0").unwrap_or(rust_decimal::Decimal::ZERO);

        if price_change_percent > five_percent {
            MarketRegime::Bull
        } else if price_change_percent < neg_five_percent {
            MarketRegime::Bear
        } else {
            MarketRegime::Sideways
        }
    }

    /// Detect market regime for a specific token based on its price history in the cache
    pub fn detect_token_regime(&self, token_address: &str) -> MarketRegime {
        let history = self.price_cache.price_history_read();
        let token_history = match history.get(token_address) {
            Some(th) => th,
            None => return MarketRegime::Sideways,
        };

        if token_history.len() < 3 {
            // Not enough data, default to sideways
            return MarketRegime::Sideways;
        }

        // Enforce a minimum history span of 2 hours for token-specific trend detection
        let first_time = token_history.front().map(|(t, _)| *t).unwrap_or_else(|| {
            tracing::error!("Empty token price history despite len() check - should not happen");
            chrono::Utc::now()
        });
        let last_time = token_history.back().map(|(t, _)| *t).unwrap_or_else(|| {
            tracing::error!("Empty token price history despite len() check - should not happen");
            chrono::Utc::now()
        });
        if last_time.signed_duration_since(first_time) < chrono::Duration::hours(2) {
            return MarketRegime::Sideways;
        }

        // Calculate price change over last 24 hours using Decimal for precision
        let prices: Vec<rust_decimal::Decimal> =
            token_history.iter().map(|(_, price)| *price).collect();
        let first_price = prices.first().unwrap_or(&rust_decimal::Decimal::ZERO);
        let last_price = prices.last().unwrap_or(&rust_decimal::Decimal::ZERO);

        if first_price.is_zero() || last_price.is_zero() {
            return MarketRegime::Sideways;
        }

        // Calculate percentage change using Decimal to avoid floating-point precision errors
        let price_change_percent = if !first_price.is_zero() {
            let diff = last_price - first_price;
            let ratio = diff / first_price;
            ratio * rust_decimal::Decimal::from(100)
        } else {
            rust_decimal::Decimal::ZERO
        };

        // Classify regime based on price change (using Decimal comparisons)
        let five_percent =
            rust_decimal::Decimal::from_str("5.0").unwrap_or(rust_decimal::Decimal::ZERO);
        let neg_five_percent =
            rust_decimal::Decimal::from_str("-5.0").unwrap_or(rust_decimal::Decimal::ZERO);

        if price_change_percent > five_percent {
            MarketRegime::Bull
        } else if price_change_percent < neg_five_percent {
            MarketRegime::Bear
        } else {
            MarketRegime::Sideways
        }
    }

    /// Merge the global SOL regime and the token-specific regime conservatively
    pub fn detect_effective_regime(&self, token_address: &str) -> MarketRegime {
        let global_regime = self.detect_regime();
        let token_regime = self.detect_token_regime(token_address);

        if global_regime == MarketRegime::Bear || token_regime == MarketRegime::Bear {
            MarketRegime::Bear
        } else if global_regime == MarketRegime::Sideways || token_regime == MarketRegime::Sideways
        {
            MarketRegime::Sideways
        } else {
            MarketRegime::Bull
        }
    }

    /// Get position sizing multiplier based on effective regime AND volume trend.
    /// Bull = 1.5, Sideways = 0.8, Bear = 0.5 — then multiplied by the week-over-week
    /// volume trend multiplier (0.7–1.1) and clamped to [0.5, 2.0].
    /// Sideways must be >= Bear: a flat market is less risky than an actively declining one.
    pub fn get_regime_multiplier(&self, token_address: &str) -> Decimal {
        let regime_mult = match self.detect_effective_regime(token_address) {
            MarketRegime::Bull => dec!(1.5),
            MarketRegime::Sideways => dec!(0.8),
            MarketRegime::Bear => dec!(0.5),
        };
        let volume_mult = self.get_volume_trend_multiplier();
        (regime_mult * volume_mult).max(dec!(0.5)).min(dec!(2.0))
    }

    /// Update volume history (called periodically, e.g., daily)
    ///
    /// # Arguments
    /// * `total_dex_volume_usd` - Total Solana DEX volume in USD for the period
    pub fn update_volume_history(&self, total_dex_volume_usd: Decimal) {
        let mut history = self.volume_history.write();
        let now = chrono::Utc::now();

        // Add current volume snapshot
        history.push_back((now, total_dex_volume_usd));

        // Keep only last 2 weeks (14 entries if called daily)
        let cutoff = now - chrono::Duration::days(14);
        while let Some(front) = history.front() {
            if front.0 < cutoff {
                history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Get price history for analysis (read-only access)
    ///
    /// Returns a cloned Arc to the internal price history, allowing read access
    /// without exposing mutable state.
    pub fn get_price_history(
        &self,
    ) -> Arc<parking_lot::RwLock<VecDeque<(chrono::DateTime<chrono::Utc>, rust_decimal::Decimal)>>> {
        self.price_history.clone()
    }

    /// Check volume trend (week-over-week)
    ///
    /// # Returns
    /// Volume trend multiplier:
    /// - > 1.0 if volume is increasing (bullish)
    /// - < 1.0 if volume is decreasing (bearish, reduce position sizes)
    /// - 1.0 if no clear trend or insufficient data
    ///
    /// Returns Decimal for precision in financial calculations.
    pub fn get_volume_trend_multiplier(&self) -> Decimal {
        let history = self.volume_history.read();

        if history.len() < 7 {
            // Need at least 7 days of data (1 week)
            return Decimal::ONE;
        }

        // Get volumes from last week and previous week
        let now = chrono::Utc::now();
        let one_week_ago = now - chrono::Duration::days(7);

        let mut last_week_volume = Decimal::ZERO;
        let mut last_week_count = 0;
        let mut previous_week_volume = Decimal::ZERO;
        let mut previous_week_count = 0;

        for (timestamp, volume) in history.iter() {
            if *timestamp >= one_week_ago {
                last_week_volume += *volume;
                last_week_count += 1;
            } else {
                previous_week_volume += *volume;
                previous_week_count += 1;
            }
        }

        if last_week_count == 0 || previous_week_count == 0 {
            return Decimal::ONE;
        }

        let last_week_avg = last_week_volume / Decimal::from(last_week_count);
        let previous_week_avg = previous_week_volume / Decimal::from(previous_week_count);

        if previous_week_avg == Decimal::ZERO {
            return Decimal::ONE;
        }

        // Calculate week-over-week change using Decimal for precision
        let volume_change_ratio = last_week_avg / previous_week_avg;

        // Return multiplier based on Decimal comparisons:
        // - If volume drops >20%, reduce position sizes by 30%
        // - If volume drops 10-20%, reduce by 15%
        // - If volume increases >20%, increase by 10% (but cap at 1.2x)
        // - Otherwise, neutral (1.0)
        let threshold_80 = dec!(0.8);
        let threshold_90 = dec!(0.9);
        let threshold_120 = dec!(1.2);

        if volume_change_ratio < threshold_80 {
            // Volume dropped >20% — reduce position sizes by 30%
            dec!(0.7)
        } else if volume_change_ratio < threshold_90 {
            // Volume dropped 10-20% — reduce by 15%
            dec!(0.85)
        } else if volume_change_ratio > threshold_120 {
            // Volume increased >20% — modest boost capped at 10%
            dec!(1.1)
        } else {
            // Neutral
            Decimal::ONE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price_cache::{PriceCache, PriceSource};
    use chrono::{Duration, Utc};
    use rust_decimal::Decimal;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_regime_detection_insufficient_span() {
        let price_cache = Arc::new(PriceCache::with_ttl(24 * 3600).expect("Failed to create price cache for test"));
        let detector = MarketRegimeDetector::new(price_cache.clone());
        let sol_mint = "So11111111111111111111111111111111111111112";
        let now = Utc::now();

        // Push 3 price points spanning only 1 hour (price went up 10%)
        price_cache.set_price_with_time(
            sol_mint,
            Decimal::from(100),
            PriceSource::Jupiter,
            now - Duration::hours(1),
        );
        detector.update_price_history().await;

        price_cache.set_price_with_time(
            sol_mint,
            Decimal::from(105),
            PriceSource::Jupiter,
            now - Duration::minutes(30),
        );
        detector.update_price_history().await;

        price_cache.set_price_with_time(sol_mint, Decimal::from(110), PriceSource::Jupiter, now);
        detector.update_price_history().await;

        // Even though price went up 10%, span is only 1 hour (< 12 hours required), so must default to Sideways
        assert_eq!(detector.detect_regime(), MarketRegime::Sideways);
    }

    #[tokio::test]
    async fn test_regime_detection_sufficient_span_bull() {
        let price_cache = Arc::new(PriceCache::with_ttl(24 * 3600).expect("Failed to create price cache for test"));
        let detector = MarketRegimeDetector::new(price_cache.clone());
        let sol_mint = "So11111111111111111111111111111111111111112";
        let now = Utc::now();

        // Push 3 price points spanning 13 hours (price went up 10%)
        price_cache.set_price_with_time(
            sol_mint,
            Decimal::from(100),
            PriceSource::Jupiter,
            now - Duration::hours(13),
        );
        detector.update_price_history().await;

        price_cache.set_price_with_time(
            sol_mint,
            Decimal::from(105),
            PriceSource::Jupiter,
            now - Duration::hours(6),
        );
        detector.update_price_history().await;

        price_cache.set_price_with_time(sol_mint, Decimal::from(110), PriceSource::Jupiter, now);
        detector.update_price_history().await;

        // Span is 13 hours (>= 12 hours) and price went up 10%, so it should detect Bull
        assert_eq!(detector.detect_regime(), MarketRegime::Bull);
    }

    #[tokio::test]
    async fn test_token_regime_detection_insufficient_span() {
        let price_cache = Arc::new(PriceCache::with_ttl(24 * 3600).expect("Failed to create price cache for test"));
        let detector = MarketRegimeDetector::new(price_cache.clone());
        let token = "Token111111111111111111111111111111111111111";
        let now = Utc::now();

        // Push 3 price points spanning only 1 hour (price went down 10%)
        price_cache.set_price_with_time(
            token,
            Decimal::from(100),
            PriceSource::Jupiter,
            now - Duration::hours(1),
        );
        price_cache.set_price_with_time(
            token,
            Decimal::from(95),
            PriceSource::Jupiter,
            now - Duration::minutes(30),
        );
        price_cache.set_price_with_time(token, Decimal::from(90), PriceSource::Jupiter, now);

        // Even though price went down 10%, span is only 1 hour (< 2 hours required), so must default to Sideways
        assert_eq!(detector.detect_token_regime(token), MarketRegime::Sideways);
    }

    #[tokio::test]
    async fn test_token_regime_detection_sufficient_span_bear() {
        let price_cache = Arc::new(PriceCache::with_ttl(24 * 3600).expect("Failed to create price cache for test"));
        let detector = MarketRegimeDetector::new(price_cache.clone());
        let token = "Token111111111111111111111111111111111111111";
        let now = Utc::now();

        // Push 3 price points spanning 3 hours (price went down 10%)
        price_cache.set_price_with_time(
            token,
            Decimal::from(100),
            PriceSource::Jupiter,
            now - Duration::hours(3),
        );
        price_cache.set_price_with_time(
            token,
            Decimal::from(95),
            PriceSource::Jupiter,
            now - Duration::hours(1),
        );
        price_cache.set_price_with_time(token, Decimal::from(90), PriceSource::Jupiter, now);

        // Span is 3 hours (>= 2 hours) and price went down 10%, so it should detect Bear
        assert_eq!(detector.detect_token_regime(token), MarketRegime::Bear);
    }
}
