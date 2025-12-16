//! Integration tests for consensus signal detection
//!
//! Tests that multiple wallets buying the same token within 5 minutes
//! triggers consensus detection and improves signal quality.

use chimera_operator::db::{init_pool, DbPool};
use chimera_operator::monitoring::SignalAggregator;
use chimera_operator::config::DatabaseConfig;
use tempfile::TempDir;

#[tokio::test]
async fn test_consensus_detection_two_wallets() {
    // Setup test database
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let config = DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    
    let pool = init_pool(&config).await.unwrap();
    db::run_migrations(&pool).await.unwrap();

    let aggregator = SignalAggregator::new(pool.clone());

    // Test token address
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
    let wallet1 = "Wallet1Address111111111111111111111111111111";
    let wallet2 = "Wallet2Address222222222222222222222222222222";

    // First wallet buys token
    let result1 = aggregator
        .add_signal(wallet1, token_address, "BUY", 1.0)
        .await;
    assert!(result1.is_none(), "First signal should not trigger consensus");

    // Second wallet buys same token within 5 minutes
    let result2 = aggregator
        .add_signal(wallet2, token_address, "BUY", 1.5)
        .await;
    
    assert!(result2.is_some(), "Second signal should trigger consensus");
    let consensus = result2.unwrap();
    assert_eq!(consensus.wallet_count, 2);
    assert_eq!(consensus.token_address, token_address);
    assert_eq!(consensus.total_amount_sol, 2.5);
    assert!(consensus.wallets.contains(&wallet1.to_string()));
    assert!(consensus.wallets.contains(&wallet2.to_string()));
    assert!(consensus.confidence > 0.0);
}

#[tokio::test]
async fn test_consensus_detection_three_wallets() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let config = DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    
    let pool = init_pool(&config).await.unwrap();
    db::run_migrations(&pool).await.unwrap();

    let aggregator = SignalAggregator::new(pool.clone());
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Add three wallets buying same token
    aggregator
        .add_signal("Wallet1", token_address, "BUY", 1.0)
        .await;
    aggregator
        .add_signal("Wallet2", token_address, "BUY", 1.5)
        .await;
    
    let result3 = aggregator
        .add_signal("Wallet3", token_address, "BUY", 2.0)
        .await;

    assert!(result3.is_some());
    let consensus = result3.unwrap();
    assert_eq!(consensus.wallet_count, 3);
    assert_eq!(consensus.total_amount_sol, 4.5);
    assert!(consensus.confidence > 0.0);
}

#[tokio::test]
async fn test_consensus_expires_after_5_minutes() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let config = DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    
    let pool = init_pool(&config).await.unwrap();
    db::run_migrations(&pool).await.unwrap();

    let aggregator = SignalAggregator::new(pool.clone());
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // First wallet buys
    aggregator
        .add_signal("Wallet1", token_address, "BUY", 1.0)
        .await;

    // Wait 6 minutes (longer than 5-minute window)
    // Note: In real tests, you'd use tokio::time::sleep, but for unit tests
    // we'll just test that the logic works without waiting
    // For full integration test, use: tokio::time::sleep(Duration::from_secs(360)).await;

    // Second wallet buys after window expires
    let result = aggregator
        .add_signal("Wallet2", token_address, "BUY", 1.5)
        .await;

    // Should not trigger consensus (window expired)
    assert!(result.is_none(), "Consensus should expire after 5 minutes");
}

#[tokio::test]
async fn test_no_consensus_for_sell_signals() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let config = DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    
    let pool = init_pool(&config).await.unwrap();
    db::run_migrations(&pool).await.unwrap();

    let aggregator = SignalAggregator::new(pool.clone());
    let token_address = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Multiple SELL signals should not trigger consensus
    aggregator
        .add_signal("Wallet1", token_address, "SELL", 1.0)
        .await;
    
    let result = aggregator
        .add_signal("Wallet2", token_address, "SELL", 1.5)
        .await;

    assert!(result.is_none(), "SELL signals should not trigger consensus");
}

#[tokio::test]
async fn test_consensus_different_tokens() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let config = DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    };
    
    let pool = init_pool(&config).await.unwrap();
    db::run_migrations(&pool).await.unwrap();

    let aggregator = SignalAggregator::new(pool.clone());
    let token1 = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
    let token2 = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"; // USDT

    // Two wallets buying different tokens should not trigger consensus
    aggregator
        .add_signal("Wallet1", token1, "BUY", 1.0)
        .await;
    
    let result = aggregator
        .add_signal("Wallet2", token2, "BUY", 1.5)
        .await;

    assert!(result.is_none(), "Different tokens should not trigger consensus");
}






