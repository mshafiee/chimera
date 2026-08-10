//! HTTP handler tests for `operator/src/handlers/webhook_lifecycle.rs`.
//!
//! All lifecycle manager operations run against the local Helius mock
//! (dry-run config for cleanup/reconcile; register hits the mock directly).

use axum::http::StatusCode;
use serde_json::json;

#[path = "../common/harness.rs"]
mod harness;

use chimera_operator::middleware::Role;
use harness::{
    api_get, api_post, auth_headers, build, json_body, seed_wallet, seed_wallet_monitoring,
    test_config, WALLET_A, WALLET_B,
};

#[tokio::test]
async fn webhook_stats_empty_and_seeded() {
    let h = build(test_config()).await;

    // Forbidden for readonly
    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/stats",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/stats",
        auth_headers(Role::Operator),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["total_webhooks"], 0);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("wh-1"),
        true,
        Some("healthy"),
        Some("active"),
        1,
        None,
    )
    .await;

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/stats",
        auth_headers(Role::Operator),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["data"]["total_webhooks"], 1);
    assert_eq!(body["data"]["active_webhooks"], 1);
}

#[tokio::test]
async fn bulk_register_webhooks_paths() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Operator);

    // Forbidden
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-register",
        auth_headers(Role::Readonly),
        json!({"wallets": [WALLET_A]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Empty wallet list → 400
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-register",
        headers.clone(),
        json!({"wallets": []}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Valid + invalid address mix → processed totals reflect per-wallet results
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-register",
        headers.clone(),
        json!({"wallets": [WALLET_A, "not-an-address"]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["total"], 2);
    assert_eq!(body["data"]["succeeded"], 1);
    assert_eq!(body["data"]["failed"], 1);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Processed 2 wallets"));
}

#[tokio::test]
async fn bulk_cleanup_webhooks_paths() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Operator);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-cleanup",
        headers.clone(),
        json!({"wallets": []}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Wallet without monitoring row → cleanup fails per-wallet
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-cleanup",
        headers.clone(),
        json!({"wallets": [WALLET_A]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["failed"], 1);
    assert!(body["message"].as_str().unwrap().contains("1 failed"));

    // Wallet WITH monitoring row + webhook id → dry-run cleanup succeeds
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(80.0)).await;
    seed_wallet_monitoring(
        &h.pool,
        WALLET_B,
        Some("mock-webhook-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-cleanup",
        headers.clone(),
        json!({"wallets": [WALLET_B]}),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["succeeded"], 1);
}

#[tokio::test]
async fn manual_reconcile_and_health_check() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Operator);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/reconcile",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert!(body["data"]["registered"].is_number());
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Reconciliation completed"));

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/health-check",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["success"], true);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Health check completed"));
}

#[tokio::test]
async fn webhook_audit_log_filters() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(80.0)).await;

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/audit",
        auth_headers(Role::Readonly),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Seed audit rows directly (action CHECK: register/delete/toggle/...).
    sqlx::query(
        "INSERT INTO webhook_lifecycle_audit (wallet_address, action, status, webhook_id, details) \
         VALUES ($1, 'register', 'success', 'wh-1', 'registered'), \
                ($2, 'delete', 'failed', NULL, 'deleted')",
    )
    .bind(WALLET_A)
    .bind(WALLET_B)
    .execute(&h.pool)
    .await
    .unwrap();

    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/audit",
        auth_headers(Role::Operator),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert!(body["message"].as_str().unwrap().contains("Retrieved 2"));

    // wallet filter
    let resp = api_get(
        &h.app,
        &format!(
            "/api/v1/monitoring/webhooks/audit?wallet_address={}",
            WALLET_A
        ),
        auth_headers(Role::Operator),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["action"], "register");

    // action + status filter
    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/audit?action=delete&status=failed",
        auth_headers(Role::Operator),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // limit param
    let resp = api_get(
        &h.app,
        "/api/v1/monitoring/webhooks/audit?limit=1",
        auth_headers(Role::Operator),
    )
    .await;
    let body = json_body(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn retry_webhook_registration_paths() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;

    // Forbidden for readonly
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/webhooks/{}/retry", WALLET_A),
        auth_headers(Role::Readonly),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Valid address → registers against the mock → 200
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/webhooks/{}/retry", WALLET_A),
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Invalid address → registration result failure → 500
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/not-an-address/retry",
        auth_headers(Role::Operator),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn toggle_wallet_webhook_paths() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;

    // Forbidden for readonly
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/webhooks/{}/toggle", WALLET_A),
        auth_headers(Role::Readonly),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // No monitoring row → toggle error → 500
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/webhooks/{}/toggle", WALLET_A),
        auth_headers(Role::Operator),
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // With monitoring row + webhook id → mock toggle → 200
    seed_wallet_monitoring(
        &h.pool,
        WALLET_A,
        Some("mock-webhook-1"),
        true,
        Some("healthy"),
        Some("active"),
        0,
        None,
    )
    .await;
    let resp = api_post(
        &h.app,
        &format!("/api/v1/monitoring/webhooks/{}/toggle", WALLET_A),
        auth_headers(Role::Operator),
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

#[tokio::test]
async fn webhook_ops_require_webhook_url() {
    let mut config = test_config();
    config.monitoring.as_mut().unwrap().helius_webhook_url = None;
    let h = build(config).await;
    let headers = auth_headers(Role::Operator);

    // get_webhook_url validation error → 400 on every lifecycle op.
    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-register",
        headers.clone(),
        json!({"wallets": [WALLET_A]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/reconcile",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/health-check",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhook_ops_forbidden_for_readonly() {
    let h = build(test_config()).await;
    let headers = auth_headers(Role::Readonly);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/bulk-cleanup",
        headers.clone(),
        json!({"wallets": [WALLET_A]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/reconcile",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = api_post(
        &h.app,
        "/api/v1/monitoring/webhooks/health-check",
        headers.clone(),
        json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
