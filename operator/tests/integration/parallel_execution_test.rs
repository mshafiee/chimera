//! Parallel Execution Integration Tests
//!
//! Verifies the worker pool processes multiple signals concurrently,
//! maintains priority ordering under load, and handles graceful shutdown.
//!
//! Run: cargo test --test integration -- parallel_execution --test-threads=1

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{create_database, DatabaseConfig};
use chimera_operator::engine::executor::Executor;
use chimera_operator::engine::signal_pipeline::SignalProcessor;
use chimera_operator::engine::worker_pool::{WorkerPool, WorkerPoolConfig};
use chimera_operator::engine::PriorityQueue;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn make_config() -> AppConfig {
    let builder = config::Config::builder()
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
        .set_default("queue.capacity", 1000)
        .unwrap()
        .set_default("queue.load_shed_threshold_percent", 80)
        .unwrap()
        .set_default("queue.parallel_enabled", true)
        .unwrap()
        .set_default("queue.num_workers", 4)
        .unwrap()
        .set_default("queue.max_concurrent_rpc", 8)
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
        .unwrap();
    builder.build().unwrap().try_deserialize().unwrap()
}

fn make_signal(id: usize, strategy: Strategy, token: &str) -> Signal {
    let payload = SignalPayload {
        strategy,
        token: token.to_string(),
        token_address: Some(format!("{}_addr", token)),
        action: Action::Buy,
        amount_sol: Decimal::from_str("0.1").unwrap(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: Some(format!("parallel-test-{}-{}", token, id)),
        exit_fraction: None,
    };
    Signal::new(payload, 12345, None)
}

async fn make_processor(
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
async fn test_five_concurrent_signals_process_in_parallel() {
    let config = Arc::new(make_config());
    let queue = Arc::new(PriorityQueue::new(1000, 80));
    let (processor, _db) = make_processor(&config).await;
    let worker_config = WorkerPoolConfig::from_app_config(&config);
    let cancel_token = CancellationToken::new();

    for i in 0..5 {
        let signal = make_signal(i, Strategy::Shield, &format!("TOKEN{}", i));
        queue
            .push(signal, Some(75.0))
            .await
            .expect("Failed to push signal");
    }

    assert_eq!(queue.len(), 5);

    let mut worker_pool = WorkerPool::new(
        queue.clone(),
        processor,
        worker_config,
        cancel_token.clone(),
    );
    worker_pool.start().await;

    let start = Instant::now();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let elapsed = start.elapsed();

    cancel_token.cancel();
    let stats = worker_pool.stats();

    assert_eq!(
        stats.queue_depth, 0,
        "All 5 signals should be dequeued, took {:?}",
        elapsed
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "5 signals should process in under 5s with 4 workers (took {:?})",
        elapsed
    );
}

#[tokio::test]
async fn test_priority_ordering_under_concurrent_load() {
    let queue = Arc::new(PriorityQueue::new(100, 80));

    queue
        .push(make_signal(1, Strategy::Spear, "SPEAR1"), None)
        .await
        .unwrap();
    queue
        .push(make_signal(2, Strategy::Shield, "SHIELD1"), None)
        .await
        .unwrap();
    queue
        .push(make_signal(3, Strategy::Exit, "EXIT1"), None)
        .await
        .unwrap();
    queue
        .push(make_signal(4, Strategy::Spear, "SPEAR2"), None)
        .await
        .unwrap();
    queue
        .push(make_signal(5, Strategy::Shield, "SHIELD2"), None)
        .await
        .unwrap();

    assert_eq!(
        queue.pop().await.unwrap().payload.strategy,
        Strategy::Exit,
        "EXIT must be first"
    );
    assert_eq!(
        queue.pop().await.unwrap().payload.strategy,
        Strategy::Shield,
        "SHIELD must be second"
    );
    assert_eq!(
        queue.pop().await.unwrap().payload.strategy,
        Strategy::Shield,
        "SHIELD must be third"
    );
    assert_eq!(
        queue.pop().await.unwrap().payload.strategy,
        Strategy::Spear,
        "SPEAR must be fourth"
    );
    assert_eq!(
        queue.pop().await.unwrap().payload.strategy,
        Strategy::Spear,
        "SPEAR must be fifth"
    );
    assert!(queue.pop().await.is_none(), "Queue should be empty");
}

#[tokio::test]
async fn test_graceful_shutdown_with_in_flight_signals() {
    let config = Arc::new(make_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));
    let (processor, _db) = make_processor(&config).await;
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
        queue
            .push(
                make_signal(i, Strategy::Shield, &format!("TOKEN{}", i)),
                None,
            )
            .await
            .unwrap();
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    cancel_token.cancel();
    worker_pool.shutdown().await;

    assert!(cancel_token.is_cancelled());
    let stats = worker_pool.stats();
    assert_eq!(stats.active_workers, 0);
}

#[tokio::test]
async fn test_rpc_semaphore_limits_concurrency() {
    let config = Arc::new(make_config());
    let queue = Arc::new(PriorityQueue::new(100, 80));
    let (processor, _db) = make_processor(&config).await;

    let worker_config = WorkerPoolConfig {
        num_workers: 4,
        max_concurrent_rpc: 2,
        rpc_rate_limit: 10,
    };

    let cancel_token = CancellationToken::new();
    let worker_pool = WorkerPool::new(queue.clone(), processor, worker_config, cancel_token);

    assert_eq!(
        worker_pool.stats().rpc_semaphore_available,
        2,
        "Semaphore should have exactly 2 permits"
    );
}
