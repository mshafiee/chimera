//! Unit tests for signal quality module

#[cfg(test)]
mod tests {
    use chimera_operator::engine::signal_quality::{QualityCategory, SignalQuality};
    use rust_decimal::Decimal;

    #[test]
    fn test_high_quality_signal() {
        // 5-wallet consensus → graduated score 1.0
        let quality = SignalQuality::calculate(
            90.0,                    // High WQS
            Some(5),                 // Strong consensus (5 wallets)
            Decimal::from(60000u32), // High liquidity
            Some(200.0),             // Old token
        );

        assert!(quality.score >= 0.9);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::High);
    }

    #[test]
    fn test_medium_quality_signal() {
        // Medium quality: decent WQS with 2-wallet consensus, medium liquidity, 2 days old
        // Without consensus, max achievable score is ~0.58 (WQS weight=40%, consensus=30%)
        let quality = SignalQuality::calculate(
            70.0,                    // Medium WQS
            Some(2),                 // 2-wallet consensus → score 0.5 → 0.5×0.3=0.15 boost
            Decimal::from(15000u32), // Medium liquidity ($15k)
            Some(48.0),              // 2 days old
        );

        // WQS: 0.7×0.4=0.28, Consensus(2w→0.5): 0.5×0.3=0.15, Liquidity(>10k=0.5): 0.5×0.2=0.10, Age(48h>24h=0.7): 0.7×0.1=0.07 → 0.60
        assert!(quality.score >= 0.55, "score was {}", quality.score);
        assert!(quality.should_enter(0.55));
    }

    #[test]
    fn test_low_quality_signal() {
        let quality = SignalQuality::calculate(
            50.0,                   // Low WQS
            None,                   // No consensus
            Decimal::from(3000u32), // Low liquidity
            Some(2.0),              // Very new token
        );

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
        // More wallets → higher quality score
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
