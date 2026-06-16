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
    pub consensus_strength: f64, // 0.0-1.0 (1.0 = strong consensus)
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
    /// * `consensus_wallet_count` - Number of wallets in agreement (None or Some(0/1) = no consensus)
    /// * `liquidity_usd` - Current liquidity in USD (using Decimal for precision)
    /// * `token_age_hours` - Token age in hours (None if unknown)
    ///
    /// # Returns
    /// SignalQuality with score and factors
    pub fn calculate(
        wallet_wqs: f64,
        consensus_wallet_count: Option<usize>,
        liquidity_usd: rust_decimal::Decimal,
        token_age_hours: Option<f64>,
    ) -> Self {
        use rust_decimal::Decimal;
        let mut score = 0.0;

        // 1. Wallet quality (40% weight)
        let wallet_score = (wallet_wqs / 100.0).min(1.0);
        score += wallet_score * 0.4;

        // 2. Consensus strength (30% weight) — graduated by wallet count
        let consensus_score = match consensus_wallet_count {
            None | Some(0) | Some(1) => 0.0, // no consensus
            Some(2) => 0.5,
            Some(3) => 0.7,
            Some(4) => 0.9,
            Some(_) => 1.0, // 5+ wallets
        };
        score += consensus_score * 0.3;

        // 3. Liquidity score (20% weight) - use Decimal directly for comparisons
        // Note: We keep the score calculation in f64 since it's a statistical metric (0.0-1.0)
        // but we use Decimal for the liquidity threshold comparisons to avoid precision issues
        let liquidity_score: f64 = if liquidity_usd > Decimal::from(50000) {
            1.0
        } else if liquidity_usd > Decimal::from(20000) {
            0.7
        } else if liquidity_usd > Decimal::from(10000) {
            0.5
        } else if liquidity_usd > Decimal::from(5000) {
            0.3
        } else {
            0.1
        };
        score += liquidity_score * 0.2;

        // 4. Token age (10% weight) - older tokens are safer
        let age_score = if let Some(age) = token_age_hours {
            if age > 168.0 {
                1.0 // > 7 days
            } else if age > 24.0 {
                0.7 // > 1 day
            } else if age > 6.0 {
                0.5 // > 6 hours
            } else {
                0.3 // < 6 hours (very new)
            }
        } else {
            0.5 // Unknown age - neutral
        };
        score += age_score * 0.1;

        // Clamp to 0.0-1.0
        let score = score.clamp(0.0, 1.0);

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

    /// Hard liquidity floor check — rejects signals for tokens with insufficient
    /// liquidity regardless of WQS or consensus score. Low-liquidity tokens can
    /// otherwise pass the quality gate if the wallet's WQS is high enough, only
    /// to be rejected later by the slow-path check.
    ///
    /// Returns false (reject) if `liquidity_usd` is below the `min_liquidity` threshold.
    pub fn passes_liquidity_floor(
        liquidity_usd: rust_decimal::Decimal,
        min_liquidity: rust_decimal::Decimal,
    ) -> bool {
        liquidity_usd >= min_liquidity
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
    use rust_decimal::Decimal;

    #[test]
    fn test_high_quality_signal() {
        // 5-wallet consensus → score 1.0
        let quality = SignalQuality::calculate(90.0, Some(5), Decimal::from(60000u32), Some(200.0));

        assert!(quality.score >= 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::High);
    }

    #[test]
    fn test_medium_quality_signal() {
        // 3-wallet consensus → graduated score 0.7 → total: 0.75*0.4 + 0.7*0.3 + liquidity + age = 0.72
        let quality = SignalQuality::calculate(75.0, Some(3), Decimal::from(25000u32), Some(48.0));

        assert!(quality.score >= 0.7, "score was {}", quality.score);
        assert!(quality.score < 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Medium);
    }

    #[test]
    fn test_low_quality_signal() {
        let quality = SignalQuality::calculate(50.0, None, Decimal::from(3000u32), Some(2.0));

        assert!(quality.score < 0.7);
        assert!(!quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Low);
    }

    #[test]
    fn test_consensus_boost() {
        // 2-wallet consensus should score higher than no consensus
        let with_consensus = SignalQuality::calculate(60.0, Some(2), Decimal::from(10000u32), None);
        let without_consensus = SignalQuality::calculate(60.0, None, Decimal::from(10000u32), None);

        assert!(with_consensus.score > without_consensus.score);
    }

    #[test]
    fn test_consensus_graduated() {
        // More wallets → higher score
        let two = SignalQuality::calculate(60.0, Some(2), Decimal::from(10000u32), None);
        let three = SignalQuality::calculate(60.0, Some(3), Decimal::from(10000u32), None);
        let five = SignalQuality::calculate(60.0, Some(5), Decimal::from(10000u32), None);
        assert!(two.score < three.score);
        assert!(three.score < five.score);
    }

    #[test]
    fn test_liquidity_scoring() {
        let high_liquidity = SignalQuality::calculate(70.0, None, Decimal::from(60000u32), None);
        let low_liquidity = SignalQuality::calculate(70.0, None, Decimal::from(3000u32), None);

        assert!(high_liquidity.score > low_liquidity.score);
    }
}
