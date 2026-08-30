#[cfg(test)]
mod tests {
    use chimera_operator::config::{MonitoringConfig, TieredPollingConfig};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_tiered_polling_config_defaults() {
        let config = TieredPollingConfig::default();
        assert_eq!(config.high_conviction_interval_secs, 30);
        assert_eq!(config.regular_conviction_interval_secs, 60);
        assert_eq!(config.emerging_conviction_interval_secs, 120);
        assert_eq!(config.high_conviction_wqs_threshold, 80);
        assert_eq!(config.regular_conviction_wqs_threshold, 60);
    }

    #[test]
    fn test_get_polling_interval_for_wallet() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(TieredPollingConfig::default()),
            ..Default::default()
        };

        // High conviction (WQS 85, ACTIVE)
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "ACTIVE"),
            30
        );

        // Regular conviction (WQS 70, ACTIVE)
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "ACTIVE"),
            60
        );

        // Emerging conviction (WQS 50, ACTIVE)
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "ACTIVE"),
            120
        );

        // CANDIDATE status always uses emerging interval
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "CANDIDATE"),
            120
        );

        // WQS exactly at high threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(80)), "ACTIVE"),
            30
        );

        // WQS exactly at regular threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(60)), "ACTIVE"),
            60
        );
    }

    #[test]
    fn test_backward_compatibility() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: false,
            rpc_poll_interval_secs: 10,
            ..Default::default()
        };

        // Should always return legacy interval when tiered polling disabled
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "ACTIVE"),
            10
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "ACTIVE"),
            10
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "CANDIDATE"),
            10
        );
    }

    #[test]
    fn test_tiered_polling_enabled_without_config_falls_back_to_defaults() {
        // tiered_polling: None → the production code falls back to the default
        // intervals/thresholds instead of panicking or disabling tiering.
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: None,
            ..Default::default()
        };

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "ACTIVE"),
            30,
            "None config must fall back to default high interval"
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "ACTIVE"),
            60
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "ACTIVE"),
            120
        );
    }

    #[test]
    fn test_threshold_lower_boundaries() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(TieredPollingConfig::default()),
            ..Default::default()
        };

        // Just below the high threshold (>= 80) → regular (60).
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(79)), "ACTIVE"),
            60
        );
        // Just below the regular threshold (>= 60) → emerging (120).
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(59)), "ACTIVE"),
            120
        );
        // Fractional WQS rounds to nearest integer BEFORE the comparison:
        // 79.5 → 80 → high (30).
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from_str("79.5").unwrap()), "ACTIVE"),
            30
        );
        // 79.49 → 79 → regular (60).
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from_str("79.49").unwrap()), "ACTIVE"),
            60
        );
    }

    #[test]
    fn test_unknown_wallet_statuses_fall_through_to_wqs() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(TieredPollingConfig::default()),
            ..Default::default()
        };

        // Only the exact "CANDIDATE" status is special-cased; every other
        // string (including case variants and empty) uses WQS tiering.
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "candidate"),
            30
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "Candidate"),
            30
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), ""),
            30
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "ACTIVE "),
            30
        );
    }

    #[test]
    fn test_tiered_polling_with_custom_intervals() {
        let custom_config = TieredPollingConfig {
            high_conviction_interval_secs: 3,
            regular_conviction_interval_secs: 10,
            emerging_conviction_interval_secs: 60,
            high_conviction_wqs_threshold: 90,
            regular_conviction_wqs_threshold: 70,
        };

        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(custom_config),
            ..Default::default()
        };

        // High conviction with custom threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(95)), "ACTIVE"),
            3
        );

        // Regular conviction with custom threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(75)), "ACTIVE"),
            10
        );

        // Below custom regular threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(65)), "ACTIVE"),
            60
        );

        // Exact custom boundary values: >= is inclusive at both thresholds.
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(90)), "ACTIVE"),
            3
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "ACTIVE"),
            10
        );
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(69)), "ACTIVE"),
            60
        );
    }

    #[test]
    fn test_get_polling_interval_none_wqs() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(TieredPollingConfig::default()),
            ..Default::default()
        };

        // WQS is None should default to emerging interval
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(None, "ACTIVE"),
            120
        );
    }

    #[test]
    fn test_candidate_status_always_emerging() {
        let monitoring_config = MonitoringConfig {
            tiered_polling_enabled: true,
            tiered_polling: Some(TieredPollingConfig::default()),
            ..Default::default()
        };

        // CANDIDATE wallets should always use emerging interval regardless of WQS
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(90)), "CANDIDATE"),
            120
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "CANDIDATE"),
            120
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "CANDIDATE"),
            120
        );
    }
}
