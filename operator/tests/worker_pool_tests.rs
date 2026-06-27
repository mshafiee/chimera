//! Unit tests for worker pool parallel signal processing

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{create_database, DatabaseConfig};
use chimera_operator::engine::executor::Executor;
use chimera_operator::engine::signal_pipeline::SignalProcessor;
use chimera_operator::engine::worker_pool::{WorkerPool, WorkerPoolConfig, WorkerPoolStats};
use chimera_operator::engine::PriorityQueue;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn create_test_signal(trade_uuid: &str, strategy: Strategy, token: &str) -> Signal {
    let payload = SignalPayload {
        strategy,
        token: token.to_string(),
        token_address: Some(format!("{}_address", token)),
        action: Action::Buy,
        amount_sol: Decimal::from_str("0.1").unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: Some(trade_uuid.to_string()),
        exit_fraction: None,
    };
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
}

fn create_test_config() -> AppConfig {
    let config = config::Config::builder()
        .set_default("server.host", "0.0.0.0")
        .unwrap()
        .set_default("server.port", 8080)
        .unwrap()
        .set_default("server.request_timeout_ms", 30000)
        .unwrap()
        .set_default("rpc.primary_provider", "helius")
        .unwrap()
        .set_default("rpc.primary_url", "https://api.mainnet-beta.solana.com")
        .unwrap()
        .set_default("rpc.rate_limit_per_second", 40)
        .unwrap()
        .set_default("rpc.timeout_ms", 2000)
        .unwrap()
        .set_default("rpc.max_consecutive_failures", 3)
        .unwrap()
        .set_default("database.path", "data/chimera.db")
        .unwrap()
        .set_default("database.max_connections", 5)
        .unwrap()
        .set_default(
            "security.webhook_secret",
            "test-secret-that-is-thirty-two-chars-long!!",
        )
        .unwrap()
        .set_default("security.max_timestamp_drift_secs", 60)
        .unwrap()
        .set_default("security.webhook_rate_limit", 100)
        .unwrap()
        .set_default("security.webhook_burst_size", 150)
        .unwrap()
        .set_default("queue.capacity", 100)
        .unwrap()
        .set_default("queue.load_shed_threshold_percent", 80)
        .unwrap()
        .set_default("queue.parallel_enabled", true)
        .unwrap()
        .set_default("queue.num_workers", 2)
        .unwrap()
        .set_default("queue.max_concurrent_rpc", 4)
        .unwrap()
        .set_default("strategy.shield_percent", 70)
        .unwrap()
        .set_default("strategy.spear_percent", 30)
        .unwrap()
        .set_default("strategy.max_position_sol", "1.0")
        .unwrap()
        .set_default("strategy.min_position_sol", "0.01")
        .unwrap()
        .set_default("jito.enabled", false)
        .unwrap()
        .set_default("jito.tip_floor_sol", "0.001")
        .unwrap()
        .set_default("jito.tip_ceiling_sol", "0.01")
        .unwrap()
        .set_default("jito.tip_percentile", 50)
        .unwrap()
        .set_default("jito.tip_percent_max", "0.10")
        .unwrap()
        .set_default("circuit_breakers.max_loss_24h_usd", "500.0")
        .unwrap()
        .set_default("circuit_breakers.max_consecutive_losses", 5)
        .unwrap()
        .set_default("circuit_breakers.max_drawdown_percent", "15.0")
        .unwrap()
        .set_default("circuit_breakers.portfolio_stop_loss_percent", "5.0")
        .unwrap()
        .set_default("circuit_breakers.cooldown_minutes", 30)
        .unwrap()
        .build()
        .unwrap();

    config.try_deserialize().unwrap()
}

async fn create_signal_processor(
    config: &Arc<AppConfig>,
) -> (
    SignalProcessor,
    Arc<dyn chimera_operator::db_abstraction::Database>,
) {
    let db_cfg = DatabaseConfig::sqlite(std::path::PathBuf::from(":memory:"));
    let db = create_database(&db_cfg)
        .await
        .expect("Failed to create test DB");
    let executor = Arc::new(RwLock::new(Executor::new((*config).clone(), db.clone())));
    let processor = SignalProcessor::new(
        db.clone(),
        executor,
        (*config).clone(),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    (processor, db)
}

#[tokio::test]
async fn test_worker_pool_config_from_app_config() {
    let config = create_test_config();
    let worker_config = WorkerPoolConfig::from_app_config(&config);

    assert_eq!(worker_config.num_workers, 2);
    assert_eq!(worker_config.max_concurrent_rpc, 4);
    assert!(worker_config.rpc_rate_limit > 0);
}

#[tokio::test]
async fn test_worker_pool_creation() {
    let config = Arc::new(create_test_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));
    let (processor, _db) = create_signal_processor(&config).await;

    let worker_config = WorkerPoolConfig::from_app_config(&config);
    let cancel_token = CancellationToken::new();

    let worker_pool = WorkerPool::new(queue.clone(), processor, worker_config, cancel_token);

    let stats = worker_pool.stats();
    assert_eq!(stats.active_workers, 0);
    assert_eq!(stats.queue_depth, 0);
}

#[tokio::test]
async fn test_concurrent_signal_processing() {
    let config = Arc::new(create_test_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));

    for i in 0..10 {
        let signal = create_test_signal(
            &format!("trade_{}", i),
            Strategy::Shield,
            &format!("TOKEN{}", i),
        );
        queue
            .push(signal, Some(75.0))
            .await
            .expect("Failed to push signal");
    }

    assert_eq!(queue.len(), 10);

    let (processor, _db) = create_signal_processor(&config).await;
    let worker_config = WorkerPoolConfig::from_app_config(&config);
    let cancel_token = CancellationToken::new();

    let mut worker_pool = WorkerPool::new(
        queue.clone(),
        processor,
        worker_config,
        cancel_token.clone(),
    );

    worker_pool.start().await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    cancel_token.cancel();

    let stats = worker_pool.stats();
    assert_eq!(stats.queue_depth, 0);
}

#[tokio::test]
async fn test_priority_preservation() {
    let queue = Arc::new(PriorityQueue::new(100, 80));

    let spear_signal = create_test_signal("trade_spear", Strategy::Spear, "SPEAR");
    let shield_signal = create_test_signal("trade_shield", Strategy::Shield, "SHIELD");
    let exit_signal = create_test_signal("trade_exit", Strategy::Exit, "EXIT");

    queue
        .push(spear_signal, Some(60.0))
        .await
        .expect("Failed to push SPEAR");
    queue
        .push(shield_signal, None)
        .await
        .expect("Failed to push SHIELD");
    queue
        .push(exit_signal, None)
        .await
        .expect("Failed to push EXIT");

    let first = queue.pop().await;
    assert!(first.is_some());
    assert_eq!(first.unwrap().payload.strategy, Strategy::Exit);

    let second = queue.pop().await;
    assert!(second.is_some());
    assert_eq!(second.unwrap().payload.strategy, Strategy::Shield);

    let third = queue.pop().await;
    assert!(third.is_some());
    assert_eq!(third.unwrap().payload.strategy, Strategy::Spear);
}

#[tokio::test]
async fn test_rate_limiting() {
    let config = Arc::new(create_test_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));

    let (processor, _db) = create_signal_processor(&config).await;

    let worker_config = WorkerPoolConfig {
        num_workers: 4,
        max_concurrent_rpc: 2,
        rpc_rate_limit: 10,
    };

    let cancel_token = CancellationToken::new();

    let worker_pool = WorkerPool::new(queue.clone(), processor, worker_config, cancel_token);

    let stats = worker_pool.stats();
    assert_eq!(stats.rpc_semaphore_available, 2);
}

#[tokio::test]
async fn test_database_concurrency() {
    let config = Arc::new(create_test_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));

    let (processor, _db) = create_signal_processor(&config).await;

    let worker_config = WorkerPoolConfig {
        num_workers: 4,
        max_concurrent_rpc: 8,
        rpc_rate_limit: 40,
    };

    let cancel_token = CancellationToken::new();

    let mut worker_pool = WorkerPool::new(
        queue.clone(),
        processor,
        worker_config,
        cancel_token.clone(),
    );

    worker_pool.start().await;

    for i in 0..5 {
        let signal = create_test_signal(
            &format!("concurrent_trade_{}", i),
            Strategy::Shield,
            &format!("TOKEN{}", i),
        );
        queue
            .push(signal, Some(75.0))
            .await
            .expect("Failed to push signal");
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    cancel_token.cancel();

    let stats = worker_pool.stats();
    assert_eq!(stats.queue_depth, 0);
}

#[tokio::test]
async fn test_worker_pool_stats() {
    let stats = WorkerPoolStats {
        active_workers: 3,
        queue_depth: 10,
        rpc_semaphore_available: 5,
    };

    let display_string = format!("{}", stats);
    assert!(display_string.contains("active: 3"));
    assert!(display_string.contains("queue_depth: 10"));
    assert!(display_string.contains("rpc_permits: 5"));
}

#[tokio::test]
async fn test_worker_pool_shutdown() {
    let config = Arc::new(create_test_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));

    let (processor, _db) = create_signal_processor(&config).await;
    let worker_config = WorkerPoolConfig::from_app_config(&config);
    let cancel_token = CancellationToken::new();

    let mut worker_pool = WorkerPool::new(
        queue.clone(),
        processor,
        worker_config,
        cancel_token.clone(),
    );

    worker_pool.start().await;

    for i in 0..3 {
        let signal = create_test_signal(
            &format!("shutdown_trade_{}", i),
            Strategy::Shield,
            &format!("TOKEN{}", i),
        );
        queue
            .push(signal, Some(75.0))
            .await
            .expect("Failed to push signal");
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    cancel_token.cancel();
    worker_pool.shutdown().await;

    assert!(cancel_token.is_cancelled());
    let stats = worker_pool.stats();
    assert_eq!(stats.active_workers, 0);
}
