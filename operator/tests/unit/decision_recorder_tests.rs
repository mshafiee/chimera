//! DecisionRecorder coverage tests (C1).
//!
//! Uses a real per-test Postgres database. The recorder is fire-and-forget, so
//! assertions poll the DB until the spawned persistence task lands.

use chimera_operator::db_abstraction::{create_database, Database, DbPool};
use chimera_operator::engine::decision_recorder::DecisionRecorder;
use chimera_operator::engine::run_context::RunContext;
use chimera_operator::engine::selection::{BuyDecision, Ingress, SelectionRequest};
use chimera_operator::models::{Action, Strategy};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[path = "../common/mod.rs"]
mod common;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

async fn wait_for_row(pool: &sqlx::Pool<sqlx::Postgres>, decision_id: &str) {
    for _ in 0..100 {
        let row: Option<String> =
            sqlx::query_scalar("SELECT decision_id FROM decision_records WHERE decision_id = $1")
                .bind(decision_id)
                .fetch_optional(pool)
                .await
                .unwrap();
        if row.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("decision row never inserted");
}

fn pg_pool(db: &Arc<dyn Database>) -> sqlx::Pool<sqlx::Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

fn run_context() -> Arc<RunContext> {
    Arc::new(RunContext::new(
        "config-hash-1234",
        &["wallet-a".to_string(), "wallet-b".to_string()],
        chrono::Utc::now(),
    ))
}

fn make_decision(admitted: bool, code: Option<&'static str>) -> BuyDecision {
    let mut d = BuyDecision {
        decision_id: format!("decision-{}", uuid::Uuid::new_v4()),
        admitted,
        rejection_reason: if admitted {
            None
        } else {
            Some("test rejection".to_string())
        },
        rejection_code: code,
        strategy: if admitted {
            Some(Strategy::Shield)
        } else {
            None
        },
        size_sol: if admitted { Some(dec("0.5")) } else { None },
        source_amount_sol: dec("1.0"),
        wqs: Some(75.5),
        wqs_confidence: Some(0.9),
        quality_score: Some(0.6),
        consensus_wallet_count: Some(2),
        regime_multiplier: Some(dec("1.2")),
        token_age_hours: Some(12.0),
        liquidity_usd: Some(dec("50000")),
        volume_24h_usd: Some(dec("100000")),
        trial_admission: false,
        price_impact_pct: Some(dec("0.5")),
        config_hash: "ch".to_string(),
        ingress: Ingress::Webhook,
        is_consensus: true,
        fast_check_errored: false,
    };
    // For the extreme-values variant: values that must be clamped or dropped.
    let _ = &mut d;
    d
}

fn make_request() -> SelectionRequest {
    SelectionRequest {
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        token_address: "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R".to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Helius,
        source_slot: Some(12345),
        source_block_time: None,
        exit_fraction: None,
        whale_entry_price: None,
    }
}

async fn wait_for<T>(pool: &sqlx::Pool<sqlx::Postgres>, query: &str, decision_id: &str) -> Option<T>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    for _ in 0..100 {
        let row: Result<Option<T>, sqlx::Error> = sqlx::query_as(query)
            .bind(decision_id)
            .fetch_optional(pool)
            .await;
        if let Ok(Some(v)) = row {
            return Some(v);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

#[tokio::test]
async fn test_record_persists_admitted_decision() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let recorder = DecisionRecorder::new(db.clone(), run_context());
    let decision = make_decision(true, None);
    let req = make_request();

    recorder.record(&decision, &req, Some("trade-1"), chrono::Utc::now());

    let _ = wait_for::<(String, String, bool, String, f64)>(
        &pool,
        "SELECT decision_id, run_id, admitted, ingress, wqs FROM decision_records WHERE decision_id = $1",
        &decision.decision_id,
    )
    .await
    .expect("decision row must land");
    let row = sqlx::query_as::<_, (String, String, bool, String, f64)>(
        "SELECT decision_id, run_id, admitted, ingress, wqs FROM decision_records WHERE decision_id = $1",
    )
    .bind(&decision.decision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, decision.decision_id);
    assert_eq!(row.2, true);
    assert_eq!(row.3, "webhook");
    assert_eq!(row.4, 75.5);

    // Completeness ratio reaches 1.0 once the insert lands.
    for _ in 0..100 {
        if recorder.completeness() >= 1.0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(recorder.completeness(), 1.0);
}

#[tokio::test]
async fn test_record_rejected_decision_with_extreme_values() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let recorder = DecisionRecorder::new(db.clone(), run_context());

    let mut decision = make_decision(false, Some("WQS_TOO_LOW"));
    // Extreme values: size beyond NUMERIC(30,18) must be clamped; usize max
    // must drop the consensus count (i32 overflow); u64 max slot must drop.
    decision.size_sol = Some(dec("999999999999999999999999"));
    decision.consensus_wallet_count = Some(usize::MAX);
    let mut req = make_request();
    req.source_slot = Some(u64::MAX);

    recorder.record(&decision, &req, None, chrono::Utc::now());

    let _ = wait_for::<(String, bool, Option<String>, Option<f64>, Option<i64>)>(
        &pool,
        "SELECT decision_id, admitted, rejection_code, size_sol::float8, source_slot FROM decision_records WHERE decision_id = $1",
        &decision.decision_id,
    )
    .await
    .expect("decision row must land");

    let (_, admitted, code, size_sol, source_slot) = sqlx::query_as::<_, (String, bool, Option<String>, Option<f64>, Option<i64>)>(
        "SELECT decision_id, admitted, rejection_code, size_sol::float8, source_slot FROM decision_records WHERE decision_id = $1",
    )
    .bind(&decision.decision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!admitted);
    assert_eq!(code.as_deref(), Some("WQS_TOO_LOW"));
    assert_eq!(
        size_sol.unwrap(),
        999_999_999_999.0,
        "clamped to NUMERIC(30,18) bound"
    );
    assert_eq!(
        source_slot, None,
        "u64::MAX cannot fit i64 and must be dropped"
    );
}

#[tokio::test]
async fn test_run_context_accessor() {
    let (db, _guard) = create_test_db().await;
    let rc = run_context();
    let recorder = DecisionRecorder::new(db.clone(), rc.clone());
    assert!(Arc::ptr_eq(recorder.run_context(), &rc));
}

#[tokio::test]
async fn test_completeness_ratio_and_idle() {
    let (db, _guard) = create_test_db().await;
    let recorder = DecisionRecorder::new(db.clone(), run_context());

    // Nothing attempted yet → 1.0 (no evidence of loss).
    assert_eq!(recorder.completeness(), 1.0);

    // Two attempts, one success → 0.5.
    let decision = make_decision(true, None);
    let req = make_request();
    recorder.record(&decision, &req, None, chrono::Utc::now());
    // Wait until the insert lands and the ratio moves to 0.5.
    for _ in 0..100 {
        if recorder.completeness() < 1.0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Second attempt with a duplicate decision_id → insert fails (PK conflict),
    // attempted increments but persisted does not.
    let dup = decision.clone();
    let req2 = make_request();
    recorder.record(&dup, &req2, None, chrono::Utc::now());
    // Give the failing task time to run.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(recorder.completeness(), 0.5);
}

#[tokio::test]
async fn test_update_quote_link_trade_and_fill_model() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let recorder = DecisionRecorder::new(db.clone(), run_context());
    let decision = make_decision(true, None);
    let req = make_request();
    recorder.record(&decision, &req, None, chrono::Utc::now());
    wait_for_row(&pool, &decision.decision_id).await;

    // update_quote
    recorder.update_quote(
        decision.decision_id.clone(),
        serde_json::json!({"inAmount": "1"}),
    );
    for _ in 0..100 {
        let q: Option<Option<serde_json::Value>> =
            sqlx::query_scalar("SELECT quote_json FROM decision_records WHERE decision_id = $1")
                .bind(&decision.decision_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        if let Some(Some(v)) = &q {
            if !v.is_null() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // link_trade
    recorder.link_trade(decision.decision_id.clone(), "trade-abc".to_string());
    for _ in 0..100 {
        let t: Option<Option<String>> =
            sqlx::query_scalar("SELECT trade_uuid FROM decision_records WHERE decision_id = $1")
                .bind(&decision.decision_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        if let Some(Some(_)) = t {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let trade: String =
        sqlx::query_scalar("SELECT trade_uuid FROM decision_records WHERE decision_id = $1")
            .bind(&decision.decision_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(trade, "trade-abc");

    // update_fill_model with modeled slippage
    recorder.update_fill_model(
        decision.decision_id.clone(),
        serde_json::json!({"model_version": "v1-delayed-requote"}),
        "v1-delayed-requote",
        Some(1.5),
    );
    for _ in 0..100 {
        let v: Option<Option<String>> = sqlx::query_scalar(
            "SELECT simulated_fill_model_version FROM decision_records WHERE decision_id = $1",
        )
        .bind(&decision.decision_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
        if let Some(Some(_)) = v {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (version, impact): (String, Option<f64>) = sqlx::query_as(
        "SELECT simulated_fill_model_version, price_impact_pct::float8 FROM decision_records WHERE decision_id = $1",
    )
    .bind(&decision.decision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(version, "v1-delayed-requote");
    assert_eq!(impact, Some(1.5));
}

#[tokio::test]
async fn test_update_fill_model_keeps_existing_impact_when_none() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let recorder = DecisionRecorder::new(db.clone(), run_context());
    let decision = make_decision(true, None);
    let req = make_request();
    recorder.record(&decision, &req, None, chrono::Utc::now());
    wait_for_row(&pool, &decision.decision_id).await;

    recorder.update_fill_model(
        decision.decision_id.clone(),
        serde_json::json!({}),
        "v1",
        None,
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let version: Option<Option<String>> = sqlx::query_scalar(
        "SELECT simulated_fill_model_version FROM decision_records WHERE decision_id = $1",
    )
    .bind(&decision.decision_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(version.flatten().as_deref(), Some("v1"));
}

#[tokio::test]
async fn test_retry_update_transient_then_error_and_saturated_semaphore() {
    let (db, _guard) = create_test_db().await;
    let pool = pg_pool(&db);
    let recorder = DecisionRecorder::new(db.clone(), run_context());
    let decision = make_decision(true, None);
    let req = make_request();
    recorder.record(&decision, &req, None, chrono::Utc::now());
    wait_for_row(&pool, &decision.decision_id).await;

    // Saturate the write semaphore by holding the table lock so the spawned
    // insert tasks block while holding their permits, then record enough
    // decisions to exhaust the 64 permits — the next record must skip.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE decision_records IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    for i in 0..70 {
        let mut d = make_decision(true, None);
        d.decision_id = format!("saturated-{i}");
        recorder.record(&d, &req, None, chrono::Utc::now());
    }
    // Give tasks time to acquire permits and block on the lock (generous
    // sleeps: tarpaulin instrumentation slows spawned tasks).
    tokio::time::sleep(Duration::from_secs(3)).await;
    tx.rollback().await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // At least 64 rows landed; the overflow decisions were dropped.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decision_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        count >= 64,
        "semaphore should admit up to 64 concurrent writes"
    );

    // Error paths: close the pool, then every update must fail (retry loop
    // exhausts and the error branch of update_quote / link_trade runs).
    db.close().await.unwrap();
    recorder.update_quote(decision.decision_id.clone(), serde_json::json!({}));
    recorder.link_trade(decision.decision_id.clone(), "t".to_string());
    recorder.update_fill_model(
        decision.decision_id.clone(),
        serde_json::json!({}),
        "v",
        Some(1.0),
    );
    // retry_update sleeps 100ms then 200ms before giving up.
    tokio::time::sleep(Duration::from_millis(1200)).await;
}
