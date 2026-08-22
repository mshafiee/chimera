//! Engine + EngineHandle coverage tests (engine/mod.rs).
//!
//! Covers the handle accessors, every `Engine::new*` constructor chain, the
//! execution-lock wiring, and the sequential/parallel run loops (including
//! graceful shutdown via the cancellation token).

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::worker_pool::WorkerPoolConfig;
use chimera_operator::engine::{
    Engine, EngineHandle, PortfolioHeat, SelectionConfig, SelectionService,
};
use chimera_operator::experiment::ToxicFlowDetector;
use chimera_operator::handlers::WsState;
use chimera_operator::metrics::MetricsState;
use chimera_operator::middleware::Role;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use chimera_operator::monitoring::WalletPerformanceTracker;
use chimera_operator::notifications::CompositeNotifier;
use chimera_operator::price_cache::PriceCache;
use chimera_operator::state::{AsyncWriteQueue, StateRegistry};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::TipManager;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn base_config() -> AppConfig {
    let mut config = AppConfig::default();
    // Fast-fail RPC instead of mainnet.
    config.rpc.primary_url = "http://127.0.0.1:1".to_string();
    config
}

fn make_signal(strategy: Strategy, token_address: &str) -> Signal {
    Signal {
        trade_uuid: format!("t-{}", uuid::Uuid::new_v4()),
        payload: SignalPayload {
            strategy,
            token: "TEST".to_string(),
            token_address: Some(token_address.to_string()),
            action: Action::Buy,
            amount_sol: dec("0.1"),
            wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            trade_uuid: None,
            exit_fraction: None,
        },
        timestamp: chrono::Utc::now().timestamp(),
        source_ip: Some("127.0.0.1".to_string()),
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals: None,
    }
}

fn make_token_parser(db: &Arc<dyn Database>) -> Arc<TokenParser> {
    let _ = db;
    let cache = Arc::new(TokenCache::new(100, 300));
    let fetcher = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        "http://127.0.0.1:1",
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    Arc::new(TokenParser::new(
        TokenSafetyConfig::default(),
        cache,
        fetcher,
    ))
}

fn make_selection_service(
    db: Arc<dyn Database>,
    parser: Arc<TokenParser>,
) -> Arc<SelectionService> {
    let config = SelectionConfig {
        total_capital_sol: dec("10"),
        max_position_sol: dec("5"),
        shield_signal_quality_threshold: 0.55,
        spear_signal_quality_threshold: 0.30,
        shield_percent: 60,
        spear_percent: 40,
        min_liquidity_shield_usd: dec("10000"),
        min_liquidity_spear_usd: dec("5000"),
        min_liquidity_pumpfun_usd: dec("25000"),
        allow_graduated_pumpfun: true,
        min_token_age_hours: 1.0,
        min_token_age_pumpfun_hours: 4.0,
        min_token_age_proven_hours: 0.1,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven: false,
        min_proven_trades: 10,
        require_proven_positive_pnl: false,
        mirror_gate_enabled: false,
        mirror_gate_min_avg_pct: dec("1.5"),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
        wallet_tstat_enabled: false,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
        shadow_proven_enabled: false,
        shadow_proven_min_samples: 20,
        shadow_proven_min_total_pnl_sol: 2.0,
        token_velocity_gate_enabled: false,
        token_min_liquidity_velocity: 0.10,
        token_max_curve_completion: 0.85,
        cluster_gate_enabled: false,
        cluster_min_profitable_wallets: 3,
        averaging_down_enabled: false,
        averaging_down_window_hours: 12,
        averaging_down_min_buys: 2,
        averaging_down_min_drop_pct: dec("3.0"),
        pump_chase_enabled: false,
        pump_chase_max_delta_pct: dec("10.0"),
        stop_loss_cooldown_enabled: false,
        stop_loss_cooldown_hours: 12,
        stop_loss_cooldown_loss_pct: dec("5.0"),
        pump_since_whale_guard_enabled: true,
        max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
        repeat_signal_gate_enabled: true,
        repeat_signal_min_prior: 1,
        entry_drift_guard_enabled: true,
        max_entry_drift_pct: rust_decimal::Decimal::new(30, 1),
        momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
        momentum_bypass_enabled: false,
        wqs_proven_waiver_enabled: true,
    };
    Arc::new(SelectionService::new(
        db, parser, None, None, None, None, None, config,
    ))
}

#[tokio::test]
async fn test_engine_handle_accessors_and_queue() {
    let (db, _guard) = create_test_db().await;
    let mut config = base_config();
    config.queue.parallel_enabled = false;
    config.queue.capacity = 10;

    let (engine, handle) = Engine::new(config, db.clone());
    let _engine_holder = engine; // engine task not run here

    assert_eq!(handle.queue_depth(), 0);

    // Medium (SHIELD) signal.
    handle
        .queue_signal(
            make_signal(
                Strategy::Shield,
                "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            ),
            None,
        )
        .await
        .unwrap();
    // High (EXIT) signal — always admitted.
    let mut exit = make_signal(
        Strategy::Exit,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
    );
    exit.payload.action = Action::Sell;
    handle.queue_signal(exit, None).await.unwrap();
    // Low (SPEAR) signal with WQS < 70 → regular queue.
    handle
        .queue_signal(
            make_signal(
                Strategy::Spear,
                "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            ),
            Some(50.0),
        )
        .await
        .unwrap();
    // High-WQS SPEAR → dedicated queue.
    handle
        .queue_signal(
            make_signal(
                Strategy::Spear,
                "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            ),
            Some(80.0),
        )
        .await
        .unwrap();
    assert_eq!(handle.queue_depth(), 4);

    // Load shedding: default threshold 80% of capacity 10 = 8 in-flight;
    // the 9th low-WQS push is shed.
    for _ in 0..4 {
        handle
            .queue_signal(
                make_signal(
                    Strategy::Shield,
                    "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
                ),
                None,
            )
            .await
            .unwrap();
    }
    let err = handle
        .queue_signal(
            make_signal(
                Strategy::Spear,
                "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            ),
            Some(50.0),
        )
        .await
        .unwrap_err();
    let err_lower = err.to_lowercase();
    assert!(
        err_lower.contains("load") || err_lower.contains("capacity"),
        "{err}"
    );
    // EXIT signals bypass load shedding entirely.
    let mut exit2 = make_signal(
        Strategy::Exit,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
    );
    exit2.payload.action = Action::Sell;
    handle.queue_signal(exit2, None).await.unwrap();

    // RPC accessors (executor wired).
    assert!(matches!(
        handle.rpc_mode(),
        chimera_operator::engine::executor::RpcMode::Standard
            | chimera_operator::engine::executor::RpcMode::Jito
    ));
    assert!(!handle.is_in_fallback());
    assert!(handle.active_rpc_client().await.is_some());

    // Health cache is empty until a health check runs; the call itself must not error.
    let _health = handle.get_rpc_health().await;
    handle.refresh_rpc_health().await;
    let _fallback = handle.fallback_duration().await;
    // Shutdown cancels the token.
    handle.shutdown();
}

#[tokio::test]
async fn test_engine_all_constructor_chains() {
    let (db, _guard) = create_test_db().await;
    let notifier = Arc::new(CompositeNotifier::new());
    let metrics = Arc::new(MetricsState::new().expect("metrics"));
    let ws = Arc::new(WsState::new(HashMap::new(), "secret".to_string(), false));
    let tip_manager = Arc::new(TipManager::new(base_config().jito.clone(), db.clone()));
    let price_cache = Arc::new(PriceCache::new().expect("price cache"));
    let token_parser = make_token_parser(&db);
    let heat = Arc::new(PortfolioHeat::new(db.clone(), dec("1000")));
    let registry = Arc::new(StateRegistry::new());
    let write_queue = Arc::new(AsyncWriteQueue::new(
        db.clone(),
        Default::default(),
        Default::default(),
    ));
    let wallet_perf = Arc::new(WalletPerformanceTracker::new(db.clone()));
    let toxic = Arc::new(ToxicFlowDetector::new(Default::default()));
    let verdict = Arc::new(tokio::sync::RwLock::new(None));

    let mut config = base_config();
    config.execution_lock.enabled = true;
    config.execution_lock.cleanup_interval_seconds = 1;

    // Simplest constructor (execution lock DISABLED → the no-lock branches).
    let mut no_lock = base_config();
    no_lock.execution_lock.enabled = false;
    let (engine, _h) = Engine::new_with_extras(
        no_lock.clone(),
        db.clone(),
        Arc::new(CompositeNotifier::new()),
        None,
        None,
    );
    drop(engine);
    let (engine, _h) = Engine::new(no_lock, db.clone());
    drop(engine);
    let (engine, _h) = Engine::new_with_notifier(config.clone(), db.clone(), notifier.clone());
    drop(engine);
    let (engine, _h) = Engine::new_with_notifier_and_metrics(
        config.clone(),
        db.clone(),
        notifier.clone(),
        Some(metrics.clone()),
    );
    drop(engine);
    let (engine, _h) = Engine::new_with_extras(
        config.clone(),
        db.clone(),
        notifier.clone(),
        Some(metrics.clone()),
        Some(ws.clone()),
    );
    drop(engine);
    let (engine, _h) = Engine::new_with_extras_and_tip_manager(
        config.clone(),
        db.clone(),
        notifier.clone(),
        Some(metrics.clone()),
        Some(ws.clone()),
        Some(tip_manager.clone()),
    );
    drop(engine);
    let (engine, _h) = Engine::new_with_extras_tip_manager_and_price_cache(
        config.clone(),
        db.clone(),
        notifier.clone(),
        Some(metrics.clone()),
        Some(ws.clone()),
        Some(tip_manager.clone()),
        Some(price_cache.clone()),
    );
    drop(engine);

    // Full constructor: every optional extras slot populated.
    let (engine, handle) = Engine::new_with_extras_tip_manager_price_cache_and_token_parser(
        config.clone(),
        db.clone(),
        notifier,
        Some(metrics),
        Some(ws),
        Some(tip_manager),
        Some(price_cache),
        Some(token_parser),
        Some(heat),
        Some(registry),
        Some(write_queue),
        Some(wallet_perf),
        Some(toxic),
        Some(verdict),
    );
    let selection = make_selection_service(db.clone(), make_token_parser(&db));
    let _selection = selection;
    assert!(handle.queue_depth() == 0);
    drop(engine);

    // Engine with parallel mode + execution lock → run_parallel shuts down
    // cleanly on cancellation.
    let mut par_config = base_config();
    par_config.queue.parallel_enabled = true;
    par_config.execution_lock.enabled = true;
    par_config.execution_lock.cleanup_interval_seconds = 1;
    let (engine, handle) = Engine::new_with_extras(
        par_config,
        db.clone(),
        Arc::new(CompositeNotifier::new()),
        Some(Arc::new(MetricsState::new().expect("metrics"))),
        None,
    );
    // No queued signals: the worker pool idles, the background tasks (metrics
    // updater, execution-lock cleanup, Jito health check) run their first
    // immediate ticks, and cancellation shuts everything down.
    let run = tokio::spawn(async move { engine.run().await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("parallel run returns");
}

#[tokio::test]
async fn test_engine_run_sequential_shutdown_and_signal_processing() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);

    // A wallet row so process_signal has something to look up.
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, win_rate) VALUES ($1, 'ACTIVE', 80.0, 0.5) \
         ON CONFLICT (address) DO NOTHING",
    )
    .bind("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")
    .execute(&pool)
    .await
    .unwrap();

    let mut config = base_config();
    config.queue.parallel_enabled = false;
    let (engine, handle) = Engine::new_with_extras(
        config.clone(),
        db.clone(),
        Arc::new(CompositeNotifier::new()),
        Some(Arc::new(MetricsState::new().expect("metrics"))),
        None,
    );
    handle
        .queue_signal(
            make_signal(
                Strategy::Shield,
                "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
            ),
            None,
        )
        .await
        .unwrap();

    let run = tokio::spawn(async move { engine.run().await });
    // Let the loop pop and process the signal (its internal RPC calls
    // fast-fail against 127.0.0.1:1), then cancel for a clean exit.
    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("sequential run returns on shutdown");
}

#[tokio::test]
async fn test_engine_run_sequential_cancelled_before_start() {
    let (db, _guard) = create_test_db().await;
    let mut config = base_config();
    config.queue.parallel_enabled = false;
    let (engine, handle) = Engine::new(config, db.clone());
    handle.shutdown();
    // First loop iteration observes the cancelled token and exits immediately.
    tokio::time::timeout(Duration::from_secs(10), engine.run())
        .await
        .expect("pre-cancelled engine exits immediately");
}

#[tokio::test]
async fn test_worker_pool_config_from_app_config() {
    let config = base_config();
    let wpc = WorkerPoolConfig::from_app_config(&Arc::new(config));
    assert!(wpc.num_workers >= 1);
}
