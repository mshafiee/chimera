//! Integration tests for consensus signal detection
//!
//! Tests that multiple wallets buying the same token within 5 minutes
//! triggers consensus detection and improves signal quality.

use chimera_operator::db_abstraction::{
    create_database, DatabaseConfig,
};
use chimera_operator::monitoring::SignalAggregator;
use rust_decimal::Decimal;
use std::str::FromStr;
use tempfile::TempDir;

#[tokio::test]
async fn test_consensus_detection_two_wallets() {
    // Setup test database
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let aggregator = SignalAggregator::new(db);

    // Test token address
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
    let wallet1 = "Wallet1Address111111111111111111111111111111";
    let wallet2 = "Wallet2Address222222222222222222222222222222";

    // First wallet buys token
    let result1 = aggregator
        .add_signal(
            wallet1,
            token_address,
            "BUY",
            Decimal::from_str("1.0").unwrap(),
        )
        .await;
    assert!(
        result1.is_none(),
        "First signal should not trigger consensus"
    );

    // Second wallet buys same token within 5 minutes
    let result2 = aggregator
        .add_signal(
            wallet2,
            token_address,
            "BUY",
            Decimal::from_str("1.5").unwrap(),
        )
        .await;

    assert!(result2.is_some(), "Second signal should trigger consensus");
    let consensus = result2.unwrap();
    assert_eq!(consensus.wallet_count, 2);
    assert_eq!(consensus.token_address, token_address);
    assert_eq!(
        consensus.total_amount_sol,
        Decimal::from_str("2.5").unwrap()
    );
    assert!(consensus.wallets.contains(&wallet1.to_string()));
    assert!(consensus.wallets.contains(&wallet2.to_string()));
    assert!(consensus.confidence > 0.0);
}

#[tokio::test]
async fn test_consensus_detection_three_wallets() {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let aggregator = SignalAggregator::new(db);
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Add three wallets buying same token
    aggregator
        .add_signal(
            "Wallet1",
            token_address,
            "BUY",
            Decimal::from_str("1.0").unwrap(),
        )
        .await;
    aggregator
        .add_signal(
            "Wallet2",
            token_address,
            "BUY",
            Decimal::from_str("1.5").unwrap(),
        )
        .await;

    let result3 = aggregator
        .add_signal(
            "Wallet3",
            token_address,
            "BUY",
            Decimal::from_str("2.0").unwrap(),
        )
        .await;

    assert!(result3.is_some());
    let consensus = result3.unwrap();
    assert_eq!(consensus.wallet_count, 3);
    assert_eq!(
        consensus.total_amount_sol,
        Decimal::from_str("4.5").unwrap()
    );
    assert!(consensus.confidence > 0.0);
}

#[tokio::test]
async fn test_consensus_expires_after_5_minutes() {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let aggregator = SignalAggregator::new(db);
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // First wallet buys — records Instant::now()
    aggregator
        .add_signal(
            "Wallet1",
            token_address,
            "BUY",
            Decimal::from_str("1.0").unwrap(),
        )
        .await;

    // Pause tokio time then advance 6 minutes so Wallet1's signal expires
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(360)).await;

    // Second wallet buys — the cleanup loop removes Wallet1 (> 5 min old)
    let result = aggregator
        .add_signal(
            "Wallet2",
            token_address,
            "BUY",
            Decimal::from_str("1.5").unwrap(),
        )
        .await;

    // Wallet1's signal was cleaned up; only Wallet2's signal exists → no consensus
    assert!(result.is_none(), "Consensus should expire after 5 minutes");
}

#[tokio::test]
async fn test_no_consensus_for_sell_signals() {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let aggregator = SignalAggregator::new(db);
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Multiple SELL signals should not trigger consensus
    aggregator
        .add_signal(
            "Wallet1",
            token_address,
            "SELL",
            Decimal::from_str("1.0").unwrap(),
        )
        .await;

    let result = aggregator
        .add_signal(
            "Wallet2",
            token_address,
            "SELL",
            Decimal::from_str("1.5").unwrap(),
        )
        .await;

    assert!(
        result.is_none(),
        "SELL signals should not trigger consensus"
    );
}

#[tokio::test]
async fn test_consensus_different_tokens() {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let aggregator = SignalAggregator::new(db);
    let token1 = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
    let token2 = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"; // USDT

    // Two wallets buying different tokens should not trigger consensus
    aggregator
        .add_signal("Wallet1", token1, "BUY", Decimal::from_str("1.0").unwrap())
        .await;

    let result = aggregator
        .add_signal("Wallet2", token2, "BUY", Decimal::from_str("1.5").unwrap())
        .await;

    assert!(
        result.is_none(),
        "Different tokens should not trigger consensus"
    );
}
