//! Unit tests for signal quality module

#[cfg(test)]
mod tests {
    use chimera_operator::engine::signal_quality::{SignalQuality, QualityCategory};
    use rust_decimal::Decimal;

    #[test]
    fn test_high_quality_signal() {
        let quality = SignalQuality::calculate(
            90.0,  // High WQS
            true,  // Consensus
            Decimal::from(60000u32),  // High liquidity
            Some(200.0),  // Old token
        );

        assert!(quality.score >= 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::High);
    }

    #[test]
    fn test_medium_quality_signal() {
        // Medium quality: decent WQS with consensus, medium liquidity, 2 days old
        // Without consensus, max achievable score is ~0.58 (WQS weight=40%, consensus=30%)
        let quality = SignalQuality::calculate(
            70.0,  // Medium WQS
            true,   // Consensus (needed to reach Medium category threshold of 0.7)
            Decimal::from(15000u32),  // Medium liquidity ($15k)
            Some(48.0),  // 2 days old
        );

        // WQS: 0.7×0.4=0.28, Consensus: 1.0×0.3=0.30, Liquidity(15k>10k=0.5): 0.5×0.2=0.10, Age(48h>24h=0.7): 0.7×0.1=0.07 → 0.75
        assert!(quality.score >= 0.7, "score was {}", quality.score);
        assert!(quality.score < 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Medium);
    }

    #[test]
    fn test_low_quality_signal() {
        let quality = SignalQuality::calculate(
            50.0,  // Low WQS
            false,  // No consensus
            Decimal::from(3000u32),  // Low liquidity
            Some(2.0),  // Very new token
        );

        assert!(quality.score < 0.7);
        assert!(!quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::Low);
    }

    #[test]
    fn test_consensus_boost() {
        let with_consensus = SignalQuality::calculate(60.0, true, Decimal::from(10000u32), None);
        let without_consensus = SignalQuality::calculate(60.0, false, Decimal::from(10000u32), None);

        assert!(with_consensus.score > without_consensus.score);
    }

    #[test]
    fn test_liquidity_scoring() {
        let high_liquidity = SignalQuality::calculate(70.0, false, Decimal::from(60000u32), None);
        let low_liquidity = SignalQuality::calculate(70.0, false, Decimal::from(3000u32), None);

        assert!(high_liquidity.score > low_liquidity.score);
    }
}






