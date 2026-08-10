//! HTTP handler tests for `operator/src/handlers/market.rs`.
//!
//! The market endpoints read a `MarketRegimeDetector` from `ApiState` and
//! derive regime/volatility/trend from its SOL price history. The harness
//! builds with `market_regime_detector: None` (the 500 path); these tests
//! wire a detector seeded with controlled price series to exercise every
//! regime, volatility band, and allocation branch.

use axum::http::StatusCode;
use chimera_operator::engine::MarketRegimeDetector;
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/harness.rs"]
mod harness;

use harness::{api_get, build, build_with_market_regime, json_body, test_config};

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// Build a detector whose SOL history is `(price, hours_ago)` points, in the
/// order given (each applied via `set_price_with_time` + `update_price_history`).
async fn detector_with_history(points: &[(&str, i64)]) -> Arc<MarketRegimeDetector> {
    let now = chrono::Utc::now();
    let pc = Arc::new(PriceCache::new().unwrap());
    // Fractional hour offsets keep every timestamp distinct even when two
    // points share an integral hour (update_price_history skips non-newer
    // entries, which would silently drop the duplicate).
    let n = points.len() as i64;
    for (i, (price, hours_ago)) in points.iter().enumerate() {
        let offset = chrono::Duration::hours(*hours_ago) + chrono::Duration::minutes(i as i64 * 5);
        pc.set_price_with_time(
            SOL_MINT,
            dec(price),
            PriceSource::Jupiter,
            now - offset,
            Some(9),
        );
    }
    let detector = Arc::new(MarketRegimeDetector::new(pc));
    for _ in 0..points.len() {
        detector.update_price_history().await;
    }
    detector
}

#[tokio::test]
async fn market_endpoints_without_detector_return_500() {
    let h = build(test_config()).await;

    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn market_insufficient_history_is_neutral_unknown() {
    let h = build_with_market_regime(test_config(), Some(detector_with_history(&[]).await)).await;

    // No history → Sideways, no metrics, unknown risk.
    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["current_regime"], "neutral");
    assert!(body["confidence"].is_null());
    assert!(body["volatility_index"].is_null());
    assert!(body["trend_strength"].is_null());
    assert!(body["last_regime_change"].is_null());
    assert_eq!(body["regime_history"].as_array().unwrap().len(), 0);
    assert_eq!(body["performance_by_regime"].as_array().unwrap().len(), 0);

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["market_sentiment"], "neutral");
    assert_eq!(body["risk_level"], "unknown");
    assert!(body["liquidity_index"].is_null());
    assert_eq!(body["recommended_allocation"]["shield_percent"], 70);
    assert_eq!(body["recommended_allocation"]["spear_percent"], 30);
}

#[tokio::test]
async fn market_bull_regime_full_response() {
    // +10% over 13h → Bull.
    let h = build_with_market_regime(
        test_config(),
        Some(
            detector_with_history(&[("100.0", 13), ("105.0", 6), ("110.0", 0)]).await,
        ),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["current_regime"], "bull");
    let vol = body["volatility_index"].as_f64().unwrap();
    assert!((vol - 3.89).abs() < 0.5, "volatility ~3.89%, got {vol}");
    let trend = body["trend_strength"].as_f64().unwrap();
    assert!((trend - 10.0).abs() < 0.01, "trend +10%, got {trend}");

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["market_sentiment"], "bullish");
    assert_eq!(body["risk_level"], "low", "low vol → low risk");
    assert_eq!(body["recommended_allocation"]["shield_percent"], 60);
    assert_eq!(body["recommended_allocation"]["spear_percent"], 40);
}

#[tokio::test]
async fn market_bear_regime_allocation() {
    // -10% over 13h → Bear.
    let h = build_with_market_regime(
        test_config(),
        Some(
            detector_with_history(&[("110.0", 13), ("105.0", 6), ("100.0", 0)]).await,
        ),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["current_regime"], "bear");

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["market_sentiment"], "bearish");
    assert_eq!(body["risk_level"], "low");
    assert_eq!(body["recommended_allocation"]["shield_percent"], 80);
    assert_eq!(body["recommended_allocation"]["spear_percent"], 20);
}

#[tokio::test]
async fn market_sideways_insufficient_span() {
    // 3 points but only ~3h span → Sideways (regime + allocation) regardless
    // of the +10% move (the <12h-span branch).
    let h = build_with_market_regime(
        test_config(),
        Some(
            detector_with_history(&[("100.0", 3), ("105.0", 1), ("110.0", 0)]).await,
        ),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["current_regime"], "neutral");

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["market_sentiment"], "neutral");
    assert_eq!(body["recommended_allocation"]["shield_percent"], 70);
}

#[tokio::test]
async fn market_risk_level_bands() {
    // Medium volatility (~21.7%): [100, 60, 100] over 13h, net 0% → Sideways.
    let h = build_with_market_regime(
        test_config(),
        Some(detector_with_history(&[("100.0", 13), ("60.0", 6), ("100.0", 0)]).await),
    )
    .await;
    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    let vol = body["volatility_index"].as_f64().unwrap();
    assert!((20.0..40.0).contains(&vol), "medium band, got {vol}");
    assert_eq!(body["risk_level"], "medium");

    // High volatility (~51%): [100, 20, 100].
    let h = build_with_market_regime(
        test_config(),
        Some(detector_with_history(&[("100.0", 13), ("20.0", 6), ("100.0", 0)]).await),
    )
    .await;
    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    let vol = body["volatility_index"].as_f64().unwrap();
    assert!(vol >= 40.0, "high band, got {vol}");
    assert_eq!(body["risk_level"], "high");
}

#[tokio::test]
async fn market_zero_prices_yield_no_metrics() {
    // All-zero prices: mean == 0 → volatility None; first_price == 0 →
    // trend None; regime defaults to Sideways.
    let h = build_with_market_regime(
        test_config(),
        Some(
            detector_with_history(&[("0.0", 13), ("0.0", 6), ("0.0", 0)]).await,
        ),
    )
    .await;

    let resp = api_get(&h.app, "/api/v1/market/regime", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["current_regime"], "neutral");
    assert!(body["volatility_index"].is_null());
    assert!(body["trend_strength"].is_null());

    let resp = api_get(&h.app, "/api/v1/market/conditions", Default::default()).await;
    let body = json_body(resp).await;
    assert_eq!(body["risk_level"], "unknown");
}
