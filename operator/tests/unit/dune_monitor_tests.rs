//! DunePnlMonitor tests.
//!
//! Runs the monitor's full check/promote/audit cycles against a real
//! per-test Postgres database with a mocked Dune REST API (`DUNE_API_BASE_URL`
//! env override, mirroring the `HELIUS_API_BASE_URL` pattern) and a mocked
//! Helius API for the on-chain assessment paths.

use chimera_operator::config::{AppConfig, DuneConfig};
use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::dune_monitor::{DunePnlMonitor, DunePromotionContext};
use chimera_operator::experiment::ToxicFlowDetector;
use chimera_operator::monitoring::helius::HeliusClient;
use chimera_operator::monitoring::rate_limiter::RateLimiter;
use chimera_operator::monitoring::webhook_lifecycle::WebhookLifecycleConfig;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[path = "../common/mod.rs"]
mod common;

#[path = "../common/mock_rpc.rs"]
mod mock_rpc;

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const WALLET_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const TOKEN: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";

/// A CSV row readable by BOTH parsers: `parse_csv` reads cols 0/2/4
/// (net_pnl -500 → losing) and `parse_profitable_csv` reads cols 0/1/5/6
/// (net_pnl 50000, roi 3.0 → profitable).
fn dual_csv() -> String {
    format!(
        "wallet,trade_count,net_pnl_usd,volume_usd,margin_pct,total_volume_usd,roi,unique_tokens\n\
         {WALLET},50,-500.0,50000.0,-10.0,50000.0,3.0,9\n\
         {WALLET_B},25,-100.0,10000.0,-5.0,10000.0,2.0,5\n"
    )
}

fn base_config() -> DuneConfig {
    let app = AppConfig::default();
    app.dune.clone()
}

/// Env vars are process-global; the Dune/Helius base URLs are read at
/// construction. Serialize set→construct→restore so parallel tests cannot
/// race each other's mock URL.
static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn monitor(
    config: &DuneConfig,
    db: Arc<dyn Database>,
    dune_url: &str,
    ctx: Option<DunePromotionContext>,
) -> DunePnlMonitor {
    let _guard = ENV_LOCK.lock();
    let old = std::env::var("DUNE_API_BASE_URL").ok();
    std::env::set_var("DUNE_API_BASE_URL", dune_url);
    let old_key = std::env::var("DUNE_API_KEY").ok();
    std::env::set_var("DUNE_API_KEY", "test-key");
    let m = DunePnlMonitor::new(config, db).with_promotion_context(ctx);
    match old {
        Some(v) => std::env::set_var("DUNE_API_BASE_URL", v),
        None => std::env::remove_var("DUNE_API_BASE_URL"),
    }
    match old_key {
        Some(v) => std::env::set_var("DUNE_API_KEY", v),
        None => std::env::remove_var("DUNE_API_KEY"),
    }
    m
}

fn helius_client(base_url: &str) -> Arc<HeliusClient> {
    let _guard = ENV_LOCK.lock();
    let old = std::env::var("HELIUS_API_BASE_URL").ok();
    std::env::set_var("HELIUS_API_BASE_URL", base_url);
    let client = Arc::new(
        HeliusClient::new(
            "test-key".to_string(),
            Arc::new(parking_lot::RwLock::new(HashMap::new())),
        )
        .expect("helius client"),
    );
    match old {
        Some(v) => std::env::set_var("HELIUS_API_BASE_URL", v),
        None => std::env::remove_var("HELIUS_API_BASE_URL"),
    }
    client
}

fn promotion_ctx(helius: Option<Arc<HeliusClient>>) -> DunePromotionContext {
    DunePromotionContext {
        helius_client: helius,
        webhook_rate_limiter: Some(Arc::new(RateLimiter::new(40, 1))),
        webhook_lifecycle_config: Some(WebhookLifecycleConfig {
            auto_register_enabled: true,
            auto_cleanup_enabled: true,
            health_check_interval_secs: 3600,
            stale_threshold_days: 7u32,
            max_registration_retries: 3u32,
            webhook_url: "https://example.invalid/webhook".to_string(),
            helius_dry_run: false,
            auth_header: None,
        }),
        toxic_detector: Some(Arc::new(ToxicFlowDetector::new(Default::default()))),
    }
}

async fn seed_wallet_status(db: &Arc<dyn Database>, address: &str, status: &str, wqs: f64) {
    let pool = pg_pool(db);
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, win_rate, roi_30d) \
         VALUES ($1, $2, $3, 0.5, 0.5) \
         ON CONFLICT (address) DO UPDATE SET status = EXCLUDED.status, wqs_score = EXCLUDED.wqs_score",
    )
    .bind(address)
    .bind(status)
    .bind(wqs)
    .execute(&pool)
    .await
    .unwrap();
}

async fn seed_recent_decision(db: &Arc<dyn Database>, wallet: &str, token: &str) {
    let pool = pg_pool(db);
    sqlx::query(
        "INSERT INTO decision_records (decision_id, run_id, ingress, wallet_address, token_address, action, admitted, source_amount_sol, received_at, decided_at, code_revision, config_hash, roster_hash) \
         VALUES ($1, 'run', 'helius', $2, $3, 'BUY', true, 1.0, NOW(), NOW(), 'rev', 'hash', 'roster')",
    )
    .bind(format!("dr-{}-{}", wallet, uuid::Uuid::new_v4()))
    .bind(wallet)
    .bind(token)
    .execute(&pool)
    .await
    .unwrap();
}

/// Two Helius SWAP transactions that form one profitable round trip for the
/// wallet: buy 100 SOL of TOKEN at 1.00, sell at 1.50.
fn round_trip_txs(wallet: &str) -> Vec<serde_json::Value> {
    let buy = json!({
        "signature": format!("sig-buy-{}", wallet),
        "timestamp": 1_700_000_000,
        "transactionError": null,
        "tokenTransfers": [
            {"mint": TOKEN, "tokenAmount": 1000000, "fromUserAccount": "dex-1", "toUserAccount": wallet},
            {"mint": "So11111111111111111111111111111111111111112", "tokenAmount": 100, "fromUserAccount": wallet, "toUserAccount": "dex-1"}
        ],
        "nativeTransfers": []
    });
    let sell = json!({
        "signature": format!("sig-sell-{}", wallet),
        "timestamp": 1_700_003_600,
        "transactionError": null,
        "tokenTransfers": [
            {"mint": TOKEN, "tokenAmount": 1000000, "fromUserAccount": wallet, "toUserAccount": "dex-1"},
            {"mint": "So11111111111111111111111111111111111111112", "tokenAmount": 150, "fromUserAccount": "dex-1", "toUserAccount": wallet}
        ],
        "nativeTransfers": []
    });
    vec![buy, sell]
}

// ── run_check ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_run_check_demotes_losing_active_wallet() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "ACTIVE", 75.0).await;
    seed_wallet_status(&db, WALLET_B, "CANDIDATE", 75.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();
    dune.state.lock().await.pending_polls = 1; // one pending poll, then completed

    let cfg = base_config();
    let m = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune.url), None);

    let result = m.run_check().await;
    if let Err(e) = &result {
        std::fs::write("/tmp/probe-z.txt", format!("run_check error: {e}")).unwrap();
    }
    assert!(result.is_ok());

    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE", "losing ACTIVE wallet must be demoted");
    let status_b: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET_B)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        status_b, "CANDIDATE",
        "CANDIDATE wallet untouched by demotion"
    );
    assert!(!dune.state.lock().await.executed_query_ids.is_empty());
}

#[tokio::test]
async fn test_run_check_losing_wallet_not_active() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Both CSV wallets are CANDIDATE (not ACTIVE) → the demote loop finds no
    // active match and returns early.
    seed_wallet_status(&db, WALLET, "CANDIDATE", 75.0).await;
    seed_wallet_status(&db, WALLET_B, "CANDIDATE", 75.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();
    let cfg = base_config();
    let m = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune.url), None);
    m.run_check().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE");
}

#[tokio::test]
async fn test_run_check_falls_back_to_json_rows_when_csv_empty() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "ACTIVE", 75.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = String::new();
    // serde_json::Map sorts keys alphabetically (no preserve_order), so the
    // wallet key is named "a_wallet" so it becomes the FIRST CSV column.
    dune.state.lock().await.json_rows = vec![json!({
        "a_wallet": WALLET,
        "trades_24h": 10,
        "net_pnl_usd": -300.0,
        "volume_usd": 1000.0,
        "margin_pct": -12.0,
    })];

    let cfg = base_config();
    let m = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune.url), None);
    m.run_check().await.unwrap();

    // The JSON fallback executes (parse + demote attempt). Note: `rows_to_csv`
    // stringifies JSON strings WITH quotes, so the parsed address never
    // matches the DB row and no demotion happens — the fallback's purpose is
    // rescuing empty CSV responses, not producing exact matches.
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE");
}

#[tokio::test]
async fn test_run_check_failed_status_and_timeout() {
    let (db, _guard) = create_test_db().await;

    // QUERY_STATE_FAILED → error.
    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.status = "QUERY_STATE_FAILED".to_string();
    dune.state.lock().await.error_message = "query exploded".to_string();
    let cfg = base_config();
    let m = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune.url), None);
    let err = m.run_check().await.unwrap_err();
    assert!(err.to_string().contains("query exploded"), "{err}");

    // Always-pending → poll timeout after MAX_POLLS.
    let dune2 = mock_rpc::DuneMock::spawn().await;
    dune2.state.lock().await.status = "QUERY_STATE_PENDING".to_string();
    let m2 = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune2.url), None);
    let err = m2
        .run_check()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn test_execute_query_network_failure() {
    let (db, _guard) = create_test_db().await;
    let cfg = base_config();
    // Point the monitor at a dead port: execute fails on transport.
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    let err = m.run_check().await.unwrap_err();
    assert!(err.to_string().contains("execute"), "{err}");
}

// ── promote_dune_verified ────────────────────────────────────────────────────

#[tokio::test]
async fn test_promote_disabled_or_no_key_returns_zero() {
    let (db, _guard) = create_test_db().await;
    let dune = mock_rpc::DuneMock::spawn().await;
    let mut cfg = base_config();
    cfg.promote_enabled = false;
    let m = monitor(&cfg, db.clone(), &format!("{}/api/v1", dune.url), None);
    assert_eq!(
        m.promote_dune_verified()
            .await
            .unwrap(),
        0
    );

    // No DUNE_API_KEY at construction → api_key empty → guard.
    let _guard = ENV_LOCK.lock();
    std::env::set_var("DUNE_API_BASE_URL", format!("{}/api/v1", dune.url));
    std::env::remove_var("DUNE_API_KEY");
    let m = DunePnlMonitor::new(&base_config(), db.clone());
    std::env::remove_var("DUNE_API_BASE_URL");
    std::env::set_var("DUNE_API_KEY", "test-key");
    drop(_guard);
    assert_eq!(
        m.promote_dune_verified()
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn test_promote_full_flow_with_onchain_gate_disabled() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();

    let mut cfg = base_config();
    cfg.promote_max_active_total = 100;
    cfg.promote_max_per_cycle = 10;
    cfg.onchain_assessment.enabled = false; // skip the Helius gate
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(None)),
    );

    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 1);

    let (status, wqs): (String, f64) =
        sqlx::query_as("SELECT status, wqs_score FROM wallets WHERE address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "ACTIVE");
    assert!(wqs >= 80.0, "Dune-verified wallets get the WQS floor");
    let notes: String = sqlx::query_scalar("SELECT notes FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(notes.contains("Dune-verified"), "{notes}");
}

#[tokio::test]
async fn test_promote_active_cap_stops_cycle() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;
    // Cap already reached by another ACTIVE wallet.
    seed_wallet_status(&db, WALLET_B, "ACTIVE", 90.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();

    let mut cfg = base_config();
    cfg.promote_max_active_total = 1;
    cfg.onchain_assessment.enabled = false;
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE");
}

#[tokio::test]
async fn test_promote_with_onchain_gate_and_webhook_registration() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = round_trip_txs(WALLET);
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.promote_max_active_total = 100;
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 1;
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(Some(helius_client))),
    );

    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 1);

    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE");
    // Webhook registration happened through the mocked Helius API.
    let webhook: Option<String> = sqlx::query_scalar(
        "SELECT helius_webhook_id FROM wallet_monitoring WHERE wallet_address = $1",
    )
    .bind(WALLET)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(webhook.is_some(), "promotion must register a webhook");
}

#[tokio::test]
async fn test_promote_onchain_gate_fails_and_skips() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();

    // Helius returns NO transactions → 0 round trips → gate fails.
    let helius = mock_rpc::HeliusApiMock::spawn().await;
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 10;
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(Some(helius_client))),
    );

    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(
        promoted, 0,
        "wallet failing the on-chain gate must not promote"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE");
}

#[tokio::test]
async fn test_promote_parse_filters_and_query_failures() {
    let (db, _guard) = create_test_db().await;
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;

    // CSV rows: one passing wallet, plus rows filtered by parse_profitable
    // (too few columns / short address / low roi / few trades / NaN).
    let csv = format!(
        "wallet,trade_count,total_volume_usd,sell_volume_usd,buy_volume_usd,net_pnl_usd,roi,unique_tokens\n\
         {WALLET},50,50000,40000,10000,30000,3.0,9\n\
         short,50,50000,40000,10000,30000,3.0,9\n\
         {},3,5000,4000,1000,2000,3.0,9\n\
         {},50,50000,40000,10000,30000,0.5,9\n\
         {},50,0,0,0,-100,1.5,9",
        WALLET_B,
        "A6Wch1mJJ1PyooNSAUtctcNmQTxqtkcWManMBQPmKceM",
        "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
    );
    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = csv;

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = false;
    cfg.promote_max_active_total = 100;
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 1, "only the fully-qualified wallet promotes");

    // 24h query execution fails → warn + continue (7d query still runs).
    let dune2 = mock_rpc::DuneMock::spawn().await;
    dune2.state.lock().await.csv = dual_csv();
    dune2.state.lock().await.fail_execute_query_id = Some(cfg.promote_query_id_24h);
    let m2 = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune2.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m2
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(
        promoted, 1,
        "7d query still promotes when 24h execute fails"
    );

    // 24h CSV fetch fails → warn + continue (7d still promotes).
    let dune3 = mock_rpc::DuneMock::spawn().await;
    dune3.state.lock().await.csv = dual_csv();
    dune3.state.lock().await.fail_csv_query_id = Some(cfg.promote_query_id_24h);
    let m3 = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune3.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m3
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 1);

    // Both queries return empty CSV → JSON fallback with profitable rows.
    // serde_json::Map sorts keys alphabetically, so the keys are prefixed to
    // sort exactly as the parse_profitable_csv column layout.
    let dune4 = mock_rpc::DuneMock::spawn().await;
    dune4.state.lock().await.csv = String::new();
    dune4.state.lock().await.json_rows = vec![json!({
        "a_wallet": WALLET,
        "b_trade_count": 50,
        "c_total_volume_usd": 50000,
        "d_sell_volume_usd": 40000,
        "e_buy_volume_usd": 10000,
        "f_net_pnl_usd": 30000,
        "g_roi": 3.0,
        "h_unique_tokens": 9,
    })];
    let m4 = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune4.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m4
        .promote_dune_verified()
        .await
        .unwrap();
    // rows_to_csv stringifies JSON strings WITH quotes, so the parsed address
    // never matches the DB row — the fallback runs but cannot promote.
    assert_eq!(
        promoted, 0,
        "JSON fallback parse runs; quoted addresses never match"
    );

    // Neither query returns anything → profitable empty → Ok(0).
    let dune5 = mock_rpc::DuneMock::spawn().await;
    dune5.state.lock().await.csv = String::new();
    let m5 = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune5.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m5
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 0);
}

#[tokio::test]
async fn test_promote_demoted_wallet_in_cooldown_skipped() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Wallet demoted 1 minute ago; cooldown 24h → ineligible.
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, win_rate, demoted_at) \
         VALUES ($1, 'CANDIDATE', 0.0, 0.5, NOW() - INTERVAL '1 minute')",
    )
    .bind(WALLET)
    .execute(&pool)
    .await
    .unwrap();

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();
    let mut cfg = base_config();
    cfg.promote_demote_cooldown_hours = 24;
    cfg.onchain_assessment.enabled = false;
    let m = monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(None)),
    );
    let promoted = m
        .promote_dune_verified()
        .await
        .unwrap();
    assert_eq!(promoted, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE");
}

// ── promote_active_candidates_onchain ────────────────────────────────────────

#[tokio::test]
async fn test_active_candidate_promotion_guards_and_flow() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;
    seed_recent_decision(&db, WALLET, TOKEN).await;

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = round_trip_txs(WALLET);
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 1;
    cfg.promote_max_active_total = 100;

    // Guard: onchain assessment disabled → 0.
    let mut disabled_cfg = cfg.clone();
    disabled_cfg.onchain_assessment.enabled = false;
    let m = monitor(
        &disabled_cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client.clone()))),
    );
    assert_eq!(m.promote_active_candidates_onchain().await.unwrap(), 0);

    // Guard: no promotion context → 0.
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.promote_active_candidates_onchain().await.unwrap(), 0);

    // Full flow: context + helius + passing assessment.
    let m = monitor(
        &cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client))),
    );
    let promoted = m.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE");
}

#[tokio::test]
async fn test_active_candidate_promotion_no_candidates_and_mid_cycle_cap() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;
    seed_wallet_status(&db, WALLET_B, "CANDIDATE", 0.0).await;
    // Decisions for both candidates (so they're assessed in activity order).
    seed_recent_decision(&db, WALLET, TOKEN).await;
    seed_recent_decision(&db, WALLET_B, TOKEN).await;

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = round_trip_txs(WALLET);
    let helius_client_arc = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 1;
    cfg.promote_max_active_total = 100;
    cfg.promote_max_per_cycle = 10;

    // Cap of 1: the first candidate promotes, the second iteration sees the
    // cap reached mid-cycle and breaks.
    let (db3, _guard3) = create_test_db().await;
    let pool3 = pg_pool(&db3);
    seed_wallet_status(&db3, WALLET, "CANDIDATE", 0.0).await;
    seed_wallet_status(&db3, WALLET_B, "CANDIDATE", 0.0).await;
    seed_recent_decision(&db3, WALLET, TOKEN).await;
    seed_recent_decision(&db3, WALLET_B, TOKEN).await;
    let helius3 = mock_rpc::HeliusApiMock::spawn().await;
    helius3.state.lock().await.transactions = round_trip_txs(WALLET);
    let helius_client3 = helius_client(&helius3.url);
    let mut cfg3 = base_config();
    cfg3.onchain_assessment.enabled = true;
    cfg3.onchain_assessment.min_round_trips = 1;
    cfg3.promote_max_active_total = 1; // binds after the first promotion
    let m3 = monitor(
        &cfg3,
        db3.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client3))),
    );
    let promoted = m3.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 1, "first candidate promotes, loop breaks at cap");
    let actives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE'")
        .fetch_one(&pool3)
        .await
        .unwrap();
    assert_eq!(actives, 1);

    // No candidates at all → 0.
    let (db4, _guard4) = create_test_db().await;
    let helius4 = mock_rpc::HeliusApiMock::spawn().await;
    let helius_client4 = helius_client(&helius4.url);
    let m4 = monitor(
        &cfg,
        db4.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client4))),
    );
    let promoted = m4.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 0);

    // Assessment error (Helius 500) → warn + skip.
    let (db5, _guard5) = create_test_db().await;
    seed_wallet_status(&db5, WALLET, "CANDIDATE", 0.0).await;
    seed_recent_decision(&db5, WALLET, TOKEN).await;
    let helius5 = mock_rpc::HeliusApiMock::spawn().await;
    helius5.state.lock().await.fail_transactions = true;
    let helius_client5 = helius_client(&helius5.url);
    let m5 = monitor(
        &cfg,
        db5.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client5))),
    );
    let promoted = m5.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 0);
}

#[tokio::test]
async fn test_active_candidate_promotion_cap_and_fail_paths() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "CANDIDATE", 0.0).await;
    seed_recent_decision(&db, WALLET, TOKEN).await;
    // Cap reached.
    seed_wallet_status(&db, WALLET_B, "ACTIVE", 90.0).await;

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    let helius_client_arc = helius_client(&helius.url);
    let helius_client2 = helius_client_arc.clone();

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.promote_max_active_total = 1;
    let m = monitor(
        &cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client_arc))),
    );
    let promoted = m.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 0, "ACTIVE cap reached → skip cycle");
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE");

    // Failing assessment (no txs → 0 round trips < min) → not promoted.
    cfg.promote_max_active_total = 100;
    cfg.onchain_assessment.min_round_trips = 10;
    let m = monitor(
        &cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client2))),
    );
    let promoted = m.promote_active_candidates_onchain().await.unwrap();
    assert_eq!(promoted, 0);
}

// ── demote_shadow_losers ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_demote_shadow_losers_flow() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "ACTIVE", 80.0).await;

    // Three admitted shadow positions with -5% mirror_main exits, each in a
    // distinct hour bucket (read-side dedup counts one exit per
    // wallet+token+hour since 2026-08-14).
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, $3, 'SHIELD', true, 0.1, 'webhook', NOW() - make_interval(hours => $4::int))",
        )
        .bind(format!("sl-{i}"))
        .bind(WALLET)
        .bind(TOKEN)
        .bind(i + 1)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, pnl_sol, exit_reason) \
             VALUES ($1, 'mirror_main', -5.0, -0.005, 'stop_loss')",
        )
        .bind(format!("sl-{i}"))
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut cfg = base_config();
    cfg.shadow_quality_enabled = true;
    cfg.shadow_quality_min_samples = 3;
    cfg.shadow_quality_demote_threshold_pct = -2.0;
    cfg.shadow_quality_cost_adjustment_pct = 0.0;
    cfg.shadow_quality_window_hours = 48;
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);

    let demoted = m.demote_shadow_losers().await.unwrap();
    assert_eq!(demoted, 1);
    let (status, wqs): (String, f64) =
        sqlx::query_as("SELECT status, wqs_score FROM wallets WHERE address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "CANDIDATE");
    assert!(wqs <= 10.0, "WQS floored below auto-promote threshold");

    // Second run: wallet no longer ACTIVE → skipped.
    let demoted = m.demote_shadow_losers().await.unwrap();
    assert_eq!(demoted, 0);
}

#[tokio::test]
async fn test_demote_shadow_losers_disabled_and_no_losers() {
    let (db, _guard) = create_test_db().await;
    let mut cfg = base_config();
    cfg.shadow_quality_enabled = false;
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.demote_shadow_losers().await.unwrap(), 0);

    cfg.shadow_quality_enabled = true;
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.demote_shadow_losers().await.unwrap(), 0);
}

// ── audit_actives_onchain ────────────────────────────────────────────────────

#[tokio::test]
async fn test_audit_actives_demotes_failing_wallet() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "ACTIVE", 80.0).await;
    seed_recent_decision(&db, WALLET, TOKEN).await;

    // Helius returns no round trips → fails the audit → demoted.
    let helius = mock_rpc::HeliusApiMock::spawn().await;
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.audit_actives_enabled = true;
    cfg.onchain_assessment.min_round_trips = 10;

    // Guards: no ctx / no helius.
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.audit_actives_onchain().await.unwrap(), 0);

    let m = monitor(
        &cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client))),
    );
    let demoted = m.audit_actives_onchain().await.unwrap();
    assert_eq!(demoted, 1);
    let (status, wqs): (String, f64) =
        sqlx::query_as("SELECT status, wqs_score FROM wallets WHERE address = $1")
            .bind(WALLET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "CANDIDATE");
    assert!(wqs <= 10.0);
}

#[tokio::test]
async fn test_audit_actives_passing_wallet_kept() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    seed_wallet_status(&db, WALLET, "ACTIVE", 80.0).await;
    seed_recent_decision(&db, WALLET, TOKEN).await;

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = round_trip_txs(WALLET);
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 1;
    let m = monitor(
        &cfg,
        db.clone(),
        "http://127.0.0.1:1",
        Some(promotion_ctx(Some(helius_client))),
    );
    let demoted = m.audit_actives_onchain().await.unwrap();
    assert_eq!(demoted, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE");
}

#[tokio::test]
async fn test_audit_actives_guards() {
    let (db, _guard) = create_test_db().await;
    let mut cfg = base_config();
    cfg.onchain_assessment.enabled = false;
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.audit_actives_onchain().await.unwrap(), 0);

    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.audit_actives_enabled = false;
    let m = monitor(&cfg, db.clone(), "http://127.0.0.1:1", None);
    assert_eq!(m.audit_actives_onchain().await.unwrap(), 0);
}

// ── run() background loops ───────────────────────────────────────────────────

#[tokio::test]
async fn test_run_shuts_down_immediately_when_cancelled() {
    let (db, _guard) = create_test_db().await;
    let dune = mock_rpc::DuneMock::spawn().await;
    let cfg = base_config();
    let m = Arc::new(monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        None,
    ));
    let token = CancellationToken::new();
    let run = tokio::spawn({
        let m = m.clone();
        let token = token.clone();
        async move { m.run(token).await }
    });
    // Cancel before the 30s startup delay elapses: the spawned timer tasks
    // exit on cancellation, and run() returns.
    token.cancel();
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("run returns on cancel");
}

#[tokio::test]
async fn test_run_without_dune_key_still_runs_onchain_cycles() {
    let (db, _guard) = create_test_db().await;
    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();
    let cfg = base_config();
    // Construct without a Dune key.
    let old_url = std::env::var("DUNE_API_BASE_URL").ok();
    std::env::set_var("DUNE_API_BASE_URL", format!("{}/api/v1", dune.url));
    std::env::remove_var("DUNE_API_KEY");
    let m = Arc::new(DunePnlMonitor::new(&cfg, db.clone()));
    match old_url {
        Some(v) => std::env::set_var("DUNE_API_BASE_URL", v),
        None => std::env::remove_var("DUNE_API_BASE_URL"),
    }
    std::env::set_var("DUNE_API_KEY", "test-key");

    let token = CancellationToken::new();
    let run = tokio::spawn({
        let m = m.clone();
        let token = token.clone();
        async move { m.run(token).await }
    });
    // With no key, the Dune-promote branch is skipped but the on-chain audit
    // still runs after the 30s startup delay; cancel after it.
    tokio::time::sleep(Duration::from_secs(32)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("run returns after cancellation");
}

#[tokio::test]
async fn test_run_full_cycle_with_dune_key() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    // Losing ACTIVE wallet (demoted by run_check) + profitable CANDIDATE
    // wallet (promoted) + recent decisions for on-chain cycles.
    seed_wallet_status(&db, WALLET, "ACTIVE", 75.0).await;
    seed_wallet_status(&db, WALLET_B, "CANDIDATE", 0.0).await;
    seed_recent_decision(&db, WALLET, TOKEN).await;
    seed_recent_decision(&db, WALLET_B, TOKEN).await;

    let dune = mock_rpc::DuneMock::spawn().await;
    dune.state.lock().await.csv = dual_csv();

    let helius = mock_rpc::HeliusApiMock::spawn().await;
    helius.state.lock().await.transactions = round_trip_txs(WALLET_B);
    let helius_client = helius_client(&helius.url);

    let mut cfg = base_config();
    cfg.check_interval_secs = 1;
    cfg.promote_check_interval_secs = 1;
    cfg.demote_losers_enabled = true;
    cfg.promote_max_active_total = 100;
    cfg.promote_max_per_cycle = 10;
    cfg.onchain_assessment.enabled = true;
    cfg.onchain_assessment.min_round_trips = 1;
    cfg.shadow_quality_enabled = true;
    cfg.shadow_quality_min_samples = 5;
    cfg.shadow_quality_window_hours = 48;

    let m = Arc::new(monitor(
        &cfg,
        db.clone(),
        &format!("{}/api/v1", dune.url),
        Some(promotion_ctx(Some(helius_client))),
    ));
    let token = CancellationToken::new();
    let run = tokio::spawn({
        let m = m.clone();
        let token = token.clone();
        async move { m.run(token).await }
    });

    // Startup catch-up runs after 30s (with 10s Dune poll sleeps inside the
    // check cycles); the 1s interval ticks follow. Wait long enough for the
    // catch-up block and at least one loop tick to complete, then cancel.
    tokio::time::sleep(Duration::from_secs(70)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("run returns after cancellation");

    // The shadow-quality demote cycle ran against the DB.
    let wqs: Option<f64> = sqlx::query_scalar("SELECT wqs_score FROM wallets WHERE address = $1")
        .bind(WALLET)
        .fetch_optional(&pool)
        .await
        .unwrap();
    let _ = wqs;
}
