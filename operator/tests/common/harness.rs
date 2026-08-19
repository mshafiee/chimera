//! Shared harness for HTTP handler tests.
//!
//! Builds the REAL axum router (mirroring api/src/main.rs) with a real
//! per-test Postgres database and real state objects (CircuitBreaker,
//! EngineHandle, MetricsState, MonitoringState, PriceCache). External
//! services (Helius) are redirected to a local mock HTTP server via
//! `HELIUS_API_BASE_URL` / `HELIUS_RPC_BASE_URL` so the handlers' HTTP
//! call paths execute end-to-end without network access.
//!
//! Each test file includes this module via `#[path = "../common/harness.rs"]`.

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware as axum_middleware,
    response::Response,
    routing::{get, post, put},
    Router,
};
use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{Database, DbPool, InsertPosition, InsertTrade};
use chimera_operator::engine::{Engine, EngineHandle, SelectionConfig};
use chimera_operator::handlers::{
    bulk_cleanup_webhooks, bulk_register_webhooks, debug_backtest_smoke, disable_wallet_monitoring,
    enable_wallet_monitoring, export_trades, get_budget_status, get_cache_stats, get_config,
    get_conviction_allocation, get_cost_metrics, get_database_performance,
    get_health_check_details, get_market_conditions, get_market_regime, get_monitoring_status,
    get_nav_history, get_performance_metrics, get_portfolio_risk, get_position,
    get_position_size_analysis, get_profit_target_metrics, get_rate_limit_status,
    get_reconciliation_history, get_reconciliation_stats, get_reconciliation_status,
    get_request_rate, get_resources, get_rpc_latency, get_scout_metrics, get_scout_status,
    get_secrets, get_shadow_leaderboard, get_signal_aggregation, get_signal_quality,
    get_signal_sources, get_stop_loss_metrics, get_strategy_performance, get_trade_latency,
    get_wallet, get_wallet_monitoring_states, get_webhook_audit_log, get_webhook_stats,
    get_wqs_distribution, helius_webhook_handler, list_config_audit, list_dead_letter_queue,
    list_positions, list_trades, list_wallets, manual_health_check, manual_reconcile_webhooks,
    reset_circuit_breaker, resolve_discrepancy, retry_dead_letter_item, retry_webhook_registration,
    toggle_wallet_webhook, trigger_reconciliation, trigger_scout_run, trip_circuit_breaker,
    update_config, update_reconciliation_metrics, update_secret_rotation_metrics, update_wallet,
    ApiState, OperationsState,
};
use chimera_operator::middleware::{bearer_auth, AuthState, Role};
use chimera_operator::monitoring::{rate_limiter::RateLimiter, HeliusClient, MonitoringState};
use chimera_operator::notifications::CompositeNotifier;
use chimera_operator::price_cache::PriceCache;
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tower::ServiceExt;

/// DB creation helpers live in `common/mod.rs`; harness.rs sits in the same
/// directory so the relative `#[path]` resolves there from any including file.
#[path = "mod.rs"]
mod common;

/// Convenience re-export of the DB guard type so tests can name it.
pub use common::TestDbGuard;

/// Parse a decimal seed value; panics on malformed test data.
fn dec(input: &str) -> Decimal {
    Decimal::from_str(input).expect("valid decimal seed")
}

// =============================================================================
// HELIUS MOCK SERVER
// =============================================================================

/// Start (once per test binary) a local mock of the Helius API and point
/// `HELIUS_API_BASE_URL` at it.
///
/// The mock implements:
/// - JSON-RPC `getTransaction`: `result` is non-null unless the signature
///   contains `missing`.
/// - `POST /v0/webhooks`: returns a fresh `webhookID`.
/// - `DELETE /v0/webhooks/{id}`: 200 OK.
///
/// Only `HELIUS_API_BASE_URL` is set (captured at HeliusClient construction;
/// used by register/delete webhook calls). `HELIUS_RPC_BASE_URL` is
/// deliberately NOT touched: it is read at call time by
/// `verify_signature_exists` and the pre-existing `helius_rpc_verify_tests`
/// assert its default value — polluting it would break those tests.
pub fn helius_mock_base() -> String {
    static MOCK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MOCK.get_or_init(|| {
        let (url, _shutdown) = spawn_mock_server();
        std::env::set_var("HELIUS_API_BASE_URL", url.clone());
        url
    })
    .clone()
}

fn spawn_mock_server() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::net::TcpListener;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("mock server runtime");
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let handle = rt.spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let _ = tx.send(format!("http://{addr}"));
        let router = mock_router();
        axum::serve(listener, router).await.expect("mock server");
    });

    let url = rx.recv().expect("mock url");
    // The mock must outlive the tests; the runtime keeps running detached.
    std::mem::forget(rt);
    (url, handle)
}

fn mock_router() -> Router {
    use axum::extract::Path;

    async fn rpc(body: String) -> (StatusCode, String) {
        // JSON-RPC: respond with a found transaction unless the signature
        // literally contains "missing" (then result: null).
        let contains_missing = body.contains("missing");
        let result = if contains_missing {
            serde_json::Value::Null
        } else {
            json!({"slot": 300000000, "meta": {"err": null}})
        };
        (
            StatusCode::OK,
            json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string(),
        )
    }

    async fn register_webhook() -> (StatusCode, String) {
        (
            StatusCode::OK,
            json!({"webhookID": format!("mock-webhook-{}", uuid::Uuid::new_v4())}).to_string(),
        )
    }

    async fn delete_webhook(Path(_id): Path<String>) -> StatusCode {
        StatusCode::OK
    }

    async fn toggle_webhook(Path(_id): Path<String>) -> StatusCode {
        StatusCode::OK
    }

    async fn list_webhooks() -> (StatusCode, String) {
        (StatusCode::OK, "[]".to_string())
    }

    Router::new()
        .route("/", post(rpc))
        .route("/webhooks", post(register_webhook).get(list_webhooks))
        .route("/webhooks/:id", axum::routing::delete(delete_webhook))
        .route("/webhooks/:id/toggle", axum::routing::patch(toggle_webhook))
}

// =============================================================================
// CONFIG HELPERS
// =============================================================================

/// Base test config: paper trade mode, monitoring configured with a webhook
/// URL, minimal notification settings.
pub fn test_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.trade_mode = chimera_operator::config::TradeMode::Paper;
    // validate() requires a >=32-char webhook secret; default is empty.
    config.security.webhook_secret =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
    // Keep any handler-triggered RPC work fail-fast instead of hitting mainnet.
    config.rpc.primary_url = "http://127.0.0.1:1".to_string();
    let mut monitoring = chimera_operator::config::MonitoringConfig::default();
    monitoring.enabled = true;
    monitoring.helius_api_key = Some("test-api-key".to_string());
    monitoring.helius_webhook_url = Some("https://example.invalid/webhook".to_string());
    // Defaults are enforce=true; the harness base config is dry-run so webhook
    // events survive RPC verification (the test key can never verify on-chain).
    monitoring.helius_auth_enforce = false;
    monitoring.rpc_verify_enforce = false;
    monitoring.webhook_lifecycle = Some(chimera_operator::config::WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        helius_reconciliation_enabled: false,
        helius_dry_run: true,
    });
    config.monitoring = Some(monitoring);
    config.notifications.rules.wallet_promoted = true;
    config
}

// =============================================================================
// HARNESS
// =============================================================================

#[allow(dead_code)] // variant fields are intentionally public for tests
pub struct Harness {
    pub db: Arc<dyn Database>,
    pub _guard: TestDbGuard,
    pub pool: Pool<Postgres>,
    pub config: Arc<tokio::sync::RwLock<AppConfig>>,
    pub config_arc: Arc<AppConfig>,
    pub api_state: Arc<ApiState>,
    pub operations_state: Arc<OperationsState>,
    pub monitoring_state: Arc<MonitoringState>,
    pub engine_handle: EngineHandle,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub notifier: Arc<CompositeNotifier>,
    pub metrics: Arc<chimera_operator::metrics::MetricsState>,
    pub price_cache: Arc<PriceCache>,
    pub webhook_rate_limiter: Arc<RateLimiter>,
    pub rpc_rate_limiter: Arc<RateLimiter>,
    pub helius_client: Arc<HeliusClient>,
    pub token_parser: Arc<TokenParser>,
    pub selection_service: Arc<chimera_operator::engine::SelectionService>,
    pub app: Router,
}

/// Build a full test harness: real DB + real router with all handler routes.
pub async fn build(config: AppConfig) -> Harness {
    build_with_market_regime(config, None).await
}

/// Like [`build`] but with an optional market-regime detector wired into
/// `ApiState` (the default is `None` — the handlers' "not initialized" 500
/// path). The detector is constructed with the same `price_cache` the
/// harness uses, so seeded prices are visible to it.
pub async fn build_with_market_regime(
    config: AppConfig,
    market_regime_detector: Option<Arc<chimera_operator::engine::MarketRegimeDetector>>,
) -> Harness {
    let (db, guard) = common::create_test_pg_db().await;
    let pool = match db.pool() {
        DbPool::PostgreSQL(p) => p.clone(),
    };

    let config_arc = Arc::new(config.clone());
    let config_lock = Arc::new(tokio::sync::RwLock::new(config));

    let notifier = Arc::new(CompositeNotifier::new());

    let metrics = Arc::new(chimera_operator::metrics::MetricsState::new().expect("metrics"));
    let price_cache = Arc::new(PriceCache::new().expect("price cache"));

    let circuit_breaker = Arc::new(
        CircuitBreaker::new(
            config_arc.circuit_breakers.clone(),
            db.clone(),
            config_arc.position_sizing.total_capital_sol,
        )
        .with_price_cache(price_cache.clone()),
    );

    let (engine, engine_handle) = Engine::new(config_arc.as_ref().clone(), db.clone());

    // SelectionService: shared by the ApiState-independent monitoring path.
    let selection_config = SelectionConfig {
        total_capital_sol: config_arc.position_sizing.total_capital_sol,
        max_position_sol: config_arc.position_sizing.max_size_sol,
        shield_signal_quality_threshold: config_arc.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config_arc.strategy.spear_signal_quality_threshold,
        shield_percent: config_arc.strategy.shield_percent,
        spear_percent: config_arc.strategy.spear_percent,
        min_liquidity_shield_usd: config_arc.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config_arc.token_safety.min_liquidity_spear_usd,
        min_liquidity_pumpfun_usd: config_arc.token_safety.min_liquidity_pumpfun_usd,
        allow_graduated_pumpfun: config_arc.token_safety.allow_graduated_pumpfun,
        min_token_age_hours: config_arc.token_safety.min_token_age_hours,
        min_token_age_pumpfun_hours: config_arc.token_safety.min_token_age_pumpfun_hours,
        min_token_age_proven_hours: 0.0,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec!(0.10),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven: false,
        min_proven_trades: 10,
        require_proven_positive_pnl: false,
        mirror_gate_enabled: false,
        mirror_gate_min_avg_pct: dec!(1.5),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
        wallet_tstat_enabled: false,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
        shadow_proven_enabled: false,
        shadow_proven_min_samples: 20,
        shadow_proven_min_total_pnl_sol: 2.0,
        token_velocity_gate_enabled: false,
        token_min_liquidity_velocity: 0.10,
        token_max_curve_completion: 0.85,
        cluster_gate_enabled: false,
        cluster_min_profitable_wallets: 3,
        averaging_down_enabled: false,
        averaging_down_window_hours: 12,
        averaging_down_min_buys: 2,
        averaging_down_min_drop_pct: dec!(3.0),
        pump_chase_enabled: false,
        pump_chase_max_delta_pct: dec!(10.0),
        stop_loss_cooldown_enabled: false,
        stop_loss_cooldown_hours: 12,
        stop_loss_cooldown_loss_pct: dec!(5.0),
        pump_since_whale_guard_enabled: true,
        max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
        repeat_signal_gate_enabled: true,
        repeat_signal_min_prior: 1,
        momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
        momentum_bypass_enabled: false,
        wqs_proven_waiver_enabled: true,
    };
    let token_cache = Arc::new(TokenCache::new(1000, 300));
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            "http://127.0.0.1:1",
            None,
            "http://127.0.0.1:1".to_string(),
        )
        .with_price_cache(price_cache.clone()),
    );
    let token_parser = Arc::new(TokenParser::new(
        TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: dec("0"),
            min_liquidity_spear_usd: dec("0"),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        token_cache,
        token_fetcher.clone(),
    ));
    let selection_service = make_selection_service(db.clone(), token_parser.clone(), false);

    // ── ApiState ──────────────────────────────────────────────────────────
    helius_mock_base();
    let helius_client = Arc::new(
        HeliusClient::new(
            "test-api-key".to_string(),
            Arc::new(parking_lot::RwLock::new(HashMap::new())),
        )
        .expect("helius client"),
    );
    let webhook_rate_limiter = Arc::new(RateLimiter::new(40, 1));
    let api_state = Arc::new(ApiState {
        db: db.clone(),
        circuit_breaker: circuit_breaker.clone(),
        config: config_lock.clone(),
        notifier: notifier.clone(),
        engine: Some(Arc::new(engine_handle.clone())),
        metrics: metrics.clone(),
        signal_aggregator: None,
        market_regime_detector,
        helius_client: Some(helius_client.clone()),
        webhook_rate_limiter: Some(webhook_rate_limiter.clone()),
        price_cache: price_cache.clone(),
        toxic_detector: None,
        run_context: None,
        decision_recorder: None,
        profitability_verdict: Arc::new(tokio::sync::RwLock::new(None)),
    });

    // ── OperationsState ───────────────────────────────────────────────────
    let rpc_rate_limiter = Arc::new(RateLimiter::new(40, 1));
    let operations_state = Arc::new(OperationsState {
        db: db.clone(),
        engine: Some(Arc::new(engine_handle.clone())),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
        webhook_rate_limiter: Some(webhook_rate_limiter.clone()),
        rpc_rate_limiter: Some(rpc_rate_limiter.clone()),
    });

    // ── MonitoringState ───────────────────────────────────────────────────
    let monitoring_state = Arc::new(
        MonitoringState::new(db.clone(), engine_handle.clone(), config_arc.clone(), None)
            .expect("monitoring state")
            .with_circuit_breaker(circuit_breaker.clone())
            .with_token_parser(token_parser.clone())
            .with_selection(selection_service.clone()),
    );

    // ── Router (mirrors api/src/main.rs) ──────────────────────────────────
    let auth_state = Arc::new(AuthState::with_auth_config(
        HashMap::from([
            ("test-readonly".to_string(), Role::Readonly),
            ("test-operator".to_string(), Role::Operator),
            ("test-admin".to_string(), Role::Admin),
        ]),
        "test-secret".to_string(),
    ));

    let public_routes = Router::new()
        .route("/positions", get(list_positions))
        .route("/positions/:trade_uuid", get(get_position))
        .route("/trades", get(list_trades))
        .route("/trades/export", get(export_trades))
        .route("/metrics/strategy", get(get_strategy_performance))
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/costs", get(get_cost_metrics))
        .route("/metrics/trade-latency", get(get_trade_latency))
        .route(
            "/metrics/database-performance",
            get(get_database_performance),
        )
        .route("/metrics/request-rate", get(get_request_rate))
        .route("/metrics/rpc-latency", get(get_rpc_latency))
        .route("/risk/portfolio", get(get_portfolio_risk))
        .route("/portfolio/nav-history", get(get_nav_history))
        .route("/risk/stop-loss", get(get_stop_loss_metrics))
        .route("/risk/profit-target", get(get_profit_target_metrics))
        .route("/risk/position-size", get(get_position_size_analysis))
        .route("/incidents/dead-letter", get(list_dead_letter_queue))
        .route("/incidents/config-audit", get(list_config_audit))
        .route(
            "/signals/consensus",
            get(chimera_operator::handlers::get_consensus),
        )
        .route(
            "/signals/clustering",
            get(chimera_operator::handlers::get_wallet_clustering),
        )
        .route(
            "/signals/aggregation",
            get(chimera_operator::handlers::get_signal_aggregation),
        )
        .route("/signals/quality", get(get_signal_quality))
        .route("/signals/sources", get(get_signal_sources))
        .route("/market/regime", get(get_market_regime))
        .route("/market/conditions", get(get_market_conditions))
        .route("/scout/status", get(get_scout_status))
        .route("/scout/wqs-distribution", get(get_wqs_distribution))
        .route("/scout/metrics", get(get_scout_metrics))
        .route("/scout/budget", get(get_budget_status))
        .route("/scout/cache", get(get_cache_stats))
        .route("/scout/conviction", get(get_conviction_allocation))
        .route("/shadow/leaderboard", get(get_shadow_leaderboard))
        .with_state(api_state.clone());

    let protected_routes = Router::new()
        .route("/config", get(get_config))
        .route("/wallets", get(list_wallets))
        .route("/wallets/:address", get(get_wallet).put(update_wallet))
        .route("/config", put(update_config))
        .route("/config/circuit-breaker/reset", post(reset_circuit_breaker))
        .route("/config/circuit-breaker/trip", post(trip_circuit_breaker))
        .route(
            "/metrics/reconciliation",
            post(update_reconciliation_metrics),
        )
        .route(
            "/metrics/secret-rotation",
            post(update_secret_rotation_metrics),
        )
        .route("/reconciliation/status", get(get_reconciliation_status))
        .route("/reconciliation/history", get(get_reconciliation_history))
        .route("/reconciliation/stats", get(get_reconciliation_stats))
        .route("/reconciliation/trigger", post(trigger_reconciliation))
        .route(
            "/reconciliation/discrepancies/:id/resolve",
            post(resolve_discrepancy),
        )
        .route(
            "/incidents/dead-letter/:trade_uuid/retry",
            post(retry_dead_letter_item),
        )
        .route("/scout/run", post(trigger_scout_run))
        .route("/debug/backtest-smoke", post(debug_backtest_smoke))
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    let operations_routes = Router::new()
        .route("/operations/resources", get(get_resources))
        .route("/operations/secrets", get(get_secrets))
        .route("/operations/rate-limit", get(get_rate_limit_status))
        .route("/operations/health-checks", get(get_health_check_details))
        .with_state(operations_state.clone());

    // /monitoring/status needs the AuthExtension the handler extracts — in
    // production it is mounted without the auth layer (always 500); here the
    // layer is added so the handler body is exercised (roles >= Readonly mean
    // the unauthorized branch is dead either way). The Helius webhook stays
    // public: Helius delivers events WITHOUT a bearer token.
    let monitoring_status = Router::new()
        .route("/monitoring/status", get(get_monitoring_status))
        .with_state(monitoring_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));
    let monitoring_public = Router::new()
        .route("/monitoring/helius-webhook", post(helius_webhook_handler))
        .with_state(monitoring_state.clone());

    let monitoring_protected = Router::new()
        .route(
            "/monitoring/wallets/:wallet_address/enable",
            post(enable_wallet_monitoring),
        )
        .route(
            "/monitoring/wallets/:wallet_address/disable",
            post(disable_wallet_monitoring),
        )
        .route("/monitoring/webhooks/stats", get(get_webhook_stats))
        .route(
            "/monitoring/webhooks/bulk-register",
            post(bulk_register_webhooks),
        )
        .route(
            "/monitoring/webhooks/bulk-cleanup",
            post(bulk_cleanup_webhooks),
        )
        .route(
            "/monitoring/webhooks/reconcile",
            post(manual_reconcile_webhooks),
        )
        .route(
            "/monitoring/webhooks/health-check",
            post(manual_health_check),
        )
        .route("/monitoring/webhooks/audit", get(get_webhook_audit_log))
        .route(
            "/monitoring/webhooks/:wallet_address/retry",
            post(retry_webhook_registration),
        )
        .route(
            "/monitoring/webhooks/:wallet_address/toggle",
            post(toggle_wallet_webhook),
        )
        .route(
            "/monitoring/wallets/states",
            get(get_wallet_monitoring_states),
        )
        .with_state(monitoring_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    let app = Router::new()
        .nest("/api/v1", public_routes)
        .nest("/api/v1", protected_routes)
        .nest("/api/v1", operations_routes)
        .nest("/api/v1", monitoring_public)
        .nest("/api/v1", monitoring_status)
        .nest("/api/v1", monitoring_protected);

    let _engine_holder = engine; // engine task is not run in tests; handle only

    Harness {
        db,
        _guard: guard,
        pool,
        config: config_lock,
        config_arc,
        api_state,
        operations_state,
        monitoring_state,
        engine_handle,
        circuit_breaker,
        notifier,
        metrics,
        price_cache,
        webhook_rate_limiter,
        rpc_rate_limiter,
        helius_client,
        token_parser,
        selection_service,
        app,
    }
}

// =============================================================================
// REQUEST HELPERS
// =============================================================================

/// Standard auth headers for a given role (uses the harness API keys).
pub fn auth_headers(role: Role) -> HeaderMap {
    let key = match role {
        Role::Readonly => "test-readonly",
        Role::Operator => "test-operator",
        Role::Admin => "test-admin",
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {key}")).unwrap(),
    );
    headers
}

pub async fn api_get(app: &Router, uri: &str, headers: HeaderMap) -> Response {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(name.unwrap(), value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

pub async fn api_post(
    app: &Router,
    uri: &str,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        builder = builder.header(name.unwrap(), value);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

pub async fn api_put(
    app: &Router,
    uri: &str,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Response {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        builder = builder.header(name.unwrap(), value);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

pub async fn json_body(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// =============================================================================
// SEEDING HELPERS
// =============================================================================

pub const WALLET_A: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
pub const WALLET_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
pub const TOKEN_A: &str = "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R";
pub const TOKEN_B: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

pub async fn seed_wallet(pool: &Pool<Postgres>, address: &str, status: &str, wqs: Option<f64>) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, promoted_at, notes) \
         VALUES ($1, $2, $3, NOW(), 'Backtest: PASSED') \
         ON CONFLICT (address) DO UPDATE SET status = EXCLUDED.status, wqs_score = EXCLUDED.wqs_score",
    )
    .bind(address)
    .bind(status)
    .bind(wqs)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_trade(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    wallet: &str,
    token: &str,
    side: &str,
    status: &str,
    strategy: &str,
    amount_sol: &str,
    pnl_sol: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, pnl_sol, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW() - INTERVAL '1 hour')",
    )
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(strategy)
    .bind(side)
    .bind(dec(amount_sol))
    .bind(status)
    .bind(pnl_sol.map(dec))
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_position(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    wallet: &str,
    token: &str,
    strategy: &str,
    state: &str,
    entry_amount_sol: &str,
    entry_price: &str,
    closed_at: Option<chrono::DateTime<chrono::Utc>>,
) {
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, closed_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'sig', $7, $8)",
    )
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(strategy)
    .bind(dec(entry_amount_sol))
    .bind(dec(entry_price))
    .bind(state)
    .bind(closed_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a CLOSED position with realized PnL (feeds get_pnl_* which sums
/// positions.realized_pnl_sol where pnl_data_valid). Also seeds the parent
/// trade row (positions.trade_uuid has an FK to trades).
pub async fn seed_closed_position_with_pnl(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    wallet: &str,
    token: &str,
    strategy: &str,
    entry_amount_sol: &str,
    realized_pnl_sol: &str,
    closed_at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, pnl_sol, created_at) \
         VALUES ($1, $2, $3, $4, 'SELL', $5, 'CLOSED', $6, $7) \
         ON CONFLICT (trade_uuid) DO NOTHING",
    )
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(strategy)
    .bind(dec(entry_amount_sol))
    .bind(dec(realized_pnl_sol))
    .bind(closed_at)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at) \
         VALUES ($1, $2, $3, $4, $5, 1.0, 'sig', 'CLOSED', $6, $7)",
    )
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(strategy)
    .bind(dec(entry_amount_sol))
    .bind(dec(realized_pnl_sol))
    .bind(closed_at)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_exit_target(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    stop_loss_price: Option<&str>,
    targets_hit: Option<serde_json::Value>,
    trailing_stop_active: bool,
    peak_profit_percent: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO exit_targets (trade_uuid, entry_price, entry_amount_sol, stop_loss_price, targets_hit, trailing_stop_active, peak_profit_percent) \
         VALUES ($1, 1.0, 1.0, $2, $3, $4, $5)",
    )
    .bind(trade_uuid)
    .bind(stop_loss_price.map(dec))
    .bind(targets_hit)
    .bind(trailing_stop_active)
    .bind(peak_profit_percent.map(dec))
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_signal(
    pool: &Pool<Postgres>,
    token: &str,
    wallet: &str,
    direction: &str,
    amount_sol: &str,
    is_consensus: bool,
) {
    sqlx::query(
        "INSERT INTO signal_aggregation (token_address, wallet_address, direction, amount_sol, is_consensus, created_at) \
         VALUES ($1, $2, $3, $4, $5, NOW())",
    )
    .bind(token)
    .bind(wallet)
    .bind(direction)
    .bind(dec(amount_sol))
    .bind(is_consensus)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_reconciliation_run(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    expected: &str,
    discrepancy: Option<&str>,
    resolved: bool,
) {
    sqlx::query(
        "INSERT INTO reconciliation_log (trade_uuid, expected_state, discrepancy, resolved_at, resolved_by) \
         VALUES ($1, $2, $3, $4, 'test-user')",
    )
    .bind(trade_uuid)
    .bind(expected)
    .bind(discrepancy)
    .bind(if resolved { Some(chrono::Utc::now()) } else { None })
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_config_audit(
    pool: &Pool<Postgres>,
    key: &str,
    old_value: Option<&str>,
    new_value: &str,
    changed_by: &str,
) {
    sqlx::query(
        "INSERT INTO config_audit (key, old_value, new_value, changed_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(key)
    .bind(old_value)
    .bind(new_value)
    .bind(changed_by)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_dead_letter(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    can_retry: bool,
    retry_count: i32,
) {
    sqlx::query(
        "INSERT INTO dead_letter_queue (trade_uuid, payload, reason, can_retry, retry_count) \
         VALUES ($1, '{}', 'test-failure', $2, $3)",
    )
    .bind(trade_uuid)
    .bind(can_retry)
    .bind(retry_count)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed a dead-letter row with a real SignalPayload JSON (the format the
/// automated DLQ retry worker and the manual retry endpoint deserialize).
/// `amount_sol` is emitted as a string exactly like rust_decimal Decimal's
/// serde representation.
pub async fn seed_dead_letter_with_payload(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    wallet: &str,
    token: &str,
    side: &str,
    strategy: &str,
    amount_sol: &str,
    can_retry: bool,
    retry_count: i32,
) {
    let payload = serde_json::json!({
        "strategy": strategy,
        "token": token,
        "token_address": token,
        "action": side,
        "amount_sol": amount_sol,
        "wallet_address": wallet,
        "trade_uuid": trade_uuid,
    });
    sqlx::query(
        "INSERT INTO dead_letter_queue (trade_uuid, payload, reason, can_retry, retry_count) \
         VALUES ($1, $2, 'test-failure', $3, $4)",
    )
    .bind(trade_uuid)
    .bind(payload.to_string())
    .bind(can_retry)
    .bind(retry_count)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_wallet_monitoring(
    pool: &Pool<Postgres>,
    wallet: &str,
    webhook_id: Option<&str>,
    enabled: bool,
    health_status: Option<&str>,
    webhook_status: Option<&str>,
    registration_attempts: i32,
    last_registration_error: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO wallet_monitoring \
         (wallet_address, helius_webhook_id, monitoring_enabled, webhook_health_status, webhook_status, registration_attempts, last_registration_error, last_monitored_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())",
    )
    .bind(wallet)
    .bind(webhook_id)
    .bind(enabled)
    .bind(health_status)
    .bind(webhook_status)
    .bind(registration_attempts)
    .bind(last_registration_error)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_portfolio_snapshot(
    pool: &Pool<Postgres>,
    nav_sol: &str,
    recorded_at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        "INSERT INTO portfolio_snapshots (nav_sol, capital_sol, realized_pnl_sol, unrealized_pnl_sol, open_positions, recorded_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(dec(nav_sol))
    .bind(dec(nav_sol))
    .bind(dec("0"))
    .bind(dec("0"))
    .bind(0)
    .bind(recorded_at)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_shadow_position(
    pool: &Pool<Postgres>,
    shadow_id: &str,
    wallet: &str,
    token: &str,
    admitted: bool,
    opened_at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        "INSERT INTO shadow_positions (shadow_id, wallet_address, token_address, strategy, main_admitted, entry_amount_sol, ingress, opened_at) \
         VALUES ($1, $2, $3, 'mirror_main', $4, 0.1, 'webhook', $5)",
    )
    .bind(shadow_id)
    .bind(wallet)
    .bind(token)
    .bind(admitted)
    .bind(opened_at)
    .execute(pool)
    .await
    .unwrap();
}

pub async fn seed_shadow_exit(
    pool: &Pool<Postgres>,
    shadow_id: &str,
    strategy: &str,
    pnl_sol: &str,
    pnl_pct: &str,
) {
    sqlx::query(
        "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_sol, pnl_pct, exited_at) \
         VALUES ($1, $2, $3, $4, NOW())",
    )
    .bind(shadow_id)
    .bind(strategy)
    .bind(dec(pnl_sol))
    .bind(dec(pnl_pct))
    .execute(pool)
    .await
    .unwrap();
}

/// Build a SelectionService with the harness base config; when
/// `require_consensus_or_proven` is true the consensus-OR-proven BUY gate is
/// enforced (single unproven wallets get SINGLE_WALLET_UNPROVEN rejections).
pub fn make_selection_service(
    db: Arc<dyn Database>,
    token_parser: Arc<TokenParser>,
    require_consensus_or_proven: bool,
) -> Arc<chimera_operator::engine::SelectionService> {
    make_selection_service_with_parser(db, token_parser, require_consensus_or_proven)
}

/// Like [`make_selection_service`] but with an explicitly constructed parser
/// (e.g. one whose token-safety cache is pre-seeded so `fast_check` passes
/// without an RPC).
pub fn make_selection_service_with_parser(
    db: Arc<dyn Database>,
    token_parser: Arc<TokenParser>,
    require_consensus_or_proven: bool,
) -> Arc<chimera_operator::engine::SelectionService> {
    let config = test_config();
    let selection_config = SelectionConfig {
        total_capital_sol: config.position_sizing.total_capital_sol,
        max_position_sol: config.position_sizing.max_size_sol,
        shield_signal_quality_threshold: config.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config.strategy.spear_signal_quality_threshold,
        shield_percent: config.strategy.shield_percent,
        spear_percent: config.strategy.spear_percent,
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        min_liquidity_pumpfun_usd: config.token_safety.min_liquidity_pumpfun_usd,
        allow_graduated_pumpfun: config.token_safety.allow_graduated_pumpfun,
        min_token_age_hours: config.token_safety.min_token_age_hours,
        min_token_age_pumpfun_hours: config.token_safety.min_token_age_pumpfun_hours,
        min_token_age_proven_hours: 0.0,
        min_wqs_score: 70.0,
        spear_lite_max_size_sol: dec("0.10"),
        spear_lite_wqs_threshold: 40.0,
        require_consensus_or_proven,
        min_proven_trades: 10,
        require_proven_positive_pnl: false,
        mirror_gate_enabled: false,
        mirror_gate_min_avg_pct: dec("1.5"),
        mirror_gate_min_samples: 10,
        mirror_gate_window_hours: 48,
        wallet_tstat_enabled: false,
        wallet_tstat_threshold: 1.645,
        wallet_tstat_min_samples: 10,
        wallet_tstat_window_days: 30,
        shadow_proven_enabled: false,
        shadow_proven_min_samples: 20,
        shadow_proven_min_total_pnl_sol: 2.0,
        token_velocity_gate_enabled: false,
        token_min_liquidity_velocity: 0.10,
        token_max_curve_completion: 0.85,
        cluster_gate_enabled: false,
        cluster_min_profitable_wallets: 3,
        averaging_down_enabled: false,
        averaging_down_window_hours: 12,
        averaging_down_min_buys: 2,
        averaging_down_min_drop_pct: dec("3.0"),
        pump_chase_enabled: false,
        pump_chase_max_delta_pct: dec("10.0"),
        stop_loss_cooldown_enabled: false,
        stop_loss_cooldown_hours: 12,
        stop_loss_cooldown_loss_pct: dec("5.0"),
        pump_since_whale_guard_enabled: true,
        max_pump_since_whale_pct: rust_decimal::Decimal::new(15, 0),
        repeat_signal_gate_enabled: true,
        repeat_signal_min_prior: 1,
        momentum_bypass_min_pct: rust_decimal::Decimal::new(3, 0),
        momentum_bypass_enabled: false,
        wqs_proven_waiver_enabled: true,
    };
    Arc::new(chimera_operator::engine::SelectionService::new(
        db,
        token_parser,
        None, // portfolio_heat
        None, // signal_aggregator
        None, // market_regime
        None, // helius_client
        None, // position_sizer
        selection_config,
    ))
}

/// A TokenParser whose safety cache is pre-seeded with a safe result for
/// `{token}:{strategy}`, so `fast_check` passes without any RPC access.
pub fn make_token_parser_with_seeded_safety(token: &str, strategy: &str) -> Arc<TokenParser> {
    let cache = Arc::new(TokenCache::new(1000, 300));
    cache.insert(
        format!("{token}:{strategy}"),
        chimera_operator::token::TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: true,
            liquidity_usd: Some(dec("100000")),
        },
    );
    let fetcher = Arc::new(TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
        "http://127.0.0.1:1",
        None,
        "http://127.0.0.1:1".to_string(),
    ));
    Arc::new(TokenParser::new(
        TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: dec("0"),
            min_liquidity_spear_usd: dec("0"),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        cache,
        fetcher,
    ))
}

/// Insert a trade through the trait (covers db-inserted fields used by the
/// reconciliation metrics endpoints).
pub async fn insert_trade_via_trait(db: &Arc<dyn Database>, trade: &InsertTrade) -> i64 {
    db.insert_trade(trade).await.unwrap()
}

/// Insert a position through the trait.
pub async fn insert_position_via_trait(db: &Arc<dyn Database>, position: &InsertPosition) -> i64 {
    db.insert_position(position).await.unwrap()
}
