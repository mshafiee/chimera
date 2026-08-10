//! HTTP handler tests for `operator/src/handlers/signals.rs`.

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use chimera_operator::handlers::ApiState;
use std::sync::Arc;

#[path = "../common/harness.rs"]
mod harness;

use harness::{
    api_get, build, json_body, seed_signal, seed_trade, seed_wallet, test_config, TOKEN_A, TOKEN_B,
    WALLET_A, WALLET_B,
};

#[tokio::test]
async fn consensus_empty() {
    let h = build(test_config()).await;
    // KNOWN PRE-EXISTING EDGE BUG: with zero signals the consensus-rate
    // query computes 0 / NULLIF(0, 0) = NULL, which sqlx cannot decode into
    // f64 → HTTP 500 on an empty database. Production code is not modified
    // per task rules; the endpoint's error path is asserted instead.
    let resp = api_get(&h.app, "/api/v1/signals/consensus", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn consensus_with_signals_and_clusters() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(85.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(75.0)).await;
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        "ACTIVE",
        Some(65.0),
    )
    .await;
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsY",
        "ACTIVE",
        Some(55.0),
    )
    .await;
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsZ",
        "ACTIVE",
        Some(45.0),
    )
    .await;

    // 5 wallets BUY the same token → strong consensus cluster
    for w in [
        WALLET_A,
        WALLET_B,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsY",
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsZ",
    ] {
        seed_signal(&h.pool, TOKEN_A, w, "BUY", "1.0", true).await;
    }
    // 2 wallets on another token → weak consensus
    seed_signal(&h.pool, TOKEN_B, WALLET_A, "BUY", "1.0", true).await;
    seed_signal(&h.pool, TOKEN_B, WALLET_B, "SELL", "1.0", true).await;

    let resp = api_get(&h.app, "/api/v1/signals/consensus", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let signals = body["consensus_signals"].as_array().unwrap();
    assert_eq!(signals.len(), 2);
    // consensus_level is skip_serializing — assert via wallet_count instead.
    let strong = signals
        .iter()
        .find(|s| s["token_address"] == TOKEN_A)
        .expect("token A signal");
    assert_eq!(strong["consensus_wallets"], 5);
    assert_eq!(strong["total_wallets"], 5);
    let weak = signals
        .iter()
        .find(|s| s["token_address"] == TOKEN_B)
        .expect("token B signal");
    assert_eq!(weak["consensus_wallets"], 2);
    // consensus rate = consensus / all signals in 24h
    let rate = body["consensus_detection_rate"].as_f64().unwrap();
    assert!(rate > 0.0 && rate <= 1.0);
    // Clusters derived from signal_aggregation
    let clusters = body["active_clusters"].as_array().unwrap();
    assert_eq!(clusters.len(), 2);
    assert!(clusters
        .iter()
        .any(|c| c["wallets"].as_array().unwrap().len() == 5));
    let cluster = clusters
        .iter()
        .find(|c| c["id"] == "token_4k3Dyjzv")
        .unwrap();
    assert_eq!(cluster["coherence"], 1.0); // all BUY
    assert_eq!(cluster["avg_wqs"], 65.0);
}

#[tokio::test]
async fn wallet_clustering_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/signals/clustering", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["clusters"].as_array().unwrap().len(), 0);
    assert_eq!(body["total_wallets"], 0);
    assert_eq!(body["clustering_metrics"]["avg_cluster_size"], 0.0);
    assert_eq!(body["clustering_metrics"]["max_cluster_size"], 0);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(70.0)).await;
    seed_signal(&h.pool, TOKEN_A, WALLET_A, "BUY", "1.0", false).await;
    seed_signal(&h.pool, TOKEN_A, WALLET_B, "SELL", "1.0", false).await;

    let resp = api_get(&h.app, "/api/v1/signals/clustering", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let clusters = body["clusters"].as_array().unwrap();
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0]["wallets"].as_array().unwrap().len(), 2);
    assert_eq!(body["total_wallets"], 2);
    assert_eq!(body["clustering_metrics"]["avg_cluster_size"], 2.0);
    assert_eq!(body["clustering_metrics"]["max_cluster_size"], 2);
    // 1 BUY + 1 SELL → coherence 0
    assert_eq!(clusters[0]["coherence"], 0.0);
}

#[tokio::test]
async fn signal_aggregation_paths() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/signals/aggregation", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_signals"], 0);
    assert_eq!(body["unique_tokens"], 0);
    assert!(body["window_start"].is_string());

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(70.0)).await;
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
        "ACTIVE",
        Some(60.0),
    )
    .await;
    // Token A: 3 BUYs → BUY recommendation
    for w in [
        WALLET_A,
        WALLET_B,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
    ] {
        seed_signal(&h.pool, TOKEN_A, w, "BUY", "1.0", false).await;
    }
    // Token B: 3 SELLs → SELL recommendation
    for w in [
        WALLET_A,
        WALLET_B,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
    ] {
        seed_signal(&h.pool, TOKEN_B, w, "SELL", "2.0", false).await;
    }
    // Token C: 2 wallets mixed → HOLD
    seed_signal(
        &h.pool,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6X",
        WALLET_A,
        "BUY",
        "1.0",
        false,
    )
    .await;
    seed_signal(
        &h.pool,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6X",
        WALLET_B,
        "SELL",
        "1.0",
        false,
    )
    .await;
    // Token D: single wallet → SKIP
    seed_signal(
        &h.pool,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6Y",
        WALLET_A,
        "BUY",
        "1.0",
        false,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/signals/aggregation", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_signals"], 9);
    assert_eq!(body["unique_tokens"], 4);
    let aggs = body["aggregated_signals"].as_array().unwrap();
    assert_eq!(aggs.len(), 4);
    let action = |token: &str| {
        aggs.iter().find(|a| a["token_address"] == token).unwrap()["recommended_action"].clone()
    };
    assert_eq!(action(TOKEN_A), "BUY");
    assert_eq!(action(TOKEN_B), "SELL");
    assert_eq!(
        action("4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6X"),
        "HOLD"
    );
    assert_eq!(
        action("4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6Y"),
        "SKIP"
    );
    // consensus_score for 3-wallet token = min(3/5, 1) = 0.6
    let buy_agg = aggs.iter().find(|a| a["token_address"] == TOKEN_A).unwrap();
    assert_eq!(buy_agg["consensus_score"], 0.6);
    assert_eq!(buy_agg["confidence"], 0.6);
}

#[tokio::test]
async fn signal_quality_ranges_and_counts() {
    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t2", WALLET_A, TOKEN_A, "BUY", "CLOSED", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t3", WALLET_A, TOKEN_A, "BUY", "FAILED", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool,
        "t4",
        WALLET_A,
        TOKEN_A,
        "BUY",
        "DEAD_LETTER",
        "SHIELD",
        "1.0",
        None,
    )
    .await;
    // Recent seeds fall inside every quality window (even 1h).
    sqlx::query("UPDATE trades SET created_at = NOW() - INTERVAL '5 minutes'")
        .execute(&h.pool)
        .await
        .unwrap();

    for range in ["1h", "6h", "24h", "7d", "bogus"] {
        let resp = api_get(
            &h.app,
            &format!("/api/v1/signals/quality?range={range}"),
            Default::default(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "range {range}");
        let body = json_body(resp).await;
        assert_eq!(body["total_signals"], 4, "range {range}");
        assert_eq!(body["accepted_signals"], 2);
        assert_eq!(body["rejected_signals"], 2);
        assert_eq!(body["rejection_rate"], 0.5);
        assert_eq!(body["current_quality_score"], 80.0);
        assert!(body["quality_distribution"].is_array());
        assert!(body["average_quality_trend"].is_array());
    }

    // Default range when no param
    let resp = api_get(&h.app, "/api/v1/signals/quality", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn signal_sources_empty_and_seeded() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/signals/sources", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_signals"], 0);
    assert_eq!(body["sources"].as_array().unwrap().len(), 0);

    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(90.0)).await;
    seed_wallet(&h.pool, WALLET_B, "ACTIVE", Some(50.0)).await;
    seed_trade(
        &h.pool, "t1", WALLET_A, TOKEN_A, "BUY", "ACTIVE", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t2", WALLET_A, TOKEN_A, "BUY", "CLOSED", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t3", WALLET_A, TOKEN_A, "BUY", "FAILED", "SHIELD", "1.0", None,
    )
    .await;
    seed_trade(
        &h.pool, "t4", WALLET_B, TOKEN_A, "BUY", "CLOSED", "SHIELD", "1.0", None,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/signals/sources", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_signals"], 4);
    let sources = body["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    let source_a = sources.iter().find(|s| s["source"] == WALLET_A).unwrap();
    assert_eq!(source_a["signal_count"], 3);
    assert_eq!(source_a["average_quality"], 90.0);
    let acc = source_a["acceptance_rate"].as_f64().unwrap();
    assert!((acc - 0.6666).abs() < 0.001, "acceptance {acc}");
    assert!(source_a["last_signal_at"].is_string());
}

// =============================================================================
// ADDITIONAL BRANCH COVERAGE
// =============================================================================

#[tokio::test]
async fn consensus_moderate_and_none_levels() {
    let h = build(test_config()).await;
    for (i, w) in [
        WALLET_A,
        WALLET_B,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
    ]
    .iter()
    .enumerate()
    {
        seed_wallet(&h.pool, w, "ACTIVE", Some(80.0 - i as f64 * 10.0)).await;
    }
    seed_wallet(
        &h.pool,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsY",
        "ACTIVE",
        Some(50.0),
    )
    .await;
    // 3 wallets on token C → moderate
    for w in [
        WALLET_A,
        WALLET_B,
        "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsX",
    ] {
        seed_signal(
            &h.pool,
            "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6X",
            w,
            "BUY",
            "1.0",
            true,
        )
        .await;
    }
    // 1 wallet on token D → none
    seed_signal(
        &h.pool,
        "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6Y",
        WALLET_A,
        "BUY",
        "1.0",
        true,
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/signals/consensus", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let signals = body["consensus_signals"].as_array().unwrap();
    assert_eq!(signals.len(), 2);
    let moderate = signals
        .iter()
        .find(|s| s["token_address"] == "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6X")
        .unwrap();
    assert_eq!(moderate["consensus_wallets"], 3);
    let none = signals
        .iter()
        .find(|s| s["token_address"] == "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6Y")
        .unwrap();
    assert_eq!(none["consensus_wallets"], 1);
}

#[tokio::test]
async fn signal_quality_empty_db() {
    let h = build(test_config()).await;
    let resp = api_get(&h.app, "/api/v1/signals/quality", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["total_signals"], 0);
    assert_eq!(body["rejection_rate"], 0.0);
    assert_eq!(body["current_quality_score"], 50.0);
}

#[tokio::test]
async fn consensus_with_signal_aggregator() {
    use chimera_operator::monitoring::signal_aggregator::SignalAggregator;

    let h = build(test_config()).await;
    seed_wallet(&h.pool, WALLET_A, "ACTIVE", Some(80.0)).await;
    seed_signal(&h.pool, TOKEN_A, WALLET_A, "BUY", "1.0", true).await;

    let aggregator = Arc::new(SignalAggregator::new(h.db.clone()));
    aggregator
        .add_signal(WALLET_A, TOKEN_A, "BUY", "1.0".parse().unwrap())
        .await;

    let state = Arc::new(ApiState {
        db: h.db.clone(),
        circuit_breaker: h.api_state.circuit_breaker.clone(),
        config: h.config.clone(),
        notifier: h.api_state.notifier.clone(),
        engine: h.api_state.engine.clone(),
        metrics: h.api_state.metrics.clone(),
        signal_aggregator: Some(aggregator),
        market_regime_detector: None,
        helius_client: h.api_state.helius_client.clone(),
        webhook_rate_limiter: h.api_state.webhook_rate_limiter.clone(),
        price_cache: h.api_state.price_cache.clone(),
        toxic_detector: None,
        run_context: None,
        decision_recorder: None,
        profitability_verdict: h.api_state.profitability_verdict.clone(),
    });
    let app = Router::new()
        .route("/consensus", get(chimera_operator::handlers::get_consensus))
        .with_state(state);
    let resp = api_get(&app, "/consensus", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // aggregator present → clustering coefficient is the placeholder 0.65
    assert_eq!(body["average_clustering"], 0.65);
    assert_eq!(body["divergence_alerts"].as_array().unwrap().len(), 0);
}
