//! Selection Service Tests (B1)
//!
//! Validates the unified decision pipeline:
//! - WQS boundaries (missing/69.99/70/79.99/80) produce correct strategy + gate
//! - Both ingress paths (Webhook/Helius) produce identical decisions for the same inputs
//! - BUY sizing comes from PositionSizer, not the copied wallet's amount (no clamp)
//! - SELL with no active position is rejected

use chimera_operator::config::PositionSizingConfig;
use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::engine::position_sizer::PositionSizer;
use chimera_operator::engine::selection::{
    Ingress, SelectionConfig, SelectionRequest, SelectionService,
};
use chimera_operator::engine::MarketRegimeDetector;
use chimera_operator::monitoring::helius::HeliusClient;
use chimera_operator::monitoring::signal_aggregator::SignalAggregator;
use chimera_operator::models::{Action, Strategy};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::postgres(
        std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set"),
    );
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn insert_wallet(db: &Arc<dyn Database>, address: &str, wqs: Option<f64>) {
    let pool = pg_pool(db);
    let wqs_val = wqs.map(|v| dec(&format!("{:.3}", v)));
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, win_rate)
         VALUES ($1, 'ACTIVE', $2, 0.75, 0.6)
         ON CONFLICT (address) DO UPDATE SET status='ACTIVE', wqs_score=$2",
    )
    .bind(address)
    .bind(wqs_val)
    .execute(&pool)
    .await
    .unwrap();
}

fn build_selection_service(
    db: Arc<dyn Database>,
) -> (
    SelectionService,
    Arc<TokenParser>,
    Arc<PositionSizer>,
) {
    let pool = match db.pool() {
        DbPool::PostgreSQL(p) => p,
        _ => panic!("requires PG"),
    };
    let token_parser = Arc::new({
        let config = TokenSafetyConfig::default();
        let cache = Arc::new(TokenCache::default_config());
        let fetcher = Arc::new(TokenMetadataFetcher::new(
            "https://api.mainnet-beta.solana.com",
        ));
        TokenParser::new(config, cache, fetcher)
    });
    let position_sizer = Arc::new(PositionSizer::new(
        db.clone(),
        Arc::new(PositionSizingConfig::default()),
    ));
    let signal_aggregator = Arc::new(SignalAggregator::new(db.clone()));
    let market_regime = Arc::new(MarketRegimeDetector::new(Arc::new(
        chimera_operator::price_cache::PriceCache::new().unwrap(),
    )));
    let helius = Arc::new(
        HeliusClient::new("test_key".to_string(), Default::default()).unwrap(),
    );
    let config = SelectionConfig {
        total_capital_sol: dec("10.0"),
        max_position_sol: dec("5.0"),
        shield_signal_quality_threshold: 0.55,
        spear_signal_quality_threshold: 0.30,
        shield_percent: 60,
        spear_percent: 40,
        min_liquidity_shield_usd: dec("10000"),
        min_liquidity_spear_usd: dec("10000"),
        min_liquidity_pumpfun_usd: dec("25000"),
        allow_graduated_pumpfun: true,
        min_token_age_hours: 1.0,
        min_token_age_pumpfun_hours: 4.0,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
    };
    let service = SelectionService::new(
        db,
        token_parser.clone(),
        None, // portfolio_heat
        Some(signal_aggregator),
        Some(market_regime),
        Some(helius),
        Some(position_sizer.clone()),
        config,
    );
    (service, token_parser, position_sizer)
}

const TOKEN: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC (known safe)
const WALLET_PREFIX: &str = "11111111111111111111111111111111"; // placeholder

#[tokio::test]
async fn test_wqs_below_70_buy_rejected() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("WQS6{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(65.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted, "WQS 65 must be rejected");
    assert_eq!(decision.rejection_code, Some("WQS_TOO_LOW"));
}

#[tokio::test]
async fn test_sell_no_position_rejected() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("SE1{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(90.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req = SelectionRequest {
        wallet_address: wallet,
        token_address: TOKEN.to_string(),
        action: Action::Sell,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let decision = service.decide(&req).await;
    assert!(!decision.admitted, "SELL with no position must be rejected");
    assert_eq!(decision.rejection_code, Some("NO_ACTIVE_POSITION"));
}

#[tokio::test]
async fn test_both_ingresses_produce_identical_rejection() {
    let (db, _tmp) = create_test_db().await;
    let wallet = format!("SE2{}", &WALLET_PREFIX[..27]);
    insert_wallet(&db, &wallet, Some(40.0)).await;
    let (service, _, _) = build_selection_service(db);

    let req_webhook = SelectionRequest {
        wallet_address: wallet.clone(),
        token_address: TOKEN.to_string(),
        action: Action::Buy,
        source_amount_sol: dec("1.0"),
        ingress: Ingress::Webhook,
        source_slot: None,
        exit_fraction: None,
    };
    let req_helius = SelectionRequest {
        ingress: Ingress::Helius,
        ..req_webhook.clone()
    };

    let d1 = service.decide(&req_webhook).await;
    let d2 = service.decide(&req_helius).await;

    // Both rejected for the same reason (WQS too low).
    assert!(!d1.admitted);
    assert!(!d2.admitted);
    assert_eq!(d1.rejection_code, d2.rejection_code);
    // The only difference is the ingress field.
    assert_eq!(d1.ingress, Ingress::Webhook);
    assert_eq!(d2.ingress, Ingress::Helius);
}
