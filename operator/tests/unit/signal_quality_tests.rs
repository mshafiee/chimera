//! Unit tests for signal quality module

#[cfg(test)]
mod tests {
    use chimera_operator::engine::signal_quality::{QualityCategory, SignalQuality};
    use rust_decimal::Decimal;

    const EPS: f64 = 1e-9;

    fn assert_score_approx(quality: &SignalQuality, expected: f64) {
        assert!(
            (quality.score - expected).abs() < EPS,
            "score {} != expected {}",
            quality.score,
            expected
        );
    }

    #[test]
    fn test_high_quality_signal() {
        // WQS 90 → 0.9×0.4 = 0.36; 5-wallet consensus → 1.0×0.3 = 0.30;
        // liquidity ≥50k → 1.0×0.2 = 0.20; age >168h → 1.0×0.1 = 0.10 → 0.96
        let quality = SignalQuality::calculate(
            90.0,                    // High WQS
            Some(5),                 // Strong consensus (5 wallets)
            Decimal::from(60000u32), // High liquidity
            Some(200.0),             // Old token
        );

        assert_score_approx(&quality, 0.96);
        assert!(quality.should_enter(0.7));
        assert_eq!(quality.category(), QualityCategory::High);
    }

    #[test]
    fn test_medium_quality_signal() {
        // WQS 70 → 0.28; consensus(2w→0.5): 0.15; liquidity(>10k=0.5): 0.10;
        // age(48h>24h=0.7): 0.07 → 0.60
        let quality = SignalQuality::calculate(
            70.0,                    // Medium WQS
            Some(2),                 // 2-wallet consensus → score 0.5 → 0.5×0.3=0.15 boost
            Decimal::from(15000u32), // Medium liquidity ($15k)
            Some(48.0),              // 2 days old
        );

        assert_score_approx(&quality, 0.60);
        assert!(quality.should_enter(0.55));
    }

    #[test]
    fn test_low_quality_signal() {
        // WQS 50 → 0.20; no consensus → 0; liquidity(≤5k=0.1): 0.02;
        // age(<6h=0.3): 0.03 → 0.25
        let quality = SignalQuality::calculate(
            50.0,                   // Low WQS
            None,                   // No consensus
            Decimal::from(3000u32), // Low liquidity
            Some(2.0),              // Very new token
        );

        assert_score_approx(&quality, 0.25);
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

    #[test]
    fn test_wqs_out_of_range_is_clamped() {
        // WQS > 100 clamps to 1.0 → 0.4; negative clamps to 0 → 0.0.
        let high = SignalQuality::calculate(150.0, None, Decimal::from(60000u32), Some(200.0));
        assert_score_approx(&high, 0.4 + 0.2 + 0.1);

        let negative = SignalQuality::calculate(-30.0, None, Decimal::from(60000u32), Some(200.0));
        assert_score_approx(&negative, 0.0 + 0.2 + 0.1);
    }

    #[test]
    fn test_liquidity_threshold_boundaries() {
        // Boundaries use `>=`: exactly at the floor scores the tier above.
        // (age None → neutral 0.05 is included in every expected value)
        let at_5k = SignalQuality::calculate(0.0, None, Decimal::from(5000u32), None);
        assert_score_approx(&at_5k, 0.3 * 0.2 + 0.05);
        let at_10k = SignalQuality::calculate(0.0, None, Decimal::from(10000u32), None);
        assert_score_approx(&at_10k, 0.5 * 0.2 + 0.05);
        let at_20k = SignalQuality::calculate(0.0, None, Decimal::from(20000u32), None);
        assert_score_approx(&at_20k, 0.7 * 0.2 + 0.05);
        let at_50k = SignalQuality::calculate(0.0, None, Decimal::from(50000u32), None);
        assert_score_approx(&at_50k, 1.0 * 0.2 + 0.05);
        let just_below = SignalQuality::calculate(0.0, None, Decimal::from(4999u32), None);
        assert_score_approx(&just_below, 0.1 * 0.2 + 0.05);

        // Zero and negative liquidity score the 0.1 floor tier, never below.
        let zero = SignalQuality::calculate(0.0, None, Decimal::ZERO, None);
        assert_score_approx(&zero, 0.1 * 0.2 + 0.05);
        let negative = SignalQuality::calculate(0.0, None, Decimal::from(-100), None);
        assert_score_approx(&negative, 0.1 * 0.2 + 0.05);
    }

    #[test]
    fn test_token_age_boundaries() {
        // Age uses strict `>`: exactly 6/24/168h score the LOWER tier.
        // (liquidity 60000 → 0.20 is included in every expected value)
        let at_6h = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), Some(6.0));
        assert_score_approx(&at_6h, 0.2 + 0.3 * 0.1);
        let just_over_6h = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), Some(6.0001));
        assert_score_approx(&just_over_6h, 0.2 + 0.5 * 0.1);
        let at_24h = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), Some(24.0));
        assert_score_approx(&at_24h, 0.2 + 0.5 * 0.1);
        let at_168h = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), Some(168.0));
        assert_score_approx(&at_168h, 0.2 + 0.7 * 0.1);
        let just_over_168h = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), Some(168.0001));
        assert_score_approx(&just_over_168h, 0.2 + 1.0 * 0.1);

        // Unknown age → neutral 0.5.
        let unknown = SignalQuality::calculate(0.0, None, Decimal::from(60000u32), None);
        assert_score_approx(&unknown, 0.2 + 0.5 * 0.1);
    }
}
