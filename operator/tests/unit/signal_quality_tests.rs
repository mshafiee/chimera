//! Unit tests for signal quality module

#[cfg(test)]
mod tests {
    use chimera_operator::engine::signal_quality::{SignalQuality, QualityCategory};

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
