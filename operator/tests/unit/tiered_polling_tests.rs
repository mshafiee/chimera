#[cfg(test)]
mod tests {
    use chimera_operator::config::{MonitoringConfig, TieredPollingConfig};
    use rust_decimal::Decimal;

    #[test]
    fn test_tiered_polling_config_defaults() {
        let config = TieredPollingConfig::default();
        assert_eq!(config.high_conviction_interval_secs, 5);
        assert_eq!(config.regular_conviction_interval_secs, 8);
        assert_eq!(config.emerging_conviction_interval_secs, 30);
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
            5
        );

        // Regular conviction (WQS 70, ACTIVE)
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "ACTIVE"),
            8
        );

        // Emerging conviction (WQS 50, ACTIVE)
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "ACTIVE"),
            30
        );

        // CANDIDATE status always uses emerging interval
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(85)), "CANDIDATE"),
            30
        );

        // WQS exactly at high threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(80)), "ACTIVE"),
            5
        );

        // WQS exactly at regular threshold
        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(60)), "ACTIVE"),
            8
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
            30
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
            30
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(70)), "CANDIDATE"),
            30
        );

        assert_eq!(
            monitoring_config.get_polling_interval_for_wallet(Some(Decimal::from(50)), "CANDIDATE"),
            30
        );
    }
}
