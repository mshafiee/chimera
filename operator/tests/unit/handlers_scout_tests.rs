//! HTTP handler tests for `operator/src/handlers/scout.rs`.

use axum::http::StatusCode;

#[path = "../common/harness.rs"]
mod harness;

use chimera_operator::middleware::Role;
use harness::{
    api_get, api_post, auth_headers, build, json_body, seed_wallet, test_config, WALLET_A, WALLET_B,
};

#[tokio::test]
async fn scout_status_empty() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/scout/status", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "idle");
    assert!(body["last_run_at"].is_null());
    assert_eq!(body["wallets_analyzed"], 0);
    assert_eq!(body["wqs_distribution"].as_array().unwrap().len(), 5);
    assert_eq!(body["promotion_queue"].as_array().unwrap().len(), 0);
    assert_eq!(body["rejection_queue"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn scout_status_with_wallets() {
    let h = build(test_config()).await;
    // ACTIVE wallet with promoted_at set → promotion queue + "completed" status
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, promoted_at, notes) VALUES ($1, 'ACTIVE', 85.0, NOW(), 'Backtest: PASSED')",
    )
    .bind(WALLET_A)
    .execute(&h.pool)
    .await
    .unwrap();
    // REJECTED wallet → rejection queue
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, notes) VALUES ($1, 'REJECTED', 30.0, 'rug risk')",
    )
    .bind(WALLET_B)
    .execute(&h.pool)
    .await
    .unwrap();

    let resp = api_get(&h.app, "/api/v1/scout/status", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "completed");
    assert!(body["last_run_at"].is_string());
    assert_eq!(body["wallets_analyzed"], 2);
    assert_eq!(body["promotion_queue"][0]["address"], WALLET_A);
    assert_eq!(body["promotion_queue"][0]["backtest_success"], true);
    assert_eq!(body["rejection_queue"][0]["address"], WALLET_B);
}

#[tokio::test]
async fn wqs_distribution_buckets_and_stats() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/scout/wqs-distribution", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_wallets"], 0);
    assert_eq!(body["average_score"], 0.0);
    assert_eq!(body["median_score"], 0.0);

    // One wallet per bucket.
    for (i, wqs) in [10.0, 30.0, 50.0, 70.0, 90.0].iter().enumerate() {
        let addr = format!("{}w{i}", &WALLET_A[..30]);
        sqlx::query(
            "INSERT INTO wallets (address, status, wqs_score) VALUES ($1, 'CANDIDATE', $2)",
        )
        .bind(addr)
        .bind(wqs)
        .execute(&h.pool)
        .await
        .unwrap();
    }

    let resp = api_get(&h.app, "/api/v1/scout/wqs-distribution", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let dist = body["distribution"].as_array().unwrap();
    assert_eq!(dist.len(), 5);
    for bucket in dist {
        assert_eq!(bucket["count"], 1);
        assert_eq!(bucket["percentage"], 20.0);
    }
    assert_eq!(body["average_score"], 50.0);
    assert_eq!(body["median_score"], 50.0); // odd count → middle value
    assert_eq!(body["total_wallets"], 5);
}

#[tokio::test]
async fn wqs_distribution_even_median() {
    let h = build(test_config()).await;
    for (i, wqs) in [20.0, 80.0].iter().enumerate() {
        let addr = format!("{}e{i}", &WALLET_A[..30]);
        sqlx::query(
            "INSERT INTO wallets (address, status, wqs_score) VALUES ($1, 'CANDIDATE', $2)",
        )
        .bind(addr)
        .bind(wqs)
        .execute(&h.pool)
        .await
        .unwrap();
    }
    let resp = api_get(&h.app, "/api/v1/scout/wqs-distribution", Default::default()).await;
    let body = json_body(resp).await;
    // Even count → average of the two middle values
    assert_eq!(body["median_score"], 50.0);
    assert_eq!(body["average_score"], 50.0);
}

#[tokio::test]
async fn scout_metrics_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/scout/metrics", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_analyzed"], 0);
    assert_eq!(body["backtest_success_rate"], 0.0);
    assert_eq!(body["validation_pass_rate"], 0.0);

    // ACTIVE with backtest PASSED note; CANDIDATE with backtest FAILED note.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await; // notes: Backtest: PASSED
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, notes) VALUES ($1, 'CANDIDATE', 55.0, 'Backtest: FAILED')",
    )
    .bind(WALLET_B)
    .execute(&h.pool)
    .await
    .unwrap();

    let resp = api_get(&h.app, "/api/v1/scout/metrics", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_analyzed"], 2);
    assert_eq!(body["rug_check_rejections"], 0);
    // backtest: 1 passed / 2 total
    assert_eq!(body["backtest_success_rate"], 50.0);
    // validation: 1 ACTIVE / (1 ACTIVE + 1 CANDIDATE)
    assert_eq!(body["validation_pass_rate"], 50.0);

    // Add a REJECTED wallet → rug count 1, validation denominator unchanged.
    sqlx::query("INSERT INTO wallets (address, status, wqs_score) VALUES ($1, 'REJECTED', 10.0)")
        .bind("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX")
        .execute(&h.pool)
        .await
        .unwrap();
    let resp = api_get(&h.app, "/api/v1/scout/metrics", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["rug_check_rejections"], 1);
    assert_eq!(body["total_analyzed"], 3);
}

#[tokio::test]
async fn scout_not_implemented_endpoints() {
    let h = build(test_config()).await;

    // trigger_scout_run → 503
    let resp = api_post(
        &h.app,
        "/api/v1/scout/run",
        auth_headers(Role::Operator),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    // get_budget_status → 503
    let resp = api_get(&h.app, "/api/v1/scout/budget", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    // get_conviction_allocation → 503
    let resp = api_get(&h.app, "/api/v1/scout/conviction", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn cache_stats_empty_and_with_wallets() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/scout/cache", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_entries"], 0);
    assert_eq!(body["hit_rate"], 0.0);
    assert_eq!(body["activity_distribution"]["very_high"], 0);
    assert_eq!(body["cache_efficiency"], 0.0);

    // ACTIVE updated within the last hour → very_high bucket
    sqlx::query(
        "INSERT INTO wallets (address, status, updated_at) VALUES ($1, 'ACTIVE', NOW() - INTERVAL '10 minutes')",
    )
    .bind(WALLET_A)
    .execute(&h.pool)
    .await
    .unwrap();
    // ACTIVE updated 12 hours ago → high bucket
    sqlx::query(
        "INSERT INTO wallets (address, status, updated_at) VALUES ($1, 'ACTIVE', NOW() - INTERVAL '12 hours')",
    )
    .bind(WALLET_B)
    .execute(&h.pool)
    .await
    .unwrap();
    // CANDIDATE updated 3 days ago → medium
    sqlx::query(
        "INSERT INTO wallets (address, status, updated_at) VALUES ($1, 'CANDIDATE', NOW() - INTERVAL '3 days')",
    )
    .bind("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX")
    .execute(&h.pool)
    .await
    .unwrap();
    // CANDIDATE updated 10 days ago → low
    sqlx::query(
        "INSERT INTO wallets (address, status, updated_at) VALUES ($1, 'CANDIDATE', NOW() - INTERVAL '10 days')",
    )
    .bind("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsY")
    .execute(&h.pool)
    .await
    .unwrap();
    // REJECTED → inactive
    sqlx::query("INSERT INTO wallets (address, status, updated_at) VALUES ($1, 'REJECTED', NOW())")
        .bind("5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsZ")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(&h.app, "/api/v1/scout/cache", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let dist = &body["activity_distribution"];
    assert_eq!(dist["very_high"], 1);
    assert_eq!(dist["high"], 1);
    assert_eq!(dist["medium"], 1);
    assert_eq!(dist["low"], 1);
    assert_eq!(dist["inactive"], 1);
    assert_eq!(body["total_entries"], 5);
    // hits = very_high*10 + high*5 = 15; misses = medium+low = 2
    assert_eq!(body["total_hits"], 15);
    assert_eq!(body["total_misses"], 2);
    let hit_rate = body["hit_rate"].as_f64().unwrap();
    assert!((hit_rate - 88.23529411764706).abs() < 1e-9);
    let efficiency = body["cache_efficiency"].as_f64().unwrap();
    assert!((efficiency - hit_rate * 5.0 / 10000.0).abs() < 1e-12);
}
