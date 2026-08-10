//! Shadow-fill calibration tests (Phase C3).
//!
//! Covers `LatencyTracker` percentile sampling and the full
//! `capture_and_model_fill` flow with a mocked Jupiter quote API and a real
//! per-test Postgres database.

use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::decision_recorder::DecisionRecorder;
use chimera_operator::engine::run_context::RunContext;
use chimera_operator::engine::shadow_fill::{capture_and_model_fill, LatencyTracker};
use chimera_operator::engine::transaction_builder::TransactionBuilder;
use std::sync::Arc;
use std::time::Duration;

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

fn recorder_with_db(db: Arc<dyn Database>) -> Arc<DecisionRecorder> {
    let rc = Arc::new(RunContext::new(
        "hash",
        &["wallet-a".to_string()],
        chrono::Utc::now(),
    ));
    Arc::new(DecisionRecorder::new(db, rc))
}

fn quote_client(jupiter_url: String) -> Arc<TransactionBuilder> {
    let mut config = AppConfig::default();
    config.jupiter.api_url = jupiter_url;
    let rpc = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
        "http://127.0.0.1:1".to_string(),
    ));
    Arc::new(TransactionBuilder::new(rpc, Arc::new(config)).expect("transaction builder"))
}

/// Insert a decision row so `update_fill_model` has a target; return its id.
async fn insert_decision_row(pool: &sqlx::Pool<sqlx::Postgres>, decision_id: &str) {
    sqlx::query(
        "INSERT INTO decision_records (decision_id, run_id, ingress, wallet_address, token_address, action, admitted, source_amount_sol, received_at, decided_at, code_revision, config_hash, roster_hash) \
         VALUES ($1, 'run', 'webhook', '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', $2, 'BUY', true, 1.0, NOW(), NOW(), 'rev', 'hash', 'roster')",
    )
    .bind(decision_id)
    .bind("4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
    .execute(pool)
    .await
    .unwrap();
}

async fn wait_for_model_version(pool: &sqlx::Pool<sqlx::Postgres>, decision_id: &str) -> String {
    for _ in 0..100 {
        let v: Option<Option<String>> = sqlx::query_scalar(
            "SELECT simulated_fill_model_version FROM decision_records WHERE decision_id = $1",
        )
        .bind(decision_id)
        .fetch_optional(pool)
        .await
        .unwrap();
        if let Some(Some(v)) = v {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("fill model never persisted for {decision_id}");
}

// ── LatencyTracker ──────────────────────────────────────────────────────────

#[test]
fn test_latency_tracker_basic_percentiles() {
    let tracker = LatencyTracker::new(5);
    assert_eq!(tracker.percentile(50.0), 0, "empty tracker -> 0");
    assert_eq!(tracker.p50_us(), 0);

    tracker.record(100);
    tracker.record(200);
    tracker.record(300);
    assert_eq!(tracker.p50_us(), 200);
    assert_eq!(tracker.percentile(100.0), 300);
    assert_eq!(tracker.percentile(0.0), 100);

    // Over-cap: oldest sample is evicted.
    tracker.record(400);
    tracker.record(500);
    tracker.record(600);
    assert_eq!(tracker.p50_us(), 400, "cap 5 keeps the newest 5 samples");
    assert_eq!(tracker.percentile(100.0), 600);
}

#[test]
fn test_latency_tracker_cap_zero_is_noop() {
    let tracker = LatencyTracker::new(0);
    tracker.record(1000);
    assert_eq!(tracker.p50_us(), 0);
}

#[test]
fn test_latency_tracker_single_sample() {
    let tracker = LatencyTracker::new(3);
    tracker.record(42);
    assert_eq!(tracker.p50_us(), 42);
}

// ── default_nonlanding_prob (via the production flow) ───────────────────────

#[tokio::test]
async fn test_default_nonlanding_prob_env_variants() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let qc = quote_client(jup_url);

    async fn prob_for(
        db: &Arc<dyn Database>,
        pool: &sqlx::Pool<sqlx::Postgres>,
        qc: &Arc<TransactionBuilder>,
        label: &str,
    ) -> f64 {
        let recorder = Arc::new(DecisionRecorder::new(
            db.clone(),
            Arc::new(RunContext::new("hash", &[], chrono::Utc::now())),
        ));
        let decision_id = format!("sf-prob-{label}-{}", uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO decision_records (decision_id, run_id, ingress, wallet_address, token_address, action, admitted, source_amount_sol, received_at, decided_at, code_revision, config_hash, roster_hash) \
             VALUES ($1, 'run', 'webhook', '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', 'BUY', true, 1.0, NOW(), NOW(), 'rev', 'hash', 'roster')",
        )
        .bind(&decision_id)
        .execute(pool)
        .await
        .unwrap();
        capture_and_model_fill(
            qc.clone(),
            Arc::new(LatencyTracker::new(10)),
            recorder,
            decision_id.clone(),
            "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
            1.0,
            0,
            true,
        )
        .await;
        for _ in 0..100 {
            let q: Option<Option<serde_json::Value>> = sqlx::query_scalar(
                "SELECT quote_json FROM decision_records WHERE decision_id = $1",
            )
            .bind(&decision_id)
            .fetch_optional(pool)
            .await
            .unwrap();
            if let Some(Some(q)) = q {
                return q["nonlanding_prob"].as_f64().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no quote_json for {decision_id}");
    }

    // Unset → 0.03
    std::env::remove_var("CHIMERA_NONLANDING_PROB");
    assert_eq!(prob_for(&db, &pool, &qc, "unset").await, 0.03);

    // Valid in-range → parsed value.
    std::env::set_var("CHIMERA_NONLANDING_PROB", "0.5");
    assert_eq!(prob_for(&db, &pool, &qc, "half").await, 0.5);

    // Out-of-range → warn + default.
    std::env::set_var("CHIMERA_NONLANDING_PROB", "2.0");
    assert_eq!(prob_for(&db, &pool, &qc, "oob").await, 0.03);

    // Unparseable → warn + default.
    std::env::set_var("CHIMERA_NONLANDING_PROB", "abc");
    assert_eq!(prob_for(&db, &pool, &qc, "bad").await, 0.03);

    std::env::remove_var("CHIMERA_NONLANDING_PROB");
}

// ── capture_and_model_fill ───────────────────────────────────────────────────

#[tokio::test]
async fn test_capture_and_model_fill_buy_full_flow() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));

    let decision_id = format!("sf-buy-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        0.5,
        1_000_000,
        true,
    )
    .await;

    let version = wait_for_model_version(&pool, &decision_id).await;
    assert_eq!(version, "v1-delayed-requote");
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(quote["model_version"], "v1-delayed-requote");
    assert!(quote["decision_quote"]["inAmount"].is_string());
    assert!(quote["delayed_quote"]["outAmount"].is_string());
    let slippage = quote["modeled_slippage_pct"].as_f64();
    assert!(
        slippage.is_some(),
        "both quotes present -> slippage computed"
    );
}

#[tokio::test]
async fn test_capture_and_model_fill_sell_direction() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));

    let decision_id = format!("sf-sell-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        0.25,
        0,
        false,
    )
    .await;

    let version = wait_for_model_version(&pool, &decision_id).await;
    assert_eq!(version, "v1-delayed-requote");
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(quote["model_version"], "v1-delayed-requote");
}

#[tokio::test]
async fn test_capture_and_model_fill_sell_with_delayed_requote() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));
    tracker.record(1_000);

    let decision_id = format!("sf-sell-delay-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        0.5,
        1_000,
        false,
    )
    .await;

    let _ = wait_for_model_version(&pool, &decision_id).await;
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(quote["delayed_quote"]["outAmount"].is_string());
}

#[tokio::test]
async fn test_capture_and_model_fill_zero_latency_no_delayed_quote() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());
    // p50 = 0 → the delayed requote is skipped entirely.
    let tracker = Arc::new(LatencyTracker::new(10));

    let decision_id = format!("sf-zero-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        0,
        true,
    )
    .await;

    let _ = wait_for_model_version(&pool, &decision_id).await;
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        quote["delayed_quote"].is_null(),
        "no delayed requote at p50=0"
    );
    assert!(quote["modeled_slippage_pct"].is_null());
}

#[tokio::test]
async fn test_capture_and_model_fill_zero_amount_quote() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    // outAmount zero → fill_price returns None (division guard); negative
    // numeric inAmount → parse_amount falls through the u64/i64 branches.
    jup_mock.state.lock().await.quote = serde_json::json!({
        "inAmount": -5,
        "outAmount": "0",
    });
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));

    let decision_id = format!("sf-zero-amt-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        100,
        true,
    )
    .await;

    let _ = wait_for_model_version(&pool, &decision_id).await;
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(quote["modeled_slippage_pct"].is_null());
}

#[tokio::test]
async fn test_capture_and_model_fill_parse_variants() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));

    // Negative numeric inAmount → parse_amount falls through to the i64
    // branch and rejects.
    jup_mock.state.lock().await.quote = serde_json::json!({"inAmount": -5, "outAmount": "10"});
    let decision_id = format!("sf-neg-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        100,
        true,
    )
    .await;
    let _ = wait_for_model_version(&pool, &decision_id).await;

    // outAmount zero → fill_price's zero guard.
    jup_mock.state.lock().await.quote = serde_json::json!({"inAmount": "100", "outAmount": "0"});
    let decision_id = format!("sf-zero-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        100,
        true,
    )
    .await;
    let _ = wait_for_model_version(&pool, &decision_id).await;

    // Boolean inAmount → parse_amount's catch-all.
    jup_mock.state.lock().await.quote = serde_json::json!({"inAmount": true, "outAmount": "10"});
    let decision_id = format!("sf-bool-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;
    capture_and_model_fill(
        quote_client(jup_url),
        Arc::new(LatencyTracker::new(10)),
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        100,
        true,
    )
    .await;
    let _ = wait_for_model_version(&pool, &decision_id).await;
}

#[tokio::test]
async fn test_capture_and_model_fill_quote_failure_records_null_quote() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    jup_mock.state.lock().await.fail = true;
    let recorder = recorder_with_db(db.clone());
    let tracker = Arc::new(LatencyTracker::new(10));

    let decision_id = format!("sf-fail-{}", uuid::Uuid::new_v4());
    insert_decision_row(&pool, &decision_id).await;

    capture_and_model_fill(
        quote_client(jup_url),
        tracker,
        recorder,
        decision_id.clone(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        100,
        true,
    )
    .await;

    let _ = wait_for_model_version(&pool, &decision_id).await;
    let quote: serde_json::Value =
        sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
            .bind(&decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(quote["decision_quote"].is_null());
    assert!(quote["delayed_quote"].is_null());
    assert!(quote["modeled_slippage_pct"].is_null());
}

#[tokio::test]
async fn test_capture_and_model_fill_guards() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let jup_mock = mock_rpc::JupiterQuoteMock::spawn().await;
    let jup_url = jup_mock.url.clone();
    let recorder = recorder_with_db(db.clone());

    // Invalid token address → early return before any quote/insert.
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-bad-token".to_string(),
        "not-a-pubkey".to_string(),
        1.0,
        0,
        true,
    )
    .await;

    // Valid inputs but the decision row does not exist: the flow completes and
    // the UPDATE matches zero rows (no new row is created).
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-2".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1.0,
        0,
        true,
    )
    .await;

    // NaN size → skip.
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-nan".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        f64::NAN,
        0,
        true,
    )
    .await;

    // Negative size → skip.
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-neg".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        -1.0,
        0,
        true,
    )
    .await;

    // Infinite size → skip.
    capture_and_model_fill(
        quote_client(jup_url.clone()),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-inf".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        f64::INFINITY,
        0,
        true,
    )
    .await;

    // Beyond u64 lamport range → skip.
    capture_and_model_fill(
        quote_client(jup_url),
        Arc::new(LatencyTracker::new(10)),
        recorder.clone(),
        "id-huge".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        f64::MAX,
        0,
        true,
    )
    .await;

    // Rounds to zero lamports → skip.
    capture_and_model_fill(
        quote_client("http://127.0.0.1:1".to_string()),
        Arc::new(LatencyTracker::new(10)),
        recorder,
        "id-tiny".to_string(),
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        1e-12,
        0,
        true,
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decision_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "guard paths must not touch the DB");
}
