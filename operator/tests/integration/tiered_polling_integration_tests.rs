//! Tiered polling integration tests
//!
//! Tests the REAL tiered-polling surfaces:
//! - `get_wallets_by_conviction_tier` against a test database
//! - `MonitoringConfig::get_polling_interval_for_wallet` (the production
//!   interval function) for tier boundaries and CANDIDATE handling
//! - Config defaults via serde
//!
//! NOTE: the end-to-end polling loop (interval scheduling + signal generation)
//! needs a mock RPC client that does not exist yet, so it is not tested here.

use chimera_operator::config::{MonitoringConfig, TieredPollingConfig};
use chimera_operator::db_abstraction::Database;
use rust_decimal::Decimal;

#[path = "../common/mod.rs"]
mod common;

#[tokio::test]
async fn test_get_wallets_by_conviction_tier() {
    use chimera_operator::config::ConvictionTier;

    let (db, _guard) = common::create_test_pg_db().await;

    // Wallets spanning the tiers (per the DB query: status ACTIVE, WQS >= 80
    // → High; 60..=79 → Regular; <= 59 → Emerging).
    for (address, wqs) in [
        ("wallet_high", 85),
        ("wallet_regular", 70),
        ("wallet_emerging", 50),
    ] {
        db.upsert_wallet(
            address,
            Some(Decimal::from(wqs)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // The tier query filters on status = 'ACTIVE'.
        db.update_wallet_status_ext(address, "ACTIVE", None, None)
            .await
            .unwrap();
    }

    let high = db
        .get_wallets_by_conviction_tier(ConvictionTier::High)
        .await
        .unwrap();
    assert_eq!(high.len(), 1, "High tier must contain exactly wallet_high");
    assert_eq!(high[0].address, "wallet_high");

    let regular = db
        .get_wallets_by_conviction_tier(ConvictionTier::Regular)
        .await
        .unwrap();
    assert_eq!(
        regular.len(), 1,
        "Regular tier must contain exactly wallet_regular"
    );
    assert_eq!(regular[0].address, "wallet_regular");

    let emerging = db
        .get_wallets_by_conviction_tier(ConvictionTier::Emerging)
        .await
        .unwrap();
    assert_eq!(
        emerging.len(), 1,
        "Emerging tier must contain exactly wallet_emerging"
    );
    assert_eq!(emerging[0].address, "wallet_emerging");
}

#[tokio::test]
async fn test_tiered_polling_configuration_loading() {
    // Serde defaults are the single source of truth: an empty config must
    // deserialize to the documented tier intervals/thresholds.
    let config: TieredPollingConfig =
        serde_json::from_str("{}").expect("empty config must deserialize");
    assert_eq!(config.high_conviction_interval_secs, 5);
    assert_eq!(config.regular_conviction_interval_secs, 8);
    assert_eq!(config.emerging_conviction_interval_secs, 30);

    let monitoring = MonitoringConfig::default();
    assert!(monitoring.tiered_polling_enabled, "tiered polling is on by default");
}

#[test]
fn test_conviction_tier_classification() {
    // The production interval function IS the tier classifier (config.rs
    // get_polling_interval_for_wallet): WQS >= 80 → high (5s), 60..=79 →
    // regular (8s), < 60 → emerging (30s), CANDIDATE always emerging.
    let monitoring = MonitoringConfig {
        tiered_polling_enabled: true,
        tiered_polling: Some(TieredPollingConfig::default()),
        ..Default::default()
    };

    for wqs in [80, 85, 90, 100] {
        assert_eq!(
            monitoring.get_polling_interval_for_wallet(Some(Decimal::from(wqs)), "ACTIVE"),
            5,
            "WQS {wqs} must classify as High (5s)"
        );
    }
    for wqs in [60, 65, 70, 79] {
        assert_eq!(
            monitoring.get_polling_interval_for_wallet(Some(Decimal::from(wqs)), "ACTIVE"),
            8,
            "WQS {wqs} must classify as Regular (8s)"
        );
    }
    for wqs in [0, 30, 50, 59] {
        assert_eq!(
            monitoring.get_polling_interval_for_wallet(Some(Decimal::from(wqs)), "ACTIVE"),
            30,
            "WQS {wqs} must classify as Emerging (30s)"
        );
    }
}

#[test]
fn test_polling_interval_calculation() {
    // Exact boundary values against the production function, including the
    // CANDIDATE special case (always emerging).
    let monitoring = MonitoringConfig {
        tiered_polling_enabled: true,
        tiered_polling: Some(TieredPollingConfig::default()),
        ..Default::default()
    };

    let test_cases = vec![
        (90, "ACTIVE", 5),    // High conviction
        (80, "ACTIVE", 5),    // At high threshold (>= 80)
        (75, "ACTIVE", 8),    // Regular conviction
        (60, "ACTIVE", 8),    // At regular threshold (>= 60)
        (59, "ACTIVE", 30),   // Just below regular → emerging
        (50, "ACTIVE", 30),   // Emerging conviction
        (90, "CANDIDATE", 30), // CANDIDATE always uses emerging
        (95, "REJECTED", 5),  // Non-CANDIDATE statuses use WQS tiering
    ];

    for (wqs, status, expected_interval) in test_cases {
        let interval = monitoring.get_polling_interval_for_wallet(
            Some(Decimal::from(wqs)),
            status,
        );
        assert_eq!(
            interval, expected_interval,
            "Failed for WQS: {}, status: {}",
            wqs, status
        );
    }
}
