//! HTTP handler tests for `operator/src/handlers/operations.rs`.

use axum::http::StatusCode;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/harness.rs"]
mod harness;

use harness::{api_get, build, json_body, seed_config_audit, test_config};

#[tokio::test]
async fn resources_endpoint() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/operations/resources", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["cpu"]["current"].is_number());
    assert!(body["cpu"]["max"].is_number());
    assert!(body["memory"]["current"].is_number());
    assert!(body["disk"]["current"].is_number());
    assert!(body["network"]["bytes_sent"].is_number());
    assert!(body["degradation"]["memory_pressure_active"].is_boolean());
    assert!(body["degradation"]["rpc_backoff_multiplier"].is_number());
    assert!(body["timestamp"].is_string());
    assert!(body["cpu"]["status"].is_string());
}

#[tokio::test]
async fn secrets_never_rotated() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/operations/secrets", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "never_rotated");
    assert_eq!(body["is_initialized"], false);
    assert!(body["last_rotation_at"].is_null());
    assert_eq!(body["rotation_history"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn secrets_rotation_states_and_parsing() {
    let h = build(test_config()).await;

    // Rotation 110 days ago → overdue; structured metrics parsing.
    seed_config_audit(
        &h.pool,
        "secret_rotation.main",
        None,
        "status=success;duration_seconds=12;keys_rotated=3;failed_keys=1",
        "SYSTEM",
    )
    .await;
    sqlx::query("UPDATE config_audit SET changed_at = NOW() - INTERVAL '110 days' WHERE key = 'secret_rotation.main'")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(&h.app, "/api/v1/operations/secrets", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "overdue");
    assert_eq!(body["is_initialized"], true);
    let days = body["days_until_due"].as_i64().unwrap();
    assert!(days < 0, "overdue → negative days, got {days}");
    let history = body["rotation_history"].as_array().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["status"], "success");
    assert_eq!(history[0]["duration_seconds"], 12);
    assert_eq!(history[0]["keys_rotated"], 3);
    assert_eq!(history[0]["failed_keys"], 1);
    assert!(body["next_rotation_at"].is_string());

    // Second rotation 20 days ago → Active.
    let h2 = build(test_config()).await;
    seed_config_audit(
        &h2.pool,
        "secret_rotation.main",
        None,
        "status=success",
        "SYSTEM",
    )
    .await;
    sqlx::query("UPDATE config_audit SET changed_at = NOW() - INTERVAL '20 days' WHERE key = 'secret_rotation.main'")
        .execute(&h2.pool)
        .await
        .unwrap();
    let resp = api_get(&h2.app, "/api/v1/operations/secrets", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["status"], "active");

    // Rotation 6 days ago → DueSoon.
    let h3 = build(test_config()).await;
    seed_config_audit(
        &h3.pool,
        "secret_rotation.main",
        None,
        "status=failed",
        "SYSTEM",
    )
    .await;
    sqlx::query("UPDATE config_audit SET changed_at = NOW() - INTERVAL '84 days' WHERE key = 'secret_rotation.main'")
        .execute(&h3.pool)
        .await
        .unwrap();
    let resp = api_get(&h3.app, "/api/v1/operations/secrets", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["status"], "due_soon");
    let history = body["rotation_history"].as_array().unwrap();
    assert_eq!(history[0]["status"], "failed");

    // NOTE: the "corrupt timestamp → Unknown" branch (handler lines
    // ~281-294) is unreachable via real DB data — Postgres only ever returns
    // parseable timestamps from the timestamptz column, so the parse error
    // path is defensive-only.
}

#[tokio::test]
async fn rate_limit_status_healthy() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/operations/rate-limit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["overall_status"], "healthy");
    let endpoints = body["endpoints"].as_array().unwrap();
    assert_eq!(endpoints.len(), 2);
    assert_eq!(endpoints[0]["endpoint"], "/api/v1/webhook");
    assert_eq!(endpoints[0]["status"], "ok");
    assert_eq!(endpoints[0]["window_seconds"], 1);
    assert!(endpoints[0]["reset_at"].is_string());
}

#[tokio::test]
async fn rate_limit_status_throttled() {
    let h = build(test_config()).await;
    // Exhaust both limiters → high utilization → throttled.
    for _ in 0..60 {
        let _ = h.monitoring_state.webhook_rate_limiter.try_acquire();
        let _ = h
            .api_state
            .webhook_rate_limiter
            .as_ref()
            .unwrap()
            .try_acquire();
    }
    let resp = api_get(&h.app, "/api/v1/operations/rate-limit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let statuses: Vec<&str> = body["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert!(
        statuses.contains(&"throttled") || statuses.contains(&"warning"),
        "statuses {statuses:?}"
    );
    assert!(body["overall_status"].is_string());
}

#[tokio::test]
async fn health_checks_healthy_and_degraded() {
    let h = build(test_config()).await;

    // DB passing; engine None → RPC warning; CB active → passing; price cache
    // empty → warning → overall Degraded.
    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["overall_status"], "degraded");
    let checks = body["checks"].as_array().unwrap();
    let by_name = |name: &str| {
        checks
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("missing check {name}"))
            .clone()
    };
    let db_check = by_name("database");
    assert_eq!(db_check["status"], "passing");
    assert_eq!(db_check["message"], "Database connection healthy");
    let rpc_check = by_name("rpc");
    assert_eq!(rpc_check["status"], "warning");
    assert_eq!(rpc_check["message"], "RPC health not yet checked");
    let cb_check = by_name("circuit_breaker");
    assert_eq!(cb_check["status"], "passing");
    let pc_check = by_name("price_cache");
    assert_eq!(pc_check["status"], "warning");
    assert!(db_check["response_time_ms"].as_f64().unwrap() >= 0.0);

    // Price cache with entries → passing.
    h.api_state.price_cache.set_price(
        "So11111111111111111111111111111111111111112",
        rust_decimal::Decimal::from_str("150.0").unwrap(),
        chimera_operator::price_cache::PriceSource::Jupiter,
        None,
    );
    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    let body = json_body(resp).await;
    let checks = body["checks"].as_array().unwrap();
    let pc_check = checks.iter().find(|c| c["name"] == "price_cache").unwrap();
    assert_eq!(pc_check["status"], "passing");
    assert!(pc_check["message"]
        .as_str()
        .unwrap()
        .contains("Price cache healthy"));
}

#[tokio::test]
async fn health_checks_unhealthy_when_cb_tripped() {
    let h = build(test_config()).await;
    h.api_state
        .circuit_breaker
        .manual_trip("test", "manual trip".to_string())
        .await
        .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["overall_status"], "unhealthy");
    let checks = body["checks"].as_array().unwrap();
    let cb_check = checks
        .iter()
        .find(|c| c["name"] == "circuit_breaker")
        .unwrap();
    assert_eq!(cb_check["status"], "failing");
    assert!(cb_check["message"]
        .as_str()
        .unwrap()
        .contains("manual trip"));
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

#[tokio::test]
async fn secrets_partial_status_and_metric_parsing() {
    let h = build(test_config()).await;
    // Older entry: success + structured metrics (failed_keys parsed).
    seed_config_audit(
        &h.pool,
        "secret_rotation.main",
        None,
        "status=success;failed_keys=1;keys_rotated=2",
        "SYSTEM",
    )
    .await;
    sqlx::query("UPDATE config_audit SET changed_at = NOW() - INTERVAL '30 days' WHERE key = 'secret_rotation.main'")
        .execute(&h.pool)
        .await
        .unwrap();
    // Newer entry: partial + unparseable duration (avoids the "failed"
    // substring so the status classifier lands on Partial).
    seed_config_audit(
        &h.pool,
        "secret_rotation.main",
        None,
        "status=partial;duration_seconds=abc",
        "SYSTEM",
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/operations/secrets", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let history = body["rotation_history"].as_array().unwrap();
    let partial = history.iter().find(|h| h["status"] == "partial").unwrap();
    assert_eq!(partial["duration_seconds"], serde_json::Value::Null); // unparseable
    let success = history.iter().find(|h| h["status"] == "success").unwrap();
    assert_eq!(success["keys_rotated"], 2);
    assert_eq!(success["failed_keys"], 1);
}

#[tokio::test]
async fn health_checks_db_failing() {
    let h = build(test_config()).await;
    // Drop the trades table so get_trade_statistics fails → database check
    // reports Failing → overall Unhealthy.
    sqlx::query("DROP TABLE trades CASCADE")
        .execute(&h.pool)
        .await
        .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["overall_status"], "unhealthy");
    let checks = body["checks"].as_array().unwrap();
    let db_check = checks.iter().find(|c| c["name"] == "database").unwrap();
    assert_eq!(db_check["status"], "failing");
}

#[tokio::test]
async fn health_checks_cb_cooldown() {
    let h = build(test_config()).await;
    h.api_state
        .circuit_breaker
        .manual_trip("test", "cooldown".to_string())
        .await
        .unwrap();
    h.api_state.circuit_breaker.enter_cooldown().await.unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let checks = body["checks"].as_array().unwrap();
    let cb_check = checks
        .iter()
        .find(|c| c["name"] == "circuit_breaker")
        .unwrap();
    assert_eq!(cb_check["status"], "warning");
    assert!(cb_check["message"].as_str().unwrap().contains("cooldown"));
}

#[tokio::test]
async fn rate_limit_status_exact_throttled() {
    use axum::routing::get;
    use axum::Router;
    use chimera_operator::handlers::OperationsState;
    use chimera_operator::monitoring::rate_limiter::RateLimiter;

    let h = build(test_config()).await;
    // Custom OperationsState with 60s-window limiters so credits cannot expire
    // while other tests run concurrently.
    let webhook_limiter = Arc::new(RateLimiter::new(40, 60));
    let rpc_limiter = Arc::new(RateLimiter::new(40, 60));
    let state = Arc::new(OperationsState {
        db: h.db.clone(),
        engine: Some(Arc::new(h.engine_handle.clone())),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        price_cache: h.api_state.price_cache.clone(),
        webhook_rate_limiter: Some(webhook_limiter.clone()),
        rpc_rate_limiter: Some(rpc_limiter.clone()),
    });
    let app = Router::new()
        .route(
            "/rate-limit",
            get(chimera_operator::handlers::get_rate_limit_status),
        )
        .with_state(state);

    for _ in 0..40 {
        assert!(webhook_limiter.try_acquire());
        assert!(rpc_limiter.try_acquire());
    }
    let resp = api_get(&app, "/rate-limit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let endpoints = body["endpoints"].as_array().unwrap();
    for e in endpoints {
        assert_eq!(e["status"], "throttled");
    }
    assert_eq!(body["overall_status"], "throttled");
}

#[tokio::test]
async fn rate_limit_status_warning_only_degraded() {
    use axum::routing::get;
    use axum::Router;
    use chimera_operator::handlers::OperationsState;
    use chimera_operator::monitoring::rate_limiter::RateLimiter;

    let h = build(test_config()).await;
    // 30 of 40 credits → 75% utilization → warning, not throttled.
    let webhook_limiter = Arc::new(RateLimiter::new(40, 60));
    let rpc_limiter = Arc::new(RateLimiter::new(40, 60));
    let state = Arc::new(OperationsState {
        db: h.db.clone(),
        engine: Some(Arc::new(h.engine_handle.clone())),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        price_cache: h.api_state.price_cache.clone(),
        webhook_rate_limiter: Some(webhook_limiter.clone()),
        rpc_rate_limiter: Some(rpc_limiter.clone()),
    });
    let app = Router::new()
        .route(
            "/rate-limit",
            get(chimera_operator::handlers::get_rate_limit_status),
        )
        .with_state(state);

    for _ in 0..30 {
        assert!(webhook_limiter.try_acquire());
        assert!(rpc_limiter.try_acquire());
    }
    let resp = api_get(&app, "/rate-limit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let endpoints = body["endpoints"].as_array().unwrap();
    for e in endpoints {
        assert_eq!(e["status"], "warning");
    }
    assert_eq!(body["overall_status"], "degraded");
}

#[tokio::test]
async fn health_checks_rpc_failing_with_refresh() {
    let h = build(test_config()).await;
    // Refresh forces an RPC health probe against the dead test URL → health
    // record becomes Some(unhealthy) → rpc check reports Failing.
    h.engine_handle.refresh_rpc_health().await;

    let resp = api_get(
        &h.app,
        "/api/v1/operations/health-checks",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let checks = body["checks"].as_array().unwrap();
    let rpc_check = checks.iter().find(|c| c["name"] == "rpc").unwrap();
    assert_eq!(rpc_check["status"], "failing");
    assert!(rpc_check["message"]
        .as_str()
        .unwrap()
        .contains("RPC unhealthy"));
}

#[tokio::test]
async fn rate_limit_status_zero_credit_limiter() {
    use axum::routing::get;
    use axum::Router;
    use chimera_operator::handlers::OperationsState;
    use chimera_operator::monitoring::rate_limiter::RateLimiter;

    let h = build(test_config()).await;
    // Zero-credit limiter → limit 0 → utilization falls back to 0.0 → ok.
    let state = Arc::new(OperationsState {
        db: h.db.clone(),
        engine: Some(Arc::new(h.engine_handle.clone())),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        price_cache: h.api_state.price_cache.clone(),
        webhook_rate_limiter: Some(Arc::new(RateLimiter::new(0, 60))),
        rpc_rate_limiter: Some(Arc::new(RateLimiter::new(0, 60))),
    });
    let app = Router::new()
        .route(
            "/rate-limit",
            get(chimera_operator::handlers::get_rate_limit_status),
        )
        .with_state(state);

    let resp = api_get(&app, "/rate-limit", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let endpoints = body["endpoints"].as_array().unwrap();
    for e in endpoints {
        assert_eq!(e["status"], "ok");
        assert_eq!(e["utilization_percent"], 0.0);
        assert_eq!(e["limit"], 0);
    }
    assert_eq!(body["overall_status"], "healthy");
}
