//! Signal Quality Scoring
//!
//! Calculates a quality score (0.0-1.0) for trading signals based on:
//! - Wallet WQS (40% weight)
//! - Consensus strength (30% weight)
//! - Liquidity score (20% weight)
//! - Token age (10% weight)
//!
//! Signals with quality < 0.7 are rejected to improve win rate.


/// Signal quality factors
#[derive(Debug, Clone)]
pub struct SignalFactors {
    /// Wallet Quality Score (0-100)
    pub wallet_wqs: f64,
    /// Whether this is a consensus signal (multiple wallets)
    pub consensus_strength: f64,  // 0.0-1.0 (1.0 = strong consensus)
    /// Liquidity score (0.0-1.0) based on USD liquidity
    pub liquidity_score: f64,
    /// Token age in hours (None if unknown)
    pub token_age_hours: Option<f64>,
}

/// Signal quality result
#[derive(Debug, Clone)]
pub struct SignalQuality {
    /// Overall quality score (0.0-1.0)
    pub score: f64,
    /// Individual factors
    pub factors: SignalFactors,
}

impl SignalQuality {
    /// Calculate signal quality score
    ///
    /// # Arguments
    /// * `wallet_wqs` - Wallet Quality Score (0-100)
    /// * `is_consensus` - Whether this is a consensus signal
    /// * `liquidity_usd` - Current liquidity in USD
    /// * `token_age_hours` - Token age in hours (None if unknown)
    ///
    /// # Returns
    /// SignalQuality with score and factors
    pub fn calculate(
        wallet_wqs: f64,
        is_consensus: bool,
        liquidity_usd: f64,
        token_age_hours: Option<f64>,
    ) -> Self {
        let mut score = 0.0;

        // 1. Wallet quality (40% weight)
        let wallet_score = (wallet_wqs / 100.0).min(1.0);
        score += wallet_score * 0.4;

        // 2. Consensus strength (30% weight)
        let consensus_score = if is_consensus {
            1.0
        } else {
            0.0
        };
        score += consensus_score * 0.3;

        // 3. Liquidity score (20% weight)
        let liquidity_score = if liquidity_usd > 50000.0 {
            1.0
        } else if liquidity_usd > 20000.0 {
            0.7
        } else if liquidity_usd > 10000.0 {
            0.5
        } else if liquidity_usd > 5000.0 {
            0.3
        } else {
            0.1
        };
        score += liquidity_score * 0.2;

        // 4. Token age (10% weight) - older tokens are safer
        let age_score = if let Some(age) = token_age_hours {
            if age > 168.0 {
                1.0  // > 7 days
            } else if age > 24.0 {
                0.7  // > 1 day
            } else if age > 6.0 {
                0.5  // > 6 hours
            } else {
                0.3  // < 6 hours (very new)
            }
        } else {
            0.5  // Unknown age - neutral
        };
        score += age_score * 0.1;

        // Clamp to 0.0-1.0
        let score = score.min(1.0).max(0.0);

        SignalQuality {
            score,
            factors: SignalFactors {
                wallet_wqs,
                consensus_strength: consensus_score,
                liquidity_score,
                token_age_hours,
            },
        }
    }

    /// Check if signal should be entered based on minimum quality threshold
    ///
    /// # Arguments
    /// * `min_quality` - Minimum quality score required (default: 0.7)
    ///
    /// # Returns
    /// true if quality >= min_quality, false otherwise
    pub fn should_enter(&self, min_quality: f64) -> bool {
        self.score >= min_quality
    }

    /// Get quality category
    pub fn category(&self) -> QualityCategory {
        if self.score >= 0.9 {
            QualityCategory::High
        } else if self.score >= 0.7 {
            QualityCategory::Medium
        } else {
            QualityCategory::Low
        }
    }
}

/// Quality category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityCategory {
    /// High quality (>= 0.9) - best signals
    High,
    /// Medium quality (0.7-0.9) - acceptable signals
    Medium,
    /// Low quality (< 0.7) - should be rejected
    Low,
}

impl std::fmt::Display for QualityCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QualityCategory::High => write!(f, "HIGH"),
            QualityCategory::Medium => write!(f, "MEDIUM"),
            QualityCategory::Low => write!(f, "LOW"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_quality_signal() {
        let quality = SignalQuality::calculate(
            90.0,  // High WQS
            true,  // Consensus
            60000.0,  // High liquidity
            Some(200.0),  // Old token
        );

        assert!(quality.score >= 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::High);
    }

    #[test]
    fn test_medium_quality_signal() {
        let quality = SignalQuality::calculate(
            70.0,  // Medium WQS
            false,  // No consensus
            15000.0,  // Medium liquidity
            Some(48.0),  // 2 days old
        );

        assert!(quality.score >= 0.7);
        assert!(quality.score < 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Medium);
    }

    #[test]
    fn test_low_quality_signal() {
        let quality = SignalQuality::calculate(
            50.0,  // Low WQS
            false,  // No consensus
            3000.0,  // Low liquidity
            Some(2.0),  // Very new token
        );

        assert!(quality.score < 0.7);
        assert!(!quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Low);
    }

    #[test]
    fn test_consensus_boost() {
        let with_consensus = SignalQuality::calculate(60.0, true, 10000.0, None);
        let without_consensus = SignalQuality::calculate(60.0, false, 10000.0, None);

        assert!(with_consensus.score > without_consensus.score);
    }

    #[test]
    fn test_liquidity_scoring() {
        let high_liquidity = SignalQuality::calculate(70.0, false, 60000.0, None);
        let low_liquidity = SignalQuality::calculate(70.0, false, 3000.0, None);

        assert!(high_liquidity.score > low_liquidity.score);
    }
}
