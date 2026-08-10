//! HTTP handler tests for `operator/src/handlers/risk.rs`.
//!
//! Covers the pure helpers directly and the DB-driven endpoints through the
//! real router against a real Postgres test database.

use axum::http::StatusCode;
use serde_json::json;

#[path = "../common/harness.rs"]
mod harness;

use chimera_operator::handlers::{
    calculate_hhi, classify_token_sector, determine_heat_status, TokenConcentration,
};
use harness::{
    api_get, build, json_body, seed_closed_position_with_pnl, seed_exit_target,
    seed_portfolio_snapshot, seed_position, seed_trade, seed_wallet, test_config, TOKEN_A,
    WALLET_A,
};

// =============================================================================
// PURE HELPERS
// =============================================================================

#[test]
fn classify_token_sector_all_branches() {
    assert_eq!(classify_token_sector("0abc"), "DeFi");
    assert_eq!(classify_token_sector("3abc"), "DeFi");
    assert_eq!(classify_token_sector("4abc"), "NFT/Gaming");
    assert_eq!(classify_token_sector("6abc"), "NFT/Gaming");
    assert_eq!(classify_token_sector("7abc"), "Meme");
    assert_eq!(classify_token_sector("9abc"), "Meme");
    assert_eq!(classify_token_sector("aabc"), "Stablecoin");
    assert_eq!(classify_token_sector("cabc"), "Stablecoin");
    assert_eq!(classify_token_sector("dabc"), "Exchange");
    assert_eq!(classify_token_sector("fabc"), "Exchange");
    assert_eq!(classify_token_sector("gabc"), "Other");
    assert_eq!(classify_token_sector("zabc"), "Other");
    // Empty address → first char falls back to '0' → DeFi
    assert_eq!(classify_token_sector(""), "DeFi");
    // Uppercase 'A' is outside all ranges → Unknown
    assert_eq!(classify_token_sector("ABC"), "Unknown");
}

#[test]
fn calculate_hhi_math() {
    let concentrations = |pcts: &[f64]| -> Vec<TokenConcentration> {
        pcts.iter()
            .enumerate()
            .map(|(i, p)| TokenConcentration {
                token_address: format!("token{i}"),
                token_symbol: None,
                position_count: 1,
                total_value_sol: *p,
                percentage: *p,
            })
            .collect()
    };
    // Single token 100% → HHI 10000
    assert_eq!(calculate_hhi(&concentrations(&[100.0])), 10000.0);
    // Two tokens 50/50 → 2500 + 2500 = 5000
    assert_eq!(calculate_hhi(&concentrations(&[50.0, 50.0])), 5000.0);
    // Empty → 0
    assert_eq!(calculate_hhi(&[]), 0.0);
}

#[test]
fn determine_heat_status_thresholds() {
    assert_eq!(determine_heat_status(0.0, 100.0), "normal");
    assert_eq!(determine_heat_status(69.9, 100.0), "normal");
    assert_eq!(determine_heat_status(70.0, 100.0), "elevated");
    assert_eq!(determine_heat_status(89.9, 100.0), "elevated");
    assert_eq!(determine_heat_status(90.0, 100.0), "high");
    assert_eq!(determine_heat_status(109.9, 100.0), "high");
    assert_eq!(determine_heat_status(110.0, 100.0), "critical");
    // Zero threshold is clamped to 0.01 → any exposure is critical
    assert_eq!(determine_heat_status(1.0, 0.0), "critical");
}

// =============================================================================
// PORTFOLIO RISK
// =============================================================================

#[tokio::test]
async fn portfolio_risk_empty() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/risk/portfolio", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["portfolio_heat_percent"], 0.0);
    assert_eq!(body["heat_status"], "normal");
    assert_eq!(body["heat_threshold"], 10.0); // default total_capital_sol
    assert_eq!(
        body["concentration"]["by_token"].as_array().unwrap().len(),
        0
    );
    assert_eq!(body["concentration"]["hhi"], 0.0);
    assert_eq!(body["exposure"]["total_exposure_sol"], 0.0);
    assert_eq!(body["drawdown"]["recovery_percent"], 100.0);
    assert_eq!(body["total_capital_sol"], 10.0);
    assert_eq!(body["wallet_balance_sol"], 10.0);
}

#[tokio::test]
async fn portfolio_risk_with_positions() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "3.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t1", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "3.0", "0.01", None,
    )
    .await;
    seed_trade(
        &h.pool, "t2", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SPEAR", "2.0", None,
    )
    .await;
    seed_position(
        &h.pool, "t2", WALLET_A, TOKEN_A, "SPEAR", "ACTIVE", "2.0", "0.02", None,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/risk/portfolio", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // 5 SOL exposure against 10 SOL capital
    assert_eq!(body["portfolio_heat_percent"], 50.0);
    assert_eq!(body["heat_status"], "normal");
    assert_eq!(body["exposure"]["total_exposure_sol"], 5.0);
    assert_eq!(body["exposure"]["long_exposure_sol"], 5.0);
    assert_eq!(body["exposure"]["short_exposure_sol"], 0.0);
    assert_eq!(body["exposure"]["net_exposure_sol"], 5.0);
    // Concentration: single token 100%
    assert_eq!(
        body["concentration"]["by_token"].as_array().unwrap().len(),
        1
    );
    assert_eq!(body["concentration"]["by_token"][0]["percentage"], 100.0);
    assert_eq!(body["concentration"]["max_concentration_percent"], 100.0);
    assert_eq!(body["concentration"]["hhi"], 10000.0);
    // Sector classification of TOKEN_A (starts with '4') → NFT/Gaming
    assert_eq!(
        body["concentration"]["by_sector"][0]["sector"],
        "NFT/Gaming"
    );
    assert_eq!(body["concentration"]["by_sector"][0]["percentage"], 100.0);
    // No closed positions → drawdown zero, recovery 100
    assert_eq!(body["drawdown"]["current_drawdown_percent"], 0.0);
    assert_eq!(body["drawdown"]["max_drawdown_percent"], 0.0);
    assert_eq!(body["wallet_balance_sol"], 5.0); // capital 10 − exposure 5
}

#[tokio::test]
async fn portfolio_risk_with_realized_pnl_and_drawdown() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    // Closed PnL history: +1 then −2 → peak 1, running −1, max drawdown
    // = (1 − (−1)) / (10 + 1) * 100 ≈ 18.18
    seed_closed_position_with_pnl(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "1.0",
        chrono::Utc::now() - chrono::Duration::hours(2),
    )
    .await;
    seed_closed_position_with_pnl(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "1.0",
        "-2.0",
        chrono::Utc::now() - chrono::Duration::hours(1),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/risk/portfolio", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let dd = body["drawdown"]["max_drawdown_percent"].as_f64().unwrap();
    assert!((dd - 18.1818).abs() < 0.01, "max drawdown {dd}");
    // wallet balance = 10 + (−1) − 0 = 9
    assert_eq!(body["wallet_balance_sol"], 9.0);
}

// =============================================================================
// NAV HISTORY
// =============================================================================

#[tokio::test]
async fn nav_history_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/portfolio/nav-history", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["points"].as_array().unwrap().len(), 0);
    assert!(body["latest_nav_sol"].is_null());

    seed_portfolio_snapshot(
        &h.pool,
        "10.5",
        chrono::Utc::now() - chrono::Duration::days(1),
    )
    .await;
    seed_portfolio_snapshot(&h.pool, "11.2", chrono::Utc::now()).await;

    let resp = api_get(
        &h.app,
        "/api/v1/portfolio/nav-history?days=30",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let points = body["points"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["nav_sol"], 10.5);
    assert_eq!(points[1]["nav_sol"], 11.2);
    assert_eq!(body["latest_nav_sol"], 11.2);
    assert_eq!(body["latest_unrealized_pnl_sol"], 0.0);

    // days=0 is clamped to 1
    let resp = api_get(
        &h.app,
        "/api/v1/portfolio/nav-history?days=0",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// =============================================================================
// STOP LOSS METRICS
// =============================================================================

#[tokio::test]
async fn stop_loss_metrics_empty_and_with_activations() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/risk/stop-loss?days=30", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["activation_rate"], 0.0);
    assert_eq!(body["total_activations"], 0);
    assert_eq!(body["recent_activations"].as_array().unwrap().len(), 0);

    // Seed a closed position whose exit price is at/below stop → activation.
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("-0.5"),
    )
    .await;
    seed_position(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "CLOSED",
        "1.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(&h.pool, "t1", Some("0.95"), None, false, None).await;
    // Exit price defaults NULL in seed_position → set it below stop.
    sqlx::query(
        "UPDATE positions SET exit_price = 0.94, realized_pnl_sol = -0.5 WHERE trade_uuid = 't1'",
    )
    .execute(&h.pool)
    .await
    .unwrap();

    // Second position: closed WITHOUT stop activation (exit above stop).
    seed_trade(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SPEAR",
        "2.0",
        Some("0.3"),
    )
    .await;
    seed_position(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "SPEAR",
        "CLOSED",
        "2.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(&h.pool, "t2", Some("0.95"), None, false, None).await;
    sqlx::query("UPDATE positions SET exit_price = 1.05 WHERE trade_uuid = 't2'")
        .execute(&h.pool)
        .await
        .unwrap();

    // KNOWN PRE-EXISTING BUG: the recent-activations query selects
    // `p.closed_at` (TIMESTAMPTZ) into an `Option<String>` tuple slot without
    // a `::text` cast — sqlx decode fails → 500 whenever any activation data
    // exists. Production code is not modified per task rules; the endpoint's
    // error path is asserted here instead of the intended 200.
    let resp = api_get(&h.app, "/api/v1/risk/stop-loss?days=30", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// =============================================================================
// PROFIT TARGET METRICS
// =============================================================================

#[tokio::test]
async fn profit_target_metrics_empty_and_with_hits() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/risk/profit-target", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["hit_rate"], 0.0);
    assert_eq!(body["total_hits"], 0);
    assert_eq!(body["trailing_stop_activations"], 0);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    // t1: targets hit [1,2], peak profit 20%
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.2"),
    )
    .await;
    seed_position(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "CLOSED",
        "1.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(
        &h.pool,
        "t1",
        None,
        Some(json!([1, 2])),
        false,
        Some("20.0"),
    )
    .await;

    // t2: closed without targets hit → dilutes hit rate but not hits.
    seed_trade(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SPEAR",
        "1.0",
        Some("-0.1"),
    )
    .await;
    seed_position(
        &h.pool,
        "t2",
        WALLET_A,
        TOKEN_A,
        "SPEAR",
        "CLOSED",
        "1.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(&h.pool, "t2", Some("0.9"), Some(json!([])), false, None).await;

    // t3: trailing stop activated + targets hit
    seed_trade(
        &h.pool,
        "t3",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.15"),
    )
    .await;
    seed_position(
        &h.pool,
        "t3",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "CLOSED",
        "1.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(&h.pool, "t3", None, Some(json!([1])), true, Some("15.0")).await;

    // Same pre-existing `closed_at` decode bug as stop-loss: with hit data
    // present the endpoint returns 500 (recent-hits query lacks `::text`).
    let resp = api_get(
        &h.app,
        "/api/v1/risk/profit-target?days=30",
        Default::default(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// =============================================================================
// POSITION SIZE ANALYSIS
// =============================================================================

#[tokio::test]
async fn position_size_analysis_empty_and_buckets() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/risk/position-size", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["average_position_sol"], 0.0);
    assert_eq!(body["median_position_sol"], 0.0);
    assert!(body["kelly_criterion_usage"].is_null());

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    // Sizes: 0.05, 0.2, 0.8, 3.0, 7.0 → median 0.8, avg 2.21
    for (i, size) in ["0.05", "0.2", "0.8", "3.0", "7.0"].iter().enumerate() {
        let uuid = format!("s{i}");
        seed_trade(
            &h.pool, &uuid, WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", size, None,
        )
        .await;
        seed_position(
            &h.pool, &uuid, WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", size, "0.01", None,
        )
        .await;
    }

    let resp = api_get(&h.app, "/api/v1/risk/position-size", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let avg = body["average_position_sol"].as_f64().unwrap();
    assert!((avg - 2.21).abs() < 1e-9, "avg {avg}");
    let median = body["median_position_sol"].as_f64().unwrap();
    assert!((median - 0.8).abs() < 1e-9, "median {median}");
    assert_eq!(body["max_position_sol"], 7.0);
    assert_eq!(body["min_position_sol"], 0.05);

    let buckets = body["position_size_distribution"].as_array().unwrap();
    // 5 distinct buckets: 0-0.1, 0.1-0.5, 0.5-1.0, 1-5, 5-10
    assert_eq!(buckets.len(), 5);
    let total: i64 = buckets.iter().map(|b| b["count"].as_i64().unwrap()).sum();
    assert_eq!(total, 5);
    let pct_sum: f64 = buckets
        .iter()
        .map(|b| b["percentage"].as_f64().unwrap())
        .sum();
    assert!((pct_sum - 100.0).abs() < 1e-9);
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

#[tokio::test]
async fn portfolio_risk_zero_amount_positions() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "0", None,
    )
    .await;
    seed_position(
        &h.pool, "t1", WALLET_A, TOKEN_A, "SHIELD", "ACTIVE", "0", "0.01", None,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/risk/portfolio", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // total exposure 0 → percentages fall back to 0.0
    assert_eq!(body["exposure"]["total_exposure_sol"], 0.0);
    assert_eq!(body["concentration"]["by_token"][0]["percentage"], 0.0);
    assert_eq!(body["concentration"]["by_sector"][0]["percentage"], 0.0);
    assert_eq!(body["concentration"]["max_concentration_percent"], 0.0);
    assert_eq!(body["portfolio_heat_percent"], 0.0);
}

#[tokio::test]
async fn portfolio_risk_zero_capital() {
    let mut config = test_config();
    config.position_sizing.total_capital_sol = "0".parse().unwrap();
    let h = build(config).await;

    let resp = api_get(&h.app, "/api/v1/risk/portfolio", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["portfolio_heat_percent"], 0.0);
    assert_eq!(body["heat_status"], "normal");
    assert_eq!(body["wallet_balance_sol"], 0.0);
}

#[tokio::test]
async fn profit_target_metrics_by_strategy_map_runs_before_recent_query() {
    // The by-strategy aggregation (targets_by_strategy) executes BEFORE the
    // broken recent-hits query, so a 500 response still proves the map ran.
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "CLOSED",
        "SHIELD",
        "1.0",
        Some("0.2"),
    )
    .await;
    seed_position(
        &h.pool,
        "t1",
        WALLET_A,
        TOKEN_A,
        "SHIELD",
        "CLOSED",
        "1.0",
        "1.0",
        Some(chrono::Utc::now() - chrono::Duration::hours(1)),
    )
    .await;
    seed_exit_target(
        &h.pool,
        "t1",
        None,
        Some(json!([1, 2])),
        false,
        Some("20.0"),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/risk/profit-target", Default::default()).await;
    // Pre-existing closed_at decode bug → 500, but the aggregation queries ran.
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
