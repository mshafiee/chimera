//! Chimera Operator - High-frequency copy-trading system for Solana
//!
//! This is the main entry point for the Operator service.
//! It sets up the Axum web server with middleware and routes.

#![allow(warnings)]

use axum::{
    middleware::{self as axum_middleware},
    routing::{get, post, put},
    Router,
};
use chrono::Utc;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use tokio_util::sync::CancellationToken;

mod tools;

use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::config::{AppConfig, TradeMode};
use chimera_operator::db_abstraction;
use chimera_operator::db_abstraction::ActivePositionEntry;
use chimera_operator::engine::{
    self, MarketRegimeDetector, MomentumExit, PortfolioHeat, PositionSizer, ProfitTargetAction,
    ProfitTargetManager, RecoveryManager, StopLossAction, StopLossManager, TipManager, VolumeCache,
};
use chimera_operator::handlers::{
    bulk_cleanup_webhooks,
    bulk_register_webhooks,
    debug_backtest_smoke,
    disable_wallet_monitoring,
    enable_wallet_monitoring,
    export_trades,
    get_config,
    get_cost_metrics,
    get_health_check_details,
    get_market_conditions,
    get_market_regime,
    get_monitoring_status,
    get_performance_metrics,
    get_position,
    get_rate_limit_status,
    get_resources,
    get_scout_metrics,
    get_scout_status,
    get_budget_status,
    get_cache_stats,
    get_conviction_allocation,
    get_secrets,
    get_strategy_performance,
    get_wallet,
    get_wallet_monitoring_states,
    get_webhook_audit_log,
    get_webhook_stats,
    get_wqs_distribution,
    health_check,
    health_simple,
    helius_webhook_handler,
    list_config_audit,
    list_dead_letter_queue,
    list_positions,
    list_trades,
    list_wallets,
    manual_health_check,
    manual_reconcile_webhooks,
    reset_circuit_breaker,
    retry_webhook_registration,
    retry_dead_letter_item,
    toggle_wallet_webhook,
    trigger_scout_run,
    trip_circuit_breaker,
    update_config,
    update_reconciliation_metrics,
    update_secret_rotation_metrics,
    update_wallet,
    wallet_auth,
    refresh_token,
    webhook_handler,
    ws_handler,
    profitability_verdict,
    ApiState,
    AppState,
    OperationsState,
    WalletAuthState,
    WebhookState,
    WsState,
};
use chimera_operator::metrics::{metrics_router, MetricsState};
use chimera_operator::middleware::{self, bearer_auth, AuthState, Role};
use chimera_operator::monitoring::{rate_limiter, HeliusClient, MonitoringState, SignalAggregator};
use chimera_operator::notifications::{self, NotificationEvent};
use chimera_operator::price_cache::PriceCache;
use chimera_operator::handlers::{fetch_outcomes, count_missing_outcomes, count_invalid_pnl, CachedVerdict};
use chimera_operator::token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyConfig};
use chimera_operator::vault;
use chimera_operator::{Action, Signal, SignalPayload, Strategy};
use rust_decimal::prelude::ToPrimitive;

async fn run_preflight(config: &AppConfig) -> anyhow::Result<()> {
    match config.trade_mode {
        chimera_operator::config::TradeMode::Paper => {
            let jupiter_url = format!(
                "{}/quote?inputMint=So11111111111111111111111111111111111111112&outputMint=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v&amount=100000000&slippageBps=50",
                config.jupiter.api_url
            );
            let client = reqwest::Client::new();
            let resp = client
                .get(&jupiter_url)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Pre-flight Jupiter probe failed: {}", e))?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "Pre-flight Jupiter probe returned HTTP {} — paper mode requires Jupiter API to be reachable",
                    resp.status()
                );
            }
            tracing::info!("Pre-flight passed: Jupiter API reachable (paper mode)");
        }
        chimera_operator::config::TradeMode::Devnet | chimera_operator::config::TradeMode::Live => {
            let secrets = chimera_operator::vault::load_secrets_with_fallback()
                .map_err(|e| anyhow::anyhow!("Pre-flight vault load failed: {}", e))?;
            let _keypair =
                chimera_operator::engine::transaction_builder::load_wallet_keypair(&secrets)
                    .map_err(|e| anyhow::anyhow!("Pre-flight keypair load failed: {}", e))?;
            let rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new(
                config.rpc.primary_url.clone(),
            );
            chimera_operator::metrics::timed_rpc(
                "primary",
                "getLatestBlockhash",
                rpc_client.get_latest_blockhash(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Pre-flight RPC probe failed: {}", e))?;
            tracing::info!(
                "Pre-flight passed: vault, keypair, and RPC reachable ({})",
                config.trade_mode
            );
        }
    }
    Ok(())
}

/// Validates JWT secret cryptographic strength
/// Returns error if secret doesn't meet minimum entropy requirements
pub(crate) fn validate_jwt_secret(secret: &str) -> Result<(), anyhow::Error> {
    // Minimum length: 64 characters for hex encoding
    if secret.len() < 64 {
        return Err(anyhow::anyhow!("JWT secret too short (minimum 64 characters)"));
    }

    // Check for hex format (0-9, a-f, A-F)
    if !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!(
            "JWT secret must be hexadecimal (0-9, a-f, A-F)"
        ));
    }

    // Calculate entropy: 4 bits per hex character
    let entropy_bits = secret.len() * 4;
    if entropy_bits < 256 {
        return Err(anyhow::anyhow!("JWT secret entropy too low (minimum 256 bits)"));
    }

    // Check for common dictionary words and patterns
    let common_patterns = ["0000000000000000000000000000000000000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "1234567890123456789012345678901234567890123456789012345678901234",
        "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd"];

    if common_patterns.contains(&secret.to_lowercase().as_str()) {
        return Err(anyhow::anyhow!(
            "JWT secret matches a common weak pattern"
        ));
    }

    // Check for repeated character patterns (e.g., "aaaaa...")
    if secret.chars().all(|c| c == secret.chars().next().unwrap()) {
        return Err(anyhow::anyhow!(
            "JWT secret contains repeated characters only"
        ));
    }
    Ok(())
}

/// Detect and force-close orphaned ACTIVE positions whose `token_amount` is
/// NULL and that are older than `min_age_secs`.
///
/// Such positions can never be sold (paper SELL requires `token_amount`) and
/// permanently block a `max_concurrent_positions` slot while spamming
/// unsellable EXIT signals every few seconds. The age guard gives a fresh
/// BUY's token_amount persistence path time to settle before intervening.
///
/// Idempotent: the underlying UPDATE only matches rows that are still ACTIVE
/// with a NULL token_amount. Safe to call at startup and periodically.
async fn cleanup_orphaned_positions(db: &Arc<dyn db_abstraction::Database>, min_age_secs: i64) {
    let now = Utc::now();
    let positions = match db.get_active_positions().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "orphan-position sweep: get_active_positions failed");
            return;
        }
    };
    for pos in positions {
        if pos.token_amount.is_some() {
            continue;
        }
        let age = now.signed_duration_since(pos.opened_at);
        if age.num_seconds() < min_age_secs {
            continue;
        }
        tracing::warn!(
            trade_uuid = %pos.trade_uuid,
            token = %pos.token_address,
            wallet = %pos.wallet_address,
            opened_at = %pos.opened_at,
            age_secs = age.num_seconds(),
            "orphan-position sweep: force-closing ACTIVE position with NULL token_amount"
        );
        if let Err(e) = db
            .force_close_orphan_position(&pos.trade_uuid, "orphan_null_token_amount_sweep")
            .await
        {
            tracing::error!(
                error = %e,
                trade_uuid = %pos.trade_uuid,
                "orphan-position sweep: force-close failed"
            );
        }
    }
}

/// Refill the ACTIVE wallet roster from the scout-discovered CANDIDATE pool.
///
/// The wallet lifecycle is otherwise one-sided — `auto_demote_wallets` and
/// inactivity rotation drain ACTIVE → CANDIDATE/REJECTED, but nothing promotes
/// back. This left the monitored roster stuck at a handful of manually-promoted
/// wallets, capping copy-trading throughput regardless of how many candidates
/// scout discovered. This task counterbalances demotion: when the ACTIVE count
/// is below `max_active`, it promotes the highest-WQS CANDIDATEs up to the gap.
///
/// Promoted wallets are immediately covered by the tiered RPC polling task
/// (which polls all ACTIVE wallets), so they begin generating live signals
/// without requiring a separate webhook-registration step. Auto-demote then
/// prunes any that underperform or go dormant — the lifecycle becomes
/// self-correcting instead of only draining.
///
/// Idempotent and safe to call repeatedly: only CANDIDATE → ACTIVE transitions
/// occur, capped at `max_active`, and a no-op when the roster is full or no
/// eligible candidates exist.
async fn auto_promote_wallets(
    db: &Arc<dyn db_abstraction::Database>,
    max_active: usize,
    min_wqs: f64,
    ttl_hours: i64,
    max_age_days: i64,
) {
    // First, demote ACTIVE wallets that haven't traded on-chain within
    // max_age_days. These generate no copy signals and occupy roster slots the
    // inactivity rotation won't reclaim during its promotion grace period.
    // Demoting them frees slots for active candidates below.
    match db.demote_dormant_active_wallets(max_age_days).await {
        Ok(n) if n > 0 => tracing::info!(
            demoted = n,
            max_age_days = max_age_days,
            "auto_promote: demoted dormant ACTIVE wallets (no trade within window) to CANDIDATE"
        ),
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "auto_promote: demote_dormant_active_wallets failed"),
    }

    let active_count = match db.get_active_wallets().await {
        Ok(w) => w.len(),
        Err(e) => {
            tracing::error!(error = %e, "auto_promote: get_active_wallets failed");
            return;
        }
    };
    if active_count >= max_active {
        return; // roster full — nothing to do
    }
    let gap = (max_active - active_count) as i64;
    let candidates = match db.get_promotion_candidates(min_wqs, max_age_days, gap).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "auto_promote: get_promotion_candidates failed");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }
    let mut promoted = 0usize;
    for w in &candidates {
        let wqs_f = w.wqs_score.and_then(|d| d.to_f64()).unwrap_or(0.0);
        let reason = format!(
            "auto_promote: recency-first refill (wqs={:.1}, last_trade={})",
            wqs_f,
            w.last_trade_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown".to_string())
        );
        match db
            .update_wallet_status_ext(&w.address, "ACTIVE", Some(ttl_hours as i32), Some(&reason))
            .await
        {
            Ok(true) => {
                promoted += 1;
                tracing::info!(
                    wallet = %w.address,
                    wqs = wqs_f,
                    last_trade_at = ?w.last_trade_at,
                    ttl_hours = ttl_hours,
                    "auto_promote: promoted CANDIDATE -> ACTIVE"
                );
            }
            Ok(false) => tracing::warn!(
                wallet = %w.address,
                "auto_promote: promotion no-op (status changed concurrently)"
            ),
            Err(e) => tracing::error!(
                error = %e,
                wallet = %w.address,
                "auto_promote: promotion failed"
            ),
        }
    }
    if promoted > 0 {
        tracing::info!(
            promoted = promoted,
            active_before = active_count,
            max_active = max_active,
            min_wqs = min_wqs,
            max_age_days = max_age_days,
            "auto_promote: refilled ACTIVE roster from recent CANDIDATE pool"
        );
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    init_tracing();

    // Load configuration
    let mut config = load_config()?;

    use chimera_operator::config::{resolve_trade_mode, TradeMode};

    let explicit_mode = {
        let mut mode: Option<TradeMode> = None;
        if let Ok(mode_str) = std::env::var("CHIMERA_TRADE_MODE") {
            mode = match mode_str.to_lowercase().as_str() {
                "devnet" => Some(TradeMode::Devnet),
                "paper" => Some(TradeMode::Paper),
                "live" => Some(TradeMode::Live),
                _ => {
                    tracing::warn!(provided = %mode_str, "Invalid CHIMERA_TRADE_MODE — must be devnet|paper|live. Ignoring.");
                    None
                }
            };
        }
        if let Ok(old_val) = std::env::var("CHIMERA_JUPITER__DEVNET_SIMULATION_MODE") {
            if (old_val == "true" || old_val == "1") && mode.is_none() {
                tracing::warn!("CHIMERA_JUPITER__DEVNET_SIMULATION_MODE is deprecated — use CHIMERA_TRADE_MODE=paper");
                mode = Some(TradeMode::Paper);
            }
        }
        mode
    };
    config.trade_mode =
        resolve_trade_mode(explicit_mode, config.trade_mode, &config.rpc.primary_url);

    // Install the Jupiter API key into the process-global credential store.
    // Attached as `x-api-key` on every Jupiter request (quote/swap/price).
    // F1: keyless access is being phased out; Live mode hard-fails without it
    // (enforced in AppConfig::validate).
    //
    // The config crate does not reliably map CHIMERA_JUPITER__API_KEY ->
    // config.jupiter.api_key (casing), and config.yaml intentionally omits the
    // key (secret not committed). Fall back to the env var when config has none,
    // mirroring the Helius key resolution below.
    let jupiter_api_key = config
        .jupiter
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty())
        .or_else(|| std::env::var("CHIMERA_JUPITER__API_KEY").ok().filter(|s| !s.trim().is_empty()))
        .or_else(|| std::env::var("JUPITER_API_KEY").ok().filter(|s| !s.trim().is_empty()));
    chimera_operator::jupiter::set_api_key(jupiter_api_key.clone());
    if jupiter_api_key.is_some() {
        tracing::info!("Jupiter API key installed (x-api-key will be sent on all Jupiter requests)");
    } else {
        tracing::warn!(
            "No Jupiter API key configured (CHIMERA_JUPITER__API_KEY) — requests will be rate-limited; \
             Live trade mode will refuse to start"
        );
    }

    match config.trade_mode {
        TradeMode::Paper => tracing::warn!("┌─────────────────────────────────────────┐"),
        _ => tracing::info!("┌─────────────────────────────────────────┐"),
    };
    tracing::info!("│  TRADE MODE: {:<28}  │", config.trade_mode.to_string());
    match config.trade_mode {
        TradeMode::Paper => tracing::warn!("│  NO REAL TRANSACTIONS WILL BE SUBMITTED │"),
        TradeMode::Devnet => tracing::info!("│  Transactions on DEVNET (test network)  │"),
        TradeMode::Live => tracing::info!("│  LIVE TRADING — REAL SOL AT RISK        │"),
    }
    tracing::info!("└─────────────────────────────────────────┘");

    match config.trade_mode {
        TradeMode::Paper => {
            tracing::info!("Paper mode: skipping vault validation (no keypair needed)");
        }
        TradeMode::Devnet | TradeMode::Live => {
            let _startup_secrets = vault::load_secrets_with_fallback()
                .map_err(|e| anyhow::anyhow!("Vault startup validation failed: {}", e))?;
            tracing::info!("Vault/secrets validated at startup");
        }
    }

    // Load API keys and JWT secret early for WebSocket state initialization
    let mut api_keys_map = std::collections::HashMap::new();
    for key_config in &config.security.api_keys {
        // Char-safe prefix: slicing a String by raw byte index can land in the
        // middle of a multi-byte UTF-8 char and panic on untrusted input.
        let key_prefix: String = key_config.key.chars().take(8).collect();
        if let Ok(role) = key_config.role.parse::<Role>() {
            api_keys_map.insert(key_config.key.clone(), role);
            tracing::debug!(key_prefix = %key_prefix, role = %role, "API key configured");
        } else {
            tracing::warn!(key_prefix = %key_prefix, role = %key_config.role, "Invalid role in API key config");
        }
    }

    let chimera_env = std::env::var("CHIMERA_ENV").unwrap_or_default();
    let jwt_secret = match std::env::var("JWT_SECRET") {
        Ok(secret) => {
            if chimera_env == "production" {
                crate::validate_jwt_secret(&secret)?;
                tracing::info!("JWT secret validated successfully");
            }
            secret
        }
        Err(_) if chimera_env == "production" => {
            tracing::error!("JWT_SECRET environment variable must be set in production mode");
            return Err(anyhow::anyhow!(
                "JWT_SECRET environment variable is required in production mode but was not set"
            ));
        }
        Err(_) => {
            tracing::warn!(
                "JWT_SECRET not set — using development default (insecure, only for local testing)"
            );
            use std::fmt::Write;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut dev_secret = String::from("dev-");
            write!(&mut dev_secret, "{}", timestamp).unwrap();
            dev_secret
        }
    };

    // Initialize database
    let db_config = db_abstraction::DatabaseConfig {
        backend: db_abstraction::DatabaseBackend::from_env(),
        url: config.database.url.clone(),
        max_connections: config.database.max_connections,
        acquire_timeout_seconds: 30,
    };
    let db_pool = db_abstraction::create_database(&db_config).await?;
    db_pool.run_migrations().await?;
    db_pool.startup_integrity_check().await?;
    db_pool.recover_executing_trades().await?;
    // Reclaim slots held by orphaned ACTIVE positions (NULL token_amount) that
    // block max_concurrent_positions and spam unsellable EXIT signals. Only
    // closes positions older than 10 minutes so a fresh BUY's token_amount
    // persistence path is not raced.
    cleanup_orphaned_positions(&db_pool, 600).await;
    // Auto-promote: refill the ACTIVE wallet roster from high-WQS CANDIDATEs so
    // the monitored pool self-replenishes (counterbalances auto-demote, which
    // otherwise leaves the roster stuck at a few manually-promoted wallets and
    // caps copy-trading throughput). Extract the (Copy) settings once while
    // config is owned; the periodic task reuses the same values.
    let auto_promote_cfg = config
        .monitoring
        .as_ref()
        .map(|m| {
            (
                m.auto_promote_enabled,
                m.auto_promote_min_wqs,
                m.auto_promote_ttl_hours,
                m.max_active_wallets,
                m.auto_promote_max_age_days,
            )
        })
        .unwrap_or((false, 60.0, 168, 20, 7));
    if auto_promote_cfg.0 {
        auto_promote_wallets(
            &db_pool,
            auto_promote_cfg.3,
            auto_promote_cfg.1,
            auto_promote_cfg.2,
            auto_promote_cfg.4,
        )
        .await;
    }
    tracing::info!("Database initialized");

    // C1: Run-scoped evidence. Build the admission threshold config once, then
    // derive the RunContext (unique per process run) and the DecisionRecorder
    // (fire-and-forget decision persistence). Constructed early so both the
    // /health endpoint and the selection engine share the same run identity.
    let selection_config = crate::engine::SelectionConfig {
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
        min_wqs_score: std::env::var("CHIMERA_SELECTION__MIN_WQS_SCORE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(70.0),
        spear_lite_max_size_sol: std::env::var("CHIMERA_SELECTION__SPEAR_LITE_MAX_SIZE_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(rust_decimal::Decimal::new(10, 2)), // 0.10 SOL
        spear_lite_wqs_threshold: std::env::var("CHIMERA_SELECTION__SPEAR_LITE_WQS_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40.0),
    };
    let roster_addresses: Vec<String> = db_pool
        .get_active_wallets()
        .await
        .map(|ws| ws.iter().map(|w| w.address.clone()).collect())
        .unwrap_or_default();
    let run_context = Arc::new(chimera_operator::engine::RunContext::new(
        selection_config.hash(),
        &roster_addresses,
        Utc::now(),
    ));
    let decision_recorder = Arc::new(chimera_operator::engine::DecisionRecorder::new(
        db_pool.clone(),
        run_context.clone(),
    ));
    tracing::info!(
        run_id = %run_context.run_id,
        code_revision = %run_context.code_revision,
        config_hash = %run_context.config_hash,
        roster_hash = %run_context.roster_hash,
        roster_size = roster_addresses.len(),
        "Run context initialized (C1 evidence)"
    );

    run_preflight(&config).await?;

    let cancel_token = CancellationToken::new();

    // Initialize WebSocket state with authentication (early initialization for circuit breaker)
    let ws_state = Arc::new(WsState::new(
        api_keys_map.clone(),
        jwt_secret.clone(),
        true, // Allow anonymous readonly for development dashboard
    ));

    // Initialize price cache with Jupiter configuration
    let price_cache = match PriceCache::with_jupiter_price_api(config.jupiter.price_api_url.clone()) {
        Ok(cache) => Arc::new(cache),
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize price cache — HTTP client build failed");
            return Err(anyhow::anyhow!("Price cache initialization failed: {}", e));
        }
    };
    // Track SOL for volatility calculation
    price_cache.track_token("So11111111111111111111111111111111111111112");

    // Prime the price cache eagerly so the first circuit-breaker evaluation
    // has a SOL price for USD-denominated loss checks. Fixes startup race
    // where CB fired before the background updater completed its first fetch.
    match price_cache.prime_prices().await {
        Ok(_) => tracing::info!("Price cache primed at startup"),
        Err(e) => tracing::warn!(
            error = %e,
            "Failed to prime price cache at startup — CB USD checks deferred until background updater fetches"
        ),
    }

    // Shadow paper trader: trades every signal for later evaluation
    let shadow_trader = Arc::new(chimera_operator::engine::ShadowTrader::new(
        db_pool.clone(),
        price_cache.clone(),
        chimera_operator::engine::ShadowConfig::from_env(
            Arc::new(config.profit_management.clone()),
            run_context.run_id.clone(),
        ),
    ));
    if shadow_trader.is_enabled() {
        tracing::info!(run_id = %run_context.run_id, "Shadow paper trader enabled");
    } else {
        tracing::info!("Shadow paper trader disabled");
    }

    // Shared VolumeCache — fed by DexScreener client (B3), consumed by
    // MomentumExit for volume-drop detection and SelectionService.
    let shared_volume_cache = Arc::new(engine::volume_cache::VolumeCache::new());
    tracing::info!("✓ Volume Cache initialized (shared) for liquidity monitoring");

    // DexScreener client (B3) — feeds the shared VolumeCache with 24h volume samples.
    let dexscreener_rate_limiter = Arc::new(
        chimera_operator::monitoring::rate_limiter::RateLimiter::new(5, 1),
    );
    let dexscreener_client = Arc::new(
        chimera_operator::monitoring::dexscreener::DexScreenerClient::new(
            dexscreener_rate_limiter,
            shared_volume_cache.clone(),
        ),
    );
    tracing::info!("✓ DexScreener client initialized (shared volume cache)");

    // Validate webhook URL reachability if monitoring is enabled
    if let Some(ref monitoring_config) = config.monitoring {
        if monitoring_config.enabled {
            if let Some(ref webhook_url) = monitoring_config.helius_webhook_url {
                if !webhook_url.is_empty() {
                    match chimera_operator::monitoring::helius::validate_webhook_reachability(
                        webhook_url,
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::info!("Webhook URL validated successfully: {}", webhook_url);
                        }
                        Err(e) => {
                            // This self-check GETs the webhook URL from INSIDE the
                            // container. For a self-hosted URL (the server's own public
                            // address) it requires NAT loopback, which most hosts lack —
                            // so a connection failure here is usually a false negative,
                            // not a real reachability problem. External delivery (what
                            // actually matters) is verified independently by Helius. Log
                            // at debug to avoid a misleading 'monitoring may not work'
                            // alarm; a genuinely malformed URL still surfaces via failed
                            // webhook registrations/deliveries.
                            tracing::debug!(
                                webhook_url = %webhook_url,
                                error = %e,
                                "Webhook URL self-reachability check inconclusive \
                                 (common for self-hosted URLs without NAT loopback); \
                                 external delivery is verified independently"
                            );
                            // Don't fail startup on webhook validation issues - the webhook
                            // may become available later, or monitoring may be optional
                        }
                    }
                }
            }
        }
    }

    // Initialize notification service
    let notifier = {
        let mut composite = notifications::CompositeNotifier::new();

        // Add Discord notifier if configured via environment variable
        if let Some(discord) = notifications::DiscordNotifier::from_env() {
            composite.add_service(Arc::new(discord));
            tracing::info!("Discord notifications enabled");
        }

        // Add Telegram notifier if configured
        if config.notifications.telegram.enabled {
            let telegram_config = notifications::telegram::TelegramConfig {
                bot_token: std::env::var("TELEGRAM_BOT_TOKEN")
                    .unwrap_or_else(|_| config.notifications.telegram.bot_token.clone()),
                chat_id: std::env::var("TELEGRAM_CHAT_ID")
                    .unwrap_or_else(|_| config.notifications.telegram.chat_id.clone()),
                enabled: true,
                rate_limit_seconds: config.notifications.telegram.rate_limit_seconds,
            };

            if !telegram_config.bot_token.is_empty() && !telegram_config.chat_id.is_empty() {
                composite.add_service(Arc::new(notifications::TelegramNotifier::new(
                    telegram_config,
                )));
                tracing::info!("Telegram notifications enabled");
            } else {
                tracing::warn!(
                    "Telegram notifications enabled in config but bot_token/chat_id not set"
                );
            }
        }

        composite.set_trade_mode(&config.trade_mode.to_string());

        Arc::new(composite)
    };
    tracing::info!("Notification service initialized");

    // Initialize circuit breaker
    let circuit_breaker = Arc::new(
        CircuitBreaker::new_with_ws(
            config.circuit_breakers.clone(),
            db_pool.clone(),
            Some(ws_state.clone()),
            config.position_sizing.total_capital_sol,
        )
        .with_price_cache(price_cache.clone()),
    );

    // Wire notification service into circuit breaker so manual/auto trips send push alerts
    circuit_breaker.set_notifier(notifier.clone());

    // FIX [R-C1]: Restore persisted circuit breaker state from DB before accepting connections.
    // This ensures that a trip persisted before last restart is re-applied and evaluate()
    // runs so cooldown expiry / breach re-evaluation happen immediately on startup.
    if let Err(e) = circuit_breaker.restore_from_db().await {
        tracing::error!(error = %e, "Failed to restore circuit breaker state from DB — starting Active");
    }

    // Restore kill-switch if it was active before last restart.
    // Reads from kill_switch_state (single-row UPSERT table) which is written synchronously
    // by the kill-switch API handler before tripping the circuit breaker in memory.
    {
        let is_active = match db_pool.get_kill_switch_state().await {
            Ok(state) => {
                tracing::info!("Kill-switch state loaded: {}", state.state);
                state.state == "ACTIVE"
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "FAIL-SAFE: Failed to read kill-switch state — assuming ACTIVE to prevent unintended trading"
                );
                true
            }
        };

        if is_active {
            tracing::warn!("Kill-switch was active before restart — re-tripping circuit breaker");
            if let Err(e) = circuit_breaker
                .manual_trip(
                    "SYSTEM_RESTART_RESTORE",
                    "Kill-switch was active before restart".to_string(),
                )
                .await
            {
                tracing::error!(error = %e, "CRITICAL: Failed to restore kill-switch — ABORTING STARTUP");
                return Err(anyhow::anyhow!("Failed to restore kill-switch: {}", e));
            }
        }
    }

    // Initialize tip manager
    let tip_manager = Arc::new(TipManager::new(config.jito.clone(), db_pool.clone()));
    if let Err(e) = tip_manager.init().await {
        tracing::error!(error = %e, "Failed to initialize tip manager — operating in cold-start mode");
    }

    // In paper mode, seed the tip history with realistic mainnet values so the
    // TipManager escapes cold-start. Paper trades never record tips, so without
    // a seed the SELL/Exit tip stays at the config ceiling forever (~0.003 SOL
    // = 3% of a 0.1 SOL position) — a structural cost drag that makes paper
    // trading look unprofitable even when live percentile-based tips would be
    // realistic. Seeding is a no-op once >= MIN_SAMPLES_FOR_PERCENTILE rows
    // exist, and live mode never seeds.
    if config.trade_mode == TradeMode::Paper {
        if let Err(e) = tip_manager.seed_paper_history_if_empty().await {
            tracing::warn!(error = %e, "Failed to seed paper tip history — continuing in cold-start mode");
        }
    }

    // Initialize token parser (needed for slow-path safety checks in engine)
    // Create RPC rate limiter for token metadata fetching (simulation calls are weighted)
    let rpc_rate_limiter = Arc::new(rate_limiter::RateLimiter::new(
        config.rpc.rate_limit_per_second,
        1,
    ));
    let token_cache = Arc::new(TokenCache::new(
        config.token_safety.cache_capacity,
        config.token_safety.cache_ttl_seconds,
    ));
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            &config.rpc.primary_url,
            Some(rpc_rate_limiter.clone()),
            config.jupiter.api_url.clone(),
        )
        .with_price_cache(price_cache.clone())
        .with_unlisted_heuristic(config.token_safety.allow_unlisted_heuristic)
        .with_liquidity_ttl(config.token_safety.liquidity_cache_ttl_secs)
        .with_fdv_ttl(config.token_safety.fdv_cache_ttl_secs),
    );

    // Create HeliusClient early (needed by token_fetcher_with_helius later)
    // Resolve API key: prefer raw env var (config crate doesn't interpolate ${VAR} in YAML)
    let helius_api_key_resolved = {
        let from_config = config
            .monitoring
            .as_ref()
            .and_then(|m| m.helius_api_key.clone())
            .unwrap_or_default();
        if from_config.starts_with("${") {
            std::env::var("HELIUS_API_KEY").unwrap_or_default()
        } else {
            from_config
        }
    };
    let helius_client: Option<Arc<HeliusClient>> = HeliusClient::new(
        helius_api_key_resolved,
        token_fetcher.get_metadata_cache(),
    )
    .map(Arc::new)
    .map_err(|e| tracing::warn!(error = %e, "HeliusClient unavailable, signal quality limited"))
    .ok();

    let token_safety_config = TokenSafetyConfig {
        freeze_authority_whitelist: config
            .token_safety
            .freeze_authority_whitelist
            .iter()
            .cloned()
            .collect(),
        mint_authority_whitelist: config
            .token_safety
            .mint_authority_whitelist
            .iter()
            .cloned()
            .collect(),
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        honeypot_detection_enabled: config.token_safety.honeypot_detection_enabled,
        holder_concentration_check_enabled: config.token_safety.holder_concentration_check_enabled,
        max_holder_concentration_pct: config.token_safety.max_holder_concentration_pct,
    };
    let token_parser = Arc::new(TokenParser::new(
        token_safety_config,
        token_cache.clone(),
        token_fetcher.clone(),
    ));
    tracing::info!("Token parser initialized");

    let state_registry = Arc::new(chimera_operator::state::StateRegistry::new());
    tracing::info!("State registry initialized");

    let portfolio_heat = Arc::new(
        PortfolioHeat::new(
            db_pool.clone(),
            config.position_sizing.total_capital_sol,
        )
        .with_registry(Arc::clone(&state_registry))
    );
    tracing::info!(
        total_capital_sol = ?config.position_sizing.total_capital_sol,
        "Portfolio heat manager initialized with registry fast path"
    );

    // B3: Wallet performance tracker + toxic flow detector
    let wallet_performance_tracker = Arc::new(
        chimera_operator::monitoring::WalletPerformanceTracker::new_with_config(db_pool.clone(), Arc::new(config.clone())),
    );
    let toxic_flow_detector = Arc::new(
        chimera_operator::experiment::ToxicFlowDetector::new(config.experiment.clone()),
    );
    let rejection_mute_detector = Arc::new(
        crate::engine::rejection_mute::RejectionMuteDetector::new(config.rejection_mute.clone()),
    );
    let dune_pnl_monitor = crate::engine::dune_monitor::DunePnlMonitor::new(
        &config.dune,
        db_pool.clone(),
    );
    tracing::info!(
        toxic_threshold = config.experiment.toxic_threshold_percent,
        dune_enabled = config.dune.enabled,
        "Toxic flow detector + wallet performance tracker + Dune monitor initialized"
    );

    let verdict_cache = Arc::new(tokio::sync::RwLock::new(None));

    // Create metrics state early so the engine executor can wire Prometheus
    // gauges (jito_health, circuit_breaker, etc.). Created before the engine
    // because the executor's metrics handle is set at construction time and
    // cannot be retrofitted once the engine is spawned.
    let metrics_state = match MetricsState::new() {
        Ok(state) => Arc::new(state),
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize metrics system — /metrics endpoint unavailable, core service will continue");
            return Err(anyhow::anyhow!("Metrics initialization failed: {}", e));
        }
    };

    // Create engine
    let (engine, _engine_handle) =
        engine::Engine::new_with_extras_tip_manager_price_cache_and_token_parser(
            config.clone(),
            db_pool.clone(),
            notifier.clone(),
            Some(metrics_state.clone()),
            Some(ws_state.clone()),
            Some(tip_manager.clone()),
            Some(price_cache.clone()),
            Some(token_parser.clone()),
            Some(portfolio_heat.clone()),
            Some(state_registry.clone()), // state_registry for fast portfolio heat
            None, // write_queue
            Some(wallet_performance_tracker.clone()),
            Some(toxic_flow_detector.clone()),
            Some(verdict_cache.clone()),
        );
    tracing::info!("Engine created");

    let mut task_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Spawn engine
    task_handles.push(tokio::spawn(async move {
        engine.run().await;
    }));
    tracing::info!("Engine task spawned");

    // Spawn recovery manager
    let recovery_manager = Arc::new(RecoveryManager::new_with_rpc(
        db_pool.clone(),
        _engine_handle.clone(),
        Some(ws_state.clone()),
    ));
    let recovery_clone = recovery_manager.clone();
    task_handles.push(tokio::spawn(async move {
        recovery_clone.start_background_task().await;
    }));
    tracing::info!("Recovery manager task spawned");

    // Periodic EXECUTING cleanup
    {
        let exec_cleanup_db = db_pool.clone();
        let ap_cfg = auto_promote_cfg;
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match exec_cleanup_db.recover_executing_trades().await {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        count = n,
                        "Periodic sweep: recovered stuck EXECUTING trades to FAILED"
                    ),
                    Err(e) => tracing::error!(error = %e, "Periodic EXECUTING cleanup failed"),
                }
                // Reclaim orphaned slots (NULL token_amount, >10 min old).
                cleanup_orphaned_positions(&exec_cleanup_db, 600).await;
                // Refill the ACTIVE wallet roster from high-WQS CANDIDATEs.
                if ap_cfg.0 {
                    auto_promote_wallets(&exec_cleanup_db, ap_cfg.3, ap_cfg.1, ap_cfg.2, ap_cfg.4)
                        .await;
                }
            }
        }));
    }

    // Spawn PnL refresh task — updates unrealized_pnl_percent every 30 seconds for active positions
    {
        let pnl_db = db_pool.clone();
        let pnl_pc = price_cache.clone();
        let pnl_token = cancel_token.clone();
        // Tracked in task_handles + cancellable so in-flight DB writes finish
        // cleanly on shutdown instead of being aborted mid-write.
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = pnl_token.cancelled() => break,
                    _ = interval.tick() => {
                match pnl_db.get_active_position_tokens().await {
                    Ok(positions) => {
                        for pos in positions {
                            pnl_pc.track_token(&pos.token_address);
                            if pnl_pc.get_price_usd(&pos.token_address).is_none() {
                                // No fresh price in cache — eager-fetch so PnL and
                                // exit checks get live data immediately instead of
                                // waiting up to PRICE_UPDATE_INTERVAL_SECS.
                                let pc = pnl_pc.clone();
                                let token = pos.token_address.clone();
                                tokio::spawn(async move {
                                    pc.eager_fetch_token(&token).await;
                                });
                            }
                            if let Some(current_usd) = pnl_pc.get_price_usd(&pos.token_address) {
                                let entry = if pos.entry_price.is_zero() {
                                    current_usd
                                } else {
                                    pos.entry_price
                                };
                                // Get current SOL/USD price for converting USD prices to SOL terms
                                let current_sol_price = pnl_pc.get_sol_price_usd();
                                let pnl_sol = match (pos.entry_sol_price_usd, current_sol_price) {
                                    (Some(entry_sol), Some(curr_sol))
                                        if !entry_sol.is_zero() && !curr_sol.is_zero() =>
                                    {
                                        // Convert both entry and current USD prices to SOL-denominated terms
                                        let entry_price_sol = pos.entry_price / entry_sol;
                                        let current_price_sol = current_usd / curr_sol;
                                        let token_amount = pos.entry_amount_sol / entry_price_sol;
                                        (current_price_sol - entry_price_sol) * token_amount
                                    }
                                    // Fallback: if SOL price unavailable, compute with what we have
                                    _ => {
                                        if !entry.is_zero() {
                                            let usd_pnl = current_usd - entry;
                                            // Approximate SOL PnL using entry SOL price if available
                                            // or just use the USD difference scaled by entry ratio
                                            match pos.entry_sol_price_usd {
                                                Some(entry_sol) if !entry_sol.is_zero() => {
                                                    let pnl_fraction = usd_pnl / entry;
                                                    // Scale USD return to SOL terms
                                                    pnl_fraction
                                                        * pos.entry_amount_sol
                                                        * (entry / entry_sol)
                                                }
                                                _ => {
                                                    // Last resort: USD difference (misleading but won't crash)
                                                    tracing::warn!(
                                                        token = %pos.token_address,
                                                        "SOL price unavailable for PnL calc — using approximate value"
                                                    );
                                                    (current_usd - entry) / entry
                                                        * pos.entry_amount_sol
                                                }
                                            }
                                        } else {
                                            rust_decimal::Decimal::ZERO
                                        }
                                    }
                                };
                                let pnl_pct = if !entry.is_zero() {
                                    (current_usd - entry) / entry * rust_decimal::Decimal::from(100)
                                } else {
                                    rust_decimal::Decimal::ZERO
                                };
                                if let Err(e) = pnl_db
                                    .update_position_unrealized_pnl(
                                        &pos.trade_uuid,
                                        current_usd,
                                        pnl_sol,
                                        pnl_pct,
                                    )
                                    .await
                                {
                                    tracing::warn!(error = %e, token = %pos.token_address,
                                        "PnL refresh: failed to update position");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PnL refresh: failed to fetch active positions")
                    }
                }
                    }
                }
            }
        }));
    }
    tracing::info!("PnL refresh task spawned");

    // Spawn circuit breaker evaluation task with notification support
    let circuit_breaker_clone = circuit_breaker.clone();
    let notifier_cb = notifier.clone();
    let notify_rules = config.notifications.rules.clone();
    let cb_token = cancel_token.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut was_tripped = false;

        loop {
            tokio::select! {
                _ = cb_token.cancelled() => {
                    tracing::info!("Shutting down circuit breaker task");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = circuit_breaker_clone.evaluate().await {
                        tracing::error!(error = %e, "Circuit breaker evaluation failed");
                    }

                    // Recovery notification: trip notification is sent directly
                    // by trip() in circuit_breaker.rs (immediate, including manual trips).
                    let is_active = circuit_breaker_clone.is_trading_allowed();
                    if is_active && was_tripped {
                        was_tripped = false;
                        if notify_rules.circuit_breaker_triggered {
                            notifier_cb
                                .notify(NotificationEvent::CircuitBreakerRecovered)
                                .await;
                        }
                    } else if !is_active {
                        was_tripped = true;
                    }
                }
            }
        }
    });
    tracing::info!("Circuit breaker task spawned");

    // Spawn DLQ retry worker task
    let dlq_token = cancel_token.clone();
    let dlq_pool = db_pool.clone();
    let dlq_engine = _engine_handle.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // Every 5 minutes
        loop {
            tokio::select! {
                _ = dlq_token.cancelled() => {
                    tracing::info!("Shutting down DLQ retry worker");
                    break;
                }
                _ = interval.tick() => {
                    // Fetch retryable items from DLQ via the Database trait
                    match dlq_pool.get_retryable_dlq_items(50).await {
                        Ok(items) => {
                            const MAX_DLQ_RETRIES: i64 = 3;
                            tracing::info!(count = items.len(), "Processing DLQ retry items");

                            // Phase 1: Increment retry counts for all items
                            let update_params: Vec<chimera_operator::db_abstraction::UpdateDlqItemParams> = items
                                .iter()
                                .map(|item| {
                                    let new_count = item.retry_count + 1;
                                    let can_still_retry = new_count < MAX_DLQ_RETRIES;
                                    chimera_operator::db_abstraction::UpdateDlqItemParams {
                                        trade_uuid: item.trade_uuid.clone(),
                                        retry_count: new_count,
                                        can_retry: can_still_retry,
                                        mark_processed: false,
                                    }
                                })
                                .collect();

                            if let Err(e) = dlq_pool.update_dlq_items_batch(update_params).await {
                                tracing::error!(error = %e, "Failed to batch update DLQ retry counts");
                                continue;
                            }

                            // Phase 2: Parse payloads and collect the trades to re-inject
                            let mut status_updates: Vec<chimera_operator::db_abstraction::UpdateTradeStatus> = Vec::new();

                            for item in &items {
                                let new_count = item.retry_count + 1;
                                let can_still_retry = new_count < MAX_DLQ_RETRIES;

                                if !can_still_retry {
                                    tracing::warn!(
                                        uuid = %item.trade_uuid,
                                        retry_count = new_count,
                                        "DLQ item permanently failed after max retries"
                                    );
                                    continue;
                                }

                                // Parse payload, reconstruct signal, and re-inject into engine
                                match serde_json::from_str::<SignalPayload>(&item.payload) {
                                    Ok(payload) => {
                                        let trade_uuid = payload.trade_uuid.clone()
                                            .unwrap_or_else(|| item.trade_uuid.clone());
                                        status_updates.push(chimera_operator::db_abstraction::UpdateTradeStatus {
                                            trade_uuid: trade_uuid.clone(),
                                            status: "QUEUED".to_string(),
                                            tx_signature: None,
                                            error_message: None,
                                            network_fee_sol: None,
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, uuid = %item.trade_uuid, "Failed to parse DLQ payload as SignalPayload");
                                    }
                                }
                            }

                            // Phase 3: Re-inject each signal into the engine and set trade to QUEUED.
                            // Use an atomic conditional UPDATE (WHERE status = 'DEAD_LETTER')
                            // to avoid a TOCTOU race where the trade transitions to
                            // ACTIVE between a SELECT and UPDATE.
                            // Only items that are successfully queued are marked as processed
                            // (Phase 4) — marking them before the queue_signal would drop a
                            // failed item from future retry cycles (processed_at IS NULL is
                            // the retry filter).
                            let mut successfully_queued: Vec<chimera_operator::db_abstraction::UpdateDlqItemParams> = Vec::new();
                            let mut updated_count = 0;
                            for status_update in &status_updates {
                                // Conditionally move DEAD_LETTER → QUEUED
                                let result = match dlq_pool.pool() {
                                    crate::db_abstraction::DbPool::PostgreSQL(ref pool) => {
                                        sqlx::query(
                                            r#"
                                            UPDATE trades
                                            SET status = 'QUEUED', error_message = NULL
                                            WHERE trade_uuid = $1 AND status = 'DEAD_LETTER'
                                            "#,
                                        )
                                        .bind(&status_update.trade_uuid)
                                        .execute(pool)
                                        .await
                                    }
                                };

                                match result {
                                    Ok(res) if res.rows_affected() > 0 => {
                                        // Re-parse payload to reconstruct signal for re-injection
                                        let dlq_item = items.iter().find(|i| {
                                            i.trade_uuid == status_update.trade_uuid
                                        });
                                        if let Some(dlq_item) = dlq_item {
                                            match serde_json::from_str::<SignalPayload>(&dlq_item.payload) {
                                                Ok(payload) => {
                                                    let signal = Signal::new(
                                                        payload,
                                                        chrono::Utc::now().timestamp(),
                                                        None,
                                                    );
                                                    // Look up wallet WQS for proper routing
                                                    let wallet_wqs = dlq_pool
                                                        .get_wallet(&signal.payload.wallet_address)
                                                        .await
                                                        .ok()
                                                        .flatten()
                                                        .and_then(|w| w.wqs_score)
                                                        .and_then(|wqs| wqs.to_f64());
                                                    match dlq_engine.queue_signal(signal.clone(), wallet_wqs).await {
                                                        Ok(_) => {
                                                            updated_count += 1;
                                                            // Only now mark the DLQ item as processed.
                                                            successfully_queued.push(chimera_operator::db_abstraction::UpdateDlqItemParams {
                                                                trade_uuid: status_update.trade_uuid.clone(),
                                                                retry_count: dlq_item.retry_count + 1,
                                                                can_retry: (dlq_item.retry_count + 1) < MAX_DLQ_RETRIES,
                                                                mark_processed: true,
                                                            });
                                                            tracing::info!(
                                                                trade_uuid = %status_update.trade_uuid,
                                                                retry_count = dlq_item.retry_count + 1,
                                                                "DLQ retry: signal re-injected into engine"
                                                            );
                                                        }
                                                        Err(e) => {
                                                            tracing::warn!(
                                                                trade_uuid = %status_update.trade_uuid,
                                                                error = %e,
                                                                "DLQ retry: failed to queue signal — reverting to DEAD_LETTER"
                                                            );
                                                            // Revert to DEAD_LETTER so it can be retried next cycle.
                                                            // The DLQ row keeps processed_at NULL (never marked),
                                                            // so the retry worker picks it up again.
                                                            let _ = dlq_pool.update_trade_status(&chimera_operator::db_abstraction::UpdateTradeStatus {
                                                                trade_uuid: status_update.trade_uuid.clone(),
                                                                status: "DEAD_LETTER".to_string(),
                                                                tx_signature: None,
                                                                error_message: Some(format!("DLQ re-queue failed: {}", e)),
                                                                network_fee_sol: None,
                                                            }).await;
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        trade_uuid = %status_update.trade_uuid,
                                                        error = %e,
                                                        "DLQ retry: failed to re-parse payload for re-injection"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Ok(_) => {
                                        tracing::info!(
                                            trade_uuid = %status_update.trade_uuid,
                                            "DLQ retry: skipping — trade no longer DEAD_LETTER"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            trade_uuid = %status_update.trade_uuid,
                                            error = %e,
                                            "DLQ retry: conditional UPDATE failed"
                                        );
                                    }
                                }
                            }

                            // Phase 4: Batch mark only the successfully re-injected items as processed.
                            if !successfully_queued.is_empty() {
                                if let Err(e) = dlq_pool.update_dlq_items_batch(successfully_queued).await {
                                    tracing::error!(error = %e, "Failed to batch mark DLQ items as processed");
                                }
                            }
                            tracing::info!("DLQ batch: {}/{} items re-injected into engine", updated_count, status_updates.len());
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to fetch DLQ items");
                        }
                    }
                }
            }
        }
    });
    tracing::info!("DLQ retry worker spawned");

    // Spawn price cache updater
    let price_cache_clone = price_cache.clone();
    let pc_token = cancel_token.clone();
    task_handles.push(tokio::spawn(async move {
        tokio::select! {
            _ = pc_token.cancelled() => {
                tracing::info!("Price cache updater cancelled on shutdown");
            }
            _ = async {
                price_cache_clone.start_updater().await;
                // start_updater only returns on error or shutdown; log so silent crashes are visible.
                tracing::error!("Price cache updater exited — token price data will become stale. All price-dependent checks (stop-loss, circuit breaker USD thresholds) are now degraded.");
            } => {}
        }
    }));

    // FIX 1+2+4: Spawn unified cache updater (liquidity + metadata monitoring + active age fetching)
    let token_fetcher_for_updater = if let Some(ref helius) = helius_client {
        // Create a new token_fetcher with HeliusClient for active age fetching in background
        Arc::new(
            TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
                &config.rpc.primary_url,
                Some(rpc_rate_limiter.clone()),
                config.jupiter.api_url.clone(),
            )
            .with_price_cache(price_cache.clone())
            .with_unlisted_heuristic(config.token_safety.allow_unlisted_heuristic)
            .with_liquidity_ttl(config.token_safety.liquidity_cache_ttl_secs)
            .with_fdv_ttl(config.token_safety.fdv_cache_ttl_secs)
            .with_helius_client(helius.clone()),
        )
    } else {
        // No HeliusClient available — reuse the shared token_fetcher so the
        // background updater populates the SAME metadata cache consumed by
        // token_parser / pre-validator. (Arc::try_unwrap always fails here
        // because the fetcher is shared, and building an independent one would
        // leave the hot-path cache permanently stale.)
        Arc::clone(&token_fetcher)
    };

    let token_fetcher_clone = token_fetcher_for_updater;
    let ucu_token = cancel_token.clone();
    task_handles.push(tokio::spawn(async move {
        tokio::select! {
            _ = ucu_token.cancelled() => {
                tracing::info!("Unified cache updater cancelled on shutdown");
            }
            _ = async {
                token_fetcher_clone.start_cache_updater().await;
                // This task runs indefinitely; if it exits, cached data will become stale.
                tracing::error!("Unified cache updater exited — cached liquidity and metadata data will become stale. Pre-validation may reject trades due to stale cache data.");
            } => {}
        }
    }));

    // Spawn daily summary notification task
    let notifier_daily = notifier.clone();
    let db_pool_daily = db_pool.clone();
    let daily_config = config.notifications.daily_summary.clone();
    let notify_daily_enabled = config.notifications.rules.daily_summary;

    // Create a specific cancellation token for this if needed, or rely on main process exit
    tokio::spawn(async move {
        if !daily_config.enabled || !notify_daily_enabled {
            return;
        }

        tracing::info!("Daily summary task started");

        loop {
            let now = Utc::now();
            let target_hour = daily_config.hour_utc as u32;
            let target_minute = daily_config.minute as u32;

            let mut next_run = now
                .date_naive()
                .and_hms_opt(target_hour, target_minute, 0)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        target_hour,
                        target_minute,
                        "Invalid daily_summary time in config, defaulting to 00:00 UTC"
                    );
                    now.date_naive()
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight always valid")
                })
                .and_utc();

            if next_run <= now {
                next_run += chrono::Duration::days(1);
            }

            let sleep_duration = (next_run - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(3600));
            tokio::time::sleep(sleep_duration).await;

            match generate_daily_summary(db_pool_daily.as_ref()).await {
                Ok((pnl_usd, trade_count, win_rate)) => {
                    notifier_daily
                        .notify(NotificationEvent::DailySummary {
                            pnl_usd,
                            trade_count,
                            win_rate,
                        })
                        .await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to generate daily summary");
                }
            }
        }
    });

    // Spawn TTL expiration background task
    let db_pool_ttl = db_pool.clone();
    let ttl_token = cancel_token.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = ttl_token.cancelled() => break,
                _ = interval.tick() => {
                    match db_pool_ttl.get_expired_ttl_wallets().await {
                        Ok(expired_wallets) => {
                            for address in expired_wallets {
                                tracing::info!(wallet = %address, "Demoting wallet due to TTL expiration");
                                if let Err(e) = db_pool_ttl.demote_wallet(
                                    &address,
                                    "Auto-demoted: TTL expired",
                                )
                                .await
                                {
                                    tracing::error!(wallet = %address, error = %e, "Failed to demote wallet");
                                } else {
                                    let _ = db_pool_ttl.log_config_change(
                                        &format!("wallet:{}", address),
                                        Some("ACTIVE"),
                                        "CANDIDATE",
                                        "SYSTEM_TTL",
                                        Some("TTL expired"),
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to check TTL expirations");
                        }
                    }
                }
            }
        }
    });

    // Spawn periodic RPC health check task
    let engine_handle_rpc = _engine_handle.clone();
    let rpc_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = rpc_token.cancelled() => break,
                _ = interval.tick() => {
                    engine_handle_rpc.refresh_rpc_health().await;
                }
            }
        }
    });
    tracing::info!("RPC health check task started");

    // Spawn periodic memory and disk pressure monitoring task
    let config_clone = config.clone();
    let monitor_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = monitor_token.cancelled() => break,
                _ = interval.tick() => {
                    if config_clone.degradation.memory_monitoring_enabled {
                        match crate::engine::check_memory_pressure().await {
                            Ok(usage) => {
                                if usage >= config_clone.degradation.memory_pressure_threshold {
                                    tracing::warn!(
                                        memory_usage_pct = usage * 100.0,
                                        threshold_pct = config_clone.degradation.memory_pressure_threshold * 100.0,
                                        "Memory pressure detected"
                                    );
                                } else {
                                    tracing::debug!(
                                        memory_usage_pct = usage * 100.0,
                                        "Memory usage normal"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to check memory pressure");
                            }
                        }
                    }

                    if config_clone.degradation.disk_monitoring_enabled {
                        // Check disk space in current directory
                        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                        match crate::engine::check_disk_space(&current_dir).await {
                            Ok(free_space) => {
                                if free_space <= config_clone.degradation.disk_space_warning_threshold {
                                    tracing::warn!(
                                        free_space_pct = free_space * 100.0,
                                        threshold_pct = config_clone.degradation.disk_space_warning_threshold * 100.0,
                                        "Disk space low"
                                    );
                                } else {
                                    tracing::debug!(
                                        free_space_pct = free_space * 100.0,
                                        "Disk space normal"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to check disk space");
                            }
                        }
                    }
                }
            }
        }
    });
    tracing::info!("Memory and disk pressure monitoring task started");

    // Spawn periodic log pruning task
    let config_prune = config.clone();
    let prune_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 minutes
        loop {
            tokio::select! {
                _ = prune_token.cancelled() => break,
                _ = interval.tick() => {
                    if config_prune.degradation.log_pruning_enabled {
                        let log_dir = std::env::var("CHIMERA_LOG_DIR")
                            .ok()
                            .filter(|v| !v.is_empty())
                            .unwrap_or_else(|| "/app/data/logs".into());
                        let log_dir = std::path::PathBuf::from(log_dir);
                        let max_age_days = 7; // Default: prune logs older than 7 days
                        match crate::engine::prune_logs_if_needed(&log_dir, max_age_days).await {
                            Ok(_) => {
                                tracing::debug!("Log pruning check completed");
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to prune logs");
                            }
                        }
                    }
                }
            }
        }
    });
    tracing::info!("Log pruning task started");

    // Build position risk managers and spawn monitoring loop
    let market_regime_detector = Arc::new(MarketRegimeDetector::new(price_cache.clone()));
    // Create SignalAggregator early so the stop-loss manager can read consensus from
    // its in-memory cache instead of issuing a DB query on every 5-second position tick.
    let signal_aggregator = Arc::new(SignalAggregator::new(db_pool.clone()));
    {
        let stop_loss_mgr = Arc::new(StopLossManager::new(
            db_pool.clone(),
            Arc::new(config.profit_management.clone()),
            price_cache.clone(),
        ));
        stop_loss_mgr
            .set_signal_aggregator(signal_aggregator.clone())
            .await;
        let volume_cache = shared_volume_cache.clone();
        let momentum_exit = Arc::new(MomentumExit::with_volume_cache(
            db_pool.clone(),
            price_cache.clone(),
            volume_cache,
            config.profit_management.wick_protection_secs,
        ));
        let profit_target_mgr = Arc::new(ProfitTargetManager::with_extras(
            db_pool.clone(),
            Arc::new(config.profit_management.clone()),
            price_cache.clone(),
            Some(momentum_exit),
            Some(market_regime_detector.clone()),
        ));

        // Dedicated HWM sweep task — runs every 5 minutes independent of the position
        // monitoring loop so memory is reclaimed even if that loop stalls or panics.
        {
            let sweep_pt = Arc::clone(&profit_target_mgr);
            let sweep_token = cancel_token.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
                loop {
                    tokio::select! {
                        _ = sweep_token.cancelled() => break,
                        _ = interval.tick() => {
                            let removed = sweep_pt.sweep_hwm_stale_entries().await;
                            if removed > 0 {
                                tracing::debug!(removed, "HWM sweep: removed stale entries");
                            }
                        }
                    }
                }
            });
            tracing::info!("HWM sweep task spawned (5-min interval)");
        }

        let monitor_db = db_pool.clone();
        let monitor_sl = stop_loss_mgr;
        let monitor_pt = profit_target_mgr;
        let monitor_engine = _engine_handle.clone();
        let monitor_token = cancel_token.clone();
        let monitor_pc = price_cache.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut last_checked: std::collections::HashMap<String, std::time::Instant> =
                std::collections::HashMap::new();
            let mut db_fail_count: u32 = 0;
            loop {
                tokio::select! {
                    _ = monitor_token.cancelled() => {
                        tracing::info!("Shutting down position monitoring task");
                        break;
                    }
                    _ = interval.tick() => {
                        let positions = match monitor_db.get_active_positions_with_entry().await {
                            Ok(p) => { db_fail_count = 0; p }
                            Err(e) => {
                                db_fail_count += 1;
                                if db_fail_count >= 3 {
                                    tracing::error!(
                                        consecutive_failures = db_fail_count,
                                        error = %e,
                                        "Position monitor: repeated DB failures — positions not being monitored"
                                    );
                                } else {
                                    tracing::warn!(error = %e, "Position monitor: DB query failed, will retry next tick");
                                }
                                continue;
                            }
                        };

                        let now = std::time::Instant::now();
                        for pos in &positions {
                            if let Some(&last) = last_checked.get(&pos.trade_uuid) {
                                if now.duration_since(last) > std::time::Duration::from_secs(60) {
                                    tracing::error!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        elapsed_secs = %now.duration_since(last).as_secs(),
                                        "MONITOR_STALENESS_ALERT: Position monitoring is stale (not checked for > 60s)"
                                    );
                                }
                            }
                        }

                        for pos in &positions {
                            last_checked.insert(pos.trade_uuid.clone(), now);
                            monitor_pc.track_token(&pos.token_address);
                        }

                        last_checked.retain(|uuid, _| positions.iter().any(|p| &p.trade_uuid == uuid));

                        for pos in positions {
                            let current_price = monitor_pc.get_price_usd(&pos.token_address);
                            if current_price.is_none() {
                                tracing::warn!(
                                    trade_uuid = %pos.trade_uuid,
                                    token = %pos.token_address,
                                    "position_monitor: no price available this tick (stale or untracked feed)"
                                );
                            }
                            let pnl_pct = match (current_price, pos.entry_price.is_zero()) {
                                (Some(cp), false) => Some(
                                    ((cp - pos.entry_price) / pos.entry_price)
                                        * rust_decimal::Decimal::from(100),
                                ),
                                _ => None,
                            };
                            let loss_pct = pnl_pct.map(|p| {
                                if p < rust_decimal::Decimal::ZERO {
                                    p
                                } else {
                                    rust_decimal::Decimal::ZERO
                                }
                            });
                            let profit_pct = pnl_pct.map(|p| {
                                if p > rust_decimal::Decimal::ZERO {
                                    p
                                } else {
                                    rust_decimal::Decimal::ZERO
                                }
                            });
                            let stop_loss_distance_pct = loss_pct.map(|p| p.abs());
                            let est_pnl_sol = pnl_pct
                                .map(|p| (p / rust_decimal::Decimal::from(100)) * pos.entry_amount_sol);
                            let elapsed_secs = chrono::Utc::now()
                                .signed_duration_since(pos.entry_time)
                                .num_seconds();

                            // Check stop-loss first (higher priority)
                            let sl_action = monitor_sl.check_stop_loss(
                                &pos.trade_uuid,
                                &pos.wallet_address,
                                pos.entry_price,
                                &pos.token_address,
                                pos.entry_time,
                            ).await;

                            if sl_action == StopLossAction::Exit {
                                tracing::warn!(
                                    trade_uuid = %pos.trade_uuid,
                                    token = %pos.token_address,
                                    exit_reason = "stop_loss",
                                    exit_price = ?current_price,
                                    pnl_percent = ?pnl_pct,
                                    pnl_sol = ?est_pnl_sol,
                                    "Stop-loss triggered, queuing EXIT signal"
                                );
                                tracing::debug!(
                                    trade_uuid = %pos.trade_uuid,
                                    token = %pos.token_address,
                                    wallet = %pos.wallet_address,
                                    strategy = %pos.strategy,
                                    side = "long",
                                    entry_price = %pos.entry_price,
                                    current_price = ?current_price,
                                    loss_pct = ?loss_pct,
                                    profit_pct = ?profit_pct,
                                    stop_loss_distance_pct = ?stop_loss_distance_pct,
                                    elapsed_secs = elapsed_secs,
                                    est_pnl_sol = ?est_pnl_sol,
                                    stop_loss_triggered = true,
                                    profit_target_triggered = false,
                                    exit = "stop_loss",
                                    "position_monitor: tick"
                                );
                                let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                                if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                    tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Stop-loss signal failed — will retry next monitoring cycle");
                                    continue;
                                }
                                monitor_pt.remove_position(&pos.trade_uuid).await;
                                continue;
                            }

                            // Register position with profit target manager (idempotent).
                            // Pass the actual trade open time so time-based exits fire
                            // correctly even after a restart.
                            let entry_st: std::time::SystemTime = pos.entry_time.into();
                            monitor_pt.register_position(
                                &pos.trade_uuid,
                                pos.entry_price,
                                pos.entry_amount_sol,
                                &pos.token_address,
                                entry_st,
                            ).await;

                            // Check profit targets
                            let pt_action = monitor_pt
                                .check_targets(&pos.trade_uuid, &pos.token_address, &pos.strategy)
                                .await;
                            let pt_triggered = !matches!(pt_action, ProfitTargetAction::None);
                            let pt_exit = match pt_action {
                                ProfitTargetAction::FullExit => "full_profit_target",
                                ProfitTargetAction::ExitAmount(_) => "partial_profit_target",
                                ProfitTargetAction::None => "none",
                            };
                            match pt_action {
                                ProfitTargetAction::FullExit => {
                                    tracing::info!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        exit_reason = "full_profit_target",
                                        exit_price = ?current_price,
                                        pnl_percent = ?pnl_pct,
                                        pnl_sol = ?est_pnl_sol,
                                        "Full profit target reached, queuing EXIT signal"
                                    );
                                    let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                                    if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                        tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Full profit target signal failed — will retry");
                                    } else {
                                        monitor_pt.remove_position(&pos.trade_uuid).await;
                                    }
                                }
                                ProfitTargetAction::ExitAmount(amount_sol) => {
                                    tracing::info!(
                                        trade_uuid = %pos.trade_uuid,
                                        token = %pos.token_address,
                                        exit_reason = "partial_profit_target",
                                        amount_sol = %amount_sol,
                                        exit_price = ?current_price,
                                        pnl_percent = ?pnl_pct,
                                        pnl_sol = ?est_pnl_sol,
                                        "Partial profit target reached, queuing partial EXIT signal"
                                    );
                                    let signal = build_exit_signal_amount(&pos, amount_sol);
                                    if let Err(e) = monitor_engine.queue_signal(signal, None).await {
                                        tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Partial profit target signal failed — will retry");
                                    }
                                }
                                ProfitTargetAction::None => {}
                            }

                            tracing::debug!(
                                trade_uuid = %pos.trade_uuid,
                                token = %pos.token_address,
                                wallet = %pos.wallet_address,
                                strategy = %pos.strategy,
                                side = "long",
                                entry_price = %pos.entry_price,
                                current_price = ?current_price,
                                loss_pct = ?loss_pct,
                                profit_pct = ?profit_pct,
                                stop_loss_distance_pct = ?stop_loss_distance_pct,
                                elapsed_secs = elapsed_secs,
                                est_pnl_sol = ?est_pnl_sol,
                                stop_loss_triggered = false,
                                profit_target_triggered = pt_triggered,
                                exit = pt_exit,
                                "position_monitor: tick"
                            );
                        }
                    }
                }
            }
        });
        tracing::info!("Position monitoring task started");
    }

    // Shadow position monitor: checks exit strategies for paper positions.
    // 60s interval (was 15s): with 10k+ open paper positions, 15s ticks
    // generated ~4,700 queries/sec (ON CONFLICT insert attempts + per-position
    // COUNT checks) which drove postgres session memory growth to ~2GB/backend.
    {
        let shadow_monitor = shadow_trader.clone();
        let shadow_token = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = shadow_token.cancelled() => {
                        tracing::info!("Shadow position monitor shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        shadow_monitor.check_exits().await;
                    }
                }
            }
        });
        tracing::info!("Shadow position monitor task started");
    }

    // FIX [B-M3]: Removed duplicate wallet TTL expiration task (3600s interval).
    // The 60s interval task above (around line 505) already handles TTL expiration.
    // Having a second task at 60-minute intervals duplicated demote_wallet calls.

    // Wire Prometheus metrics into circuit breaker for event-driven updates
    circuit_breaker.set_metrics(
        metrics_state.circuit_breaker_state.clone(),
        metrics_state.circuit_breaker_trips.clone(),
    );

    // Create exit detector early for use in both polling task and monitoring state
    let exit_detector = chimera_operator::monitoring::ExitDetector::new()
        .with_db(db_pool.clone());
    let exit_detector = Arc::new(exit_detector);

    // Spawn metrics update task
    let metrics_state_clone = metrics_state.clone();
    let circuit_breaker_clone = circuit_breaker.clone();
    let db_pool_metrics = db_pool.clone();
    let engine_handle_metrics = _engine_handle.clone();
    let ws_state_metrics = ws_state.clone();
    let metrics_token = cancel_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = metrics_token.cancelled() => break,
                _ = interval.tick() => {
                    // CB gauge is updated event-driven by circuit_breaker.rs — no polling needed.
                    let is_active = circuit_breaker_clone.is_trading_allowed();

                    // Update RPC health
                    if let Some(rpc_health) = engine_handle_metrics.get_rpc_health().await {
                        metrics_state_clone
                            .rpc_health
                            .set(if rpc_health.healthy { 1 } else { 0 });
                    }

                    // Update active positions count
                    if let Ok(positions) = db_pool_metrics.get_active_positions().await {
                        let count = positions.len() as i64;
                        metrics_state_clone.active_positions.set(count);
                    }

                    // Update total trades count
                    if let Ok(count) = db_pool_metrics.count_trades_filtered(None, None, None, None, None).await {
                        metrics_state_clone.total_trades.set(count);
                    }

                    // Broadcast health update via WebSocket
                    ws_state_metrics.broadcast(chimera_operator::handlers::WsEvent::HealthUpdate(
                        chimera_operator::handlers::HealthUpdateData {
                            status: "healthy".to_string(), // Could be more sophisticated
                            queue_depth: engine_handle_metrics.queue_depth(),
                            trading_allowed: is_active,
                        },
                    ));
                }
            }
        }
    });
    tracing::info!("Metrics update task started");

    // Start RPC polling task if enabled
    if config
        .monitoring
        .as_ref()
        .map(|m| m.rpc_polling_enabled)
        .unwrap_or(false)
    {
        let interval_secs = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_interval_secs)
            .unwrap_or(8);
        let batch_size = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_batch_size)
            .unwrap_or(6);
        let rate_limit = config
            .monitoring
            .as_ref()
            .map(|m| m.rpc_poll_rate_limit)
            .unwrap_or(40);
        let exit_detection_delay_secs = config
            .monitoring
            .as_ref()
            .map(|m| m.exit_detection_delay_secs)
            .unwrap_or(5);

        let polling_config = chimera_operator::monitoring::PollingConfig {
            interval_secs,
            tiered_polling_enabled: config.monitoring.as_ref()
                .map(|m| m.tiered_polling_enabled)
                .unwrap_or(true),
            high_conviction_interval_secs: config.monitoring.as_ref()
                .and_then(|m| m.tiered_polling.as_ref())
                .map(|t| t.high_conviction_interval_secs),
            regular_conviction_interval_secs: config.monitoring.as_ref()
                .and_then(|m| m.tiered_polling.as_ref())
                .map(|t| t.regular_conviction_interval_secs),
            emerging_conviction_interval_secs: config.monitoring.as_ref()
                .and_then(|m| m.tiered_polling.as_ref())
                .map(|t| t.emerging_conviction_interval_secs),
            high_conviction_wqs_threshold: config.monitoring.as_ref()
                .and_then(|m| m.tiered_polling.as_ref())
                .map(|t| t.high_conviction_wqs_threshold),
            regular_conviction_wqs_threshold: config.monitoring.as_ref()
                .and_then(|m| m.tiered_polling.as_ref())
                .map(|t| t.regular_conviction_wqs_threshold),
            batch_size,
            rpc_url: config.rpc.primary_url.clone(),
            rate_limit,
            exit_detection_delay_secs,
            min_position_sol: config.strategy.min_position_sol,
        };

        let polling_db = db_pool.clone();
        let polling_engine = _engine_handle.clone();
        let polling_token = cancel_token.clone();
        let polling_cb = circuit_breaker.clone();
        let polling_tp = token_parser.clone();
        let polling_ed = exit_detector.clone();

        tokio::spawn(async move {
            chimera_operator::monitoring::start_polling_task(
                polling_db,
                polling_engine,
                polling_config,
                polling_token,
                polling_cb,
                polling_tp,
                polling_ed,
            )
            .await;
        });

        tracing::info!(interval_secs, batch_size, "RPC polling task started");
    } else {
        tracing::info!("RPC polling disabled in configuration");
    }

    // B3: Background volume-polling task — fetches DexScreener data for tokens
    // with open ACTIVE positions every 60s, feeding the shared VolumeCache so
    // MomentumExit's volume-drop detection has fresh samples.
    {
        let vol_db = db_pool.clone();
        let vol_dex = dexscreener_client.clone();
        let vol_token = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await; // consume first immediate tick
            loop {
                tokio::select! {
                    _ = vol_token.cancelled() => {
                        tracing::info!("Volume polling task shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        let tokens = match vol_db.get_active_positions_with_entry().await {
                            Ok(positions) => {
                                let set: std::collections::HashSet<String> = positions
                                    .iter()
                                    .map(|p| p.token_address.clone())
                                    .collect();
                                set
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Volume polling: DB query failed");
                                continue;
                            }
                        };
                        for token in &tokens {
                            let _ = vol_dex.get_market_data(token).await;
                        }
                        if !tokens.is_empty() {
                            tracing::debug!(
                                tokens_checked = tokens.len(),
                                "Volume polling: refreshed DexScreener data for open positions"
                            );
                        }
                    }
                }
            }
        });
        tracing::info!("Volume polling task started (60s interval)");
    }

    // Start Helius LaserStream WebSocket if enabled
    if config
        .monitoring
        .as_ref()
        .map(|m| m.use_websocket)
        .unwrap_or(false)
    {
        tracing::info!("LaserStream WebSocket enabled in config, starting client...");

        // Get Helius API key with proper error handling (resolve ${VAR} from env)
        let helius_api_key = {
            let from_config = config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_api_key.clone())
                .unwrap_or_default();
            if from_config.starts_with("${") {
                std::env::var("HELIUS_API_KEY")
                    .map_err(|_| anyhow::anyhow!("HELIUS_API_KEY env var not set"))?
            } else if from_config.is_empty() {
                anyhow::bail!("HELIUS_API_KEY not set in monitoring config")
            } else {
                from_config
            }
        };

        let helius_client = chimera_operator::monitoring::helius::HeliusClient::new(
            helius_api_key.clone(),
            token_fetcher.get_metadata_cache(),
        ).map_err(|e| anyhow::anyhow!("Failed to create Helius client: {}", e))?;

        let laserstream_config = chimera_operator::monitoring::helius_wss::LaserStreamConfig {
            websocket_url: config
                .monitoring
                .as_ref()
                .and_then(|m| m.helius_websocket_url.clone())
                .unwrap_or_else(|| {
                    format!(
                        "wss://mainnet.helius-rpc.com/?api-key={}",
                        helius_api_key
                    )
                }),
            reconnect: config
                .monitoring
                .as_ref()
                .and_then(|m| m.websocket_reconnect.as_ref())
                .map(|ws_reconnect| chimera_operator::monitoring::helius_wss::ReconnectConfig {
                    initial_backoff_secs: ws_reconnect.initial_backoff_secs,
                    max_backoff_secs: ws_reconnect.max_backoff_secs,
                    backoff_multiplier: ws_reconnect.backoff_multiplier,
                    max_attempts: ws_reconnect.max_attempts,
                })
                .unwrap_or_else(chimera_operator::monitoring::helius_wss::ReconnectConfig::default),
            health_timeout_secs: config
                .monitoring
                .as_ref()
                .map(|m| m.websocket_health_timeout_secs)
                .unwrap_or(60),
            commitment: config
                .monitoring
                .as_ref()
                .map(|m| m.websocket_commitment.clone())
                .unwrap_or_else(|| "confirmed".to_string()),
        };

        let laserstream_client = chimera_operator::monitoring::helius_wss::LaserStreamClient::new(
            db_pool.clone(),
            _engine_handle.clone(),
            laserstream_config,
            circuit_breaker.clone(),
            token_parser.clone(),
            std::sync::Arc::new(helius_client),
            exit_detector.clone(),
        );

        // Spawn LaserStream task
        let laserstream_cancel = cancel_token.clone();
        tokio::spawn(async move {
            if let Err(e) = laserstream_client.start(laserstream_cancel).await {
                tracing::error!(error = %e, "LaserStream WebSocket client failed");
            }
        });

        tracing::info!("✓ LaserStream WebSocket client started");
    } else {
        tracing::info!("LaserStream WebSocket disabled in config, relying on webhooks + RPC polling");
    }

    // Spawn market regime price history update task (every 5 minutes)
    {
        let regime_token = cancel_token.clone();
        let detector_clone = market_regime_detector.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                tokio::select! {
                    _ = regime_token.cancelled() => break,
                    _ = interval.tick() => {
                        detector_clone.update_price_history().await;
                    }
                }
            }
        });
    }

    // Spawn rent scavenger background task (every 6 hours)
    if config.trade_mode != chimera_operator::config::TradeMode::Paper {
        if let Ok(scavenger_enabled) = std::env::var("RENT_SCAVENGER_ENABLED") {
            if scavenger_enabled == "true" || scavenger_enabled == "1" {
                use chimera_operator::engine::transaction_builder::load_wallet_keypair;
                
                
                let rpc_url = config.rpc.primary_url.clone();
                
                match vault::load_secrets_with_fallback()
                    .ok()
                    .and_then(|s| load_wallet_keypair(&s).ok())
                {
                    Some(wallet_keypair) => {
                        let rent_scavenger_config = chimera_operator::engine::RentScavengerConfig {
                            enabled: true,
                            interval_secs: std::env::var("RENT_SCAVENGER_INTERVAL_SECS")
                                .unwrap_or_else(|_| "21600".to_string())
                                .parse()
                                .unwrap_or(6 * 3600), // 6 hours default
                            max_batch_size: std::env::var("RENT_SCAVENGER_BATCH_SIZE")
                                .unwrap_or_else(|_| "10".to_string())
                                .parse()
                                .unwrap_or(10),
                            max_rent_lamports: std::env::var("RENT_SCAVENGER_MAX_RENT_LAMPORTS")
                                .unwrap_or_else(|_| "1000000000".to_string())
                                .parse()
                                .unwrap_or(1_000_000_000), // 1 SOL default
                        };

                        // Register with the shared registry so rent-scavenger
                        // metrics are actually scraped at /metrics.
                        let rent_metrics = Arc::new(chimera_operator::metrics::RentScavengerMetrics::new(metrics_state.registry()));
                        // Use the configured interval (RENT_SCAVENGER_INTERVAL_SECS),
                        // not a hardcoded 6-hour ticker.
                        let scavenger_interval_secs = rent_scavenger_config.interval_secs;
                        let rpc_url_clone = rpc_url.clone();
                        let rent_scavenger = chimera_operator::engine::RentScavenger::new(
                            rpc_url_clone,
                            Arc::new(wallet_keypair),
                            rent_scavenger_config,
                            Some(rent_metrics),
                        );

                        let rent_scavenger = Arc::new(rent_scavenger);
                        let rent_token = cancel_token.clone();
                        let circuit_breaker_clone = circuit_breaker.clone();

                        tokio::spawn(async move {
                            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(scavenger_interval_secs));

                            loop {
                                tokio::select! {
                                    _ = rent_token.cancelled() => break,
                                    _ = ticker.tick() => {
                                        // Only run if circuit breaker is healthy
                                        match circuit_breaker_clone.current_state() {
                                            chimera_operator::circuit_breaker::CircuitBreakerState::Active => {
                                                tracing::info!("Rent scavenger: circuit breaker healthy, running reclaim cycle");
                                                if let Err(e) = rent_scavenger.reclaim_empty_accounts().await {
                                                    tracing::error!(error = %e, "Rent scavenger run failed");
                                                }
                                            }
                                            state => {
                                                tracing::debug!(state = %state, "Rent scavenger: circuit breaker not healthy, skipping run");
                                            }
                                        }
                                    }
                                }
                            }
                        });

                        tracing::info!("✓ Rent scavenger started ({}s interval, gated on circuit breaker health)", scavenger_interval_secs);
                    }
                    None => {
                        tracing::warn!("Rent scavenger disabled: failed to load wallet keypair");
                    }
                }
            } else {
                tracing::info!("Rent scavenger disabled via RENT_SCAVENGER_ENABLED=false");
            }
        } else {
            tracing::info!("Rent scavenger disabled (RENT_SCAVENGER_ENABLED not set)");
        }
    } else {
        tracing::info!("Rent scavenger disabled in paper mode");
    }

    // Stale trade reaper: cancel PENDING/QUEUED trades older than threshold
    let stale_trade_max_age = config.monitoring.as_ref()
        .map(|m| m.stale_trade_reaper_minutes)
        .unwrap_or(30);
    if stale_trade_max_age > 0 {
        let stale_trades_db = db_pool.clone();
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // Every 5 minutes
            loop {
                interval.tick().await;
                match stale_trades_db.cancel_stale_trades(stale_trade_max_age).await {
                    Ok(count) if count > 0 => {
                        tracing::info!("Stale trade reaper cancelled {} stale trades", count);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Stale trade reaper error: {}", e);
                    }
                }
            }
        }));
        tracing::info!(max_age_minutes = stale_trade_max_age, "Stale trade reaper started");
    } else {
        tracing::info!("Stale trade reaper disabled (stale_trade_reaper_minutes = 0)");
    }

    tracing::info!("All background tasks spawned");

    // Now create the FULL router with all routes
    tracing::info!("Creating full router with states...");

    let app_state = Arc::new(AppState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        started_at: Utc::now(),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
        trade_mode: config.trade_mode.to_string().to_lowercase(),
        run_context: Some(run_context.clone()),
        last_db_ok_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });

    // signal_aggregator was created earlier (before stop_loss_mgr) so it could be wired
    // into the stop-loss manager's consensus cache. Reuse it here.
    // helius_client was created earlier (around line 450) to avoid forward declaration issues

    // Create webhook API rate limiter for lifecycle management operations
    let webhook_api_rate_limiter: Arc<rate_limiter::RateLimiter> =
        Arc::new(rate_limiter::RateLimiter::new(
            config
                .monitoring
                .as_ref()
                .map(|m| m.webhook_processing_rate_limit)
                .unwrap_or(40),
            1,
        ));

    // Spawn webhook health monitoring task
    if let Some(ref monitoring_config) = config.monitoring {
        if let Some(ref webhook_lifecycle_config) = monitoring_config.webhook_lifecycle {
            if webhook_lifecycle_config.health_check_interval_secs > 0 {
                if let Some(ref helius) = helius_client {
                    let webhook_db = db_pool.clone();
                    let webhook_helius = helius.clone();
                    let webhook_limiter = webhook_api_rate_limiter.clone();
                    let webhook_token = cancel_token.clone();
                    let webhook_url = monitoring_config
                        .helius_webhook_url
                        .clone()
                        .unwrap_or_default();

                    let health_config = chimera_operator::monitoring::WebhookHealthConfig {
                        check_interval_secs: webhook_lifecycle_config.health_check_interval_secs,
                        stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                        webhook_url: webhook_url.clone(),
                        helius_dry_run: webhook_lifecycle_config.helius_dry_run,
                        auto_cleanup_enabled: webhook_lifecycle_config.auto_cleanup_enabled,
                        auth_header: monitoring_config.resolved_helius_auth_header(),
                    };

                    tokio::spawn(async move {
                        chimera_operator::monitoring::webhook_health_task::start_webhook_health_task(
                            webhook_db,
                            webhook_helius,
                            webhook_limiter,
                            health_config,
                            webhook_token,
                        )
                        .await;
                    });

                    tracing::info!(
                        interval_secs = webhook_lifecycle_config.health_check_interval_secs,
                        "Webhook health monitoring task started"
                    );
                }
            }
        }
    }

    // Create API state
    let api_state = Arc::new(ApiState {
        db: db_pool.clone(),
        circuit_breaker: circuit_breaker.clone(),
        config: Arc::new(tokio::sync::RwLock::new(config.clone())),
        notifier: notifier.clone(),
        engine: Some(Arc::new(_engine_handle.clone())),
        metrics: metrics_state.clone(),
        signal_aggregator: Some(signal_aggregator.clone()),
        market_regime_detector: Some(market_regime_detector.clone()),
        helius_client: helius_client.clone(),
        webhook_rate_limiter: Some(webhook_api_rate_limiter.clone()),
        price_cache: price_cache.clone(),
        toxic_detector: Some(toxic_flow_detector.clone()),
        run_context: Some(run_context.clone()),
        decision_recorder: Some(decision_recorder.clone()),
        profitability_verdict: verdict_cache.clone(),
    });

    // Spawn the periodic mark-to-market NAV snapshot writer (dashboard equity curve).
    chimera_operator::monitoring::nav_snapshot::spawn_nav_snapshot_task(
        api_state.db.clone(),
        api_state.config.clone(),
        price_cache.clone(),
        config.trade_mode.to_string(),
        cancel_token.clone(),
    );

    // Run startup webhook management check
    // This ensures all ACTIVE wallets have registered webhooks before server starts
    if config
        .monitoring
        .as_ref()
        .map(|m| m.enabled)
        .unwrap_or(false)
    {
        if let Some(webhook_lifecycle_config) = config
            .monitoring
            .as_ref()
            .and_then(|m| m.webhook_lifecycle.as_ref())
        {
            if webhook_lifecycle_config.auto_register_enabled {
                if let Some(ref startup_helius) = helius_client {
                    let startup_db = db_pool.clone();
                    let startup_rate_limiter = webhook_api_rate_limiter.clone();
                    let startup_webhook_url = config
                        .monitoring
                        .as_ref()
                        .and_then(|m| m.helius_webhook_url.clone())
                        .unwrap_or_default();

                    let startup_config = chimera_operator::monitoring::WebhookHealthConfig {
                        check_interval_secs: webhook_lifecycle_config.health_check_interval_secs,
                        stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                        webhook_url: startup_webhook_url,
                        helius_dry_run: webhook_lifecycle_config.helius_dry_run,
                        auto_cleanup_enabled: webhook_lifecycle_config.auto_cleanup_enabled,
                        auth_header: config
                            .monitoring
                            .as_ref()
                            .and_then(|m| m.resolved_helius_auth_header()),
                    };

                    tracing::info!("Running startup webhook check...");

                    let startup_result = chimera_operator::monitoring::webhook_health_task::run_startup_webhook_check(
                        startup_db,
                        startup_helius.clone(),
                        startup_rate_limiter,
                        startup_config,
                    ).await;

                    match startup_result {
                        Ok(result) => {
                            tracing::info!(
                                wallets_checked = result.wallets_checked,
                                registered = result.registered,
                                orphaned = result.orphaned,
                                cleaned_up = result.cleaned_up,
                                failed = result.failed,
                                duration_ms = result.duration_ms,
                                "Startup webhook check completed"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Startup webhook check failed");
                        }
                    }
                } else {
                    tracing::info!("Helius client not available, skipping startup webhook check");
                }

                // Spawn Helius webhook reconciliation as background task
                // This runs asynchronously and does NOT delay startup
                if webhook_lifecycle_config.helius_reconciliation_enabled {
                    if let Some(ref reconcile_helius) = helius_client {
                        let reconcile_db = db_pool.clone();
                        let reconcile_helius_client = reconcile_helius.clone();
                        let reconcile_rate_limiter = webhook_api_rate_limiter.clone();
                        let reconcile_webhook_url = config
                            .monitoring
                            .as_ref()
                            .and_then(|m| m.helius_webhook_url.clone())
                            .unwrap_or_default();
                        let reconcile_config = chimera_operator::monitoring::WebhookHealthConfig {
                            check_interval_secs: webhook_lifecycle_config
                                .health_check_interval_secs,
                            stale_threshold_days: webhook_lifecycle_config.stale_threshold_days,
                            webhook_url: reconcile_webhook_url,
                            helius_dry_run: webhook_lifecycle_config.helius_dry_run,
                            auto_cleanup_enabled: webhook_lifecycle_config.auto_cleanup_enabled,
                            auth_header: config
                                .monitoring
                                .as_ref()
                                .and_then(|m| m.resolved_helius_auth_header()),
                        };

                        tokio::spawn(async move {
                            tracing::info!("Helius webhook reconciliation task started (async)");
                            match chimera_operator::monitoring::webhook_health_task::reconcile_helius_webhooks_async(
                                reconcile_db,
                                reconcile_helius_client,
                                reconcile_rate_limiter,
                                reconcile_config,
                            ).await {
                                Ok(result) => {
                                    tracing::info!(
                                        total = result.total_helius_webhooks,
                                        eligible = result.eligible_wallets,
                                        ineligible = result.ineligible_wallets,
                                        deleted = result.deleted_webhooks,
                                        duration_ms = result.duration_ms,
                                        "Helius webhook reconciliation completed"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, error_chain = ?e, "Helius webhook reconciliation task failed");
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    // Create operations state
    let operations_state = Arc::new(OperationsState {
        db: db_pool.clone(),
        engine: Some(Arc::new(_engine_handle.clone())),
        circuit_breaker: circuit_breaker.clone(),
        price_cache: price_cache.clone(),
        webhook_rate_limiter: Some(webhook_api_rate_limiter.clone()),
        rpc_rate_limiter: Some(rpc_rate_limiter.clone()),
    });

    // Create auth state (reuse already-loaded api_keys_map and jwt_secret)
    let auth_state = Arc::new(AuthState::with_auth_config(
        api_keys_map.clone(),
        jwt_secret.clone(),
    ));
    tracing::info!(
        api_key_count = config.security.api_keys.len(),
        "Auth state initialized"
    );
    tracing::info!("WebSocket state initialized");

    // Build health routes with AppState
    let health_routes = Router::new()
        .route("/health", get(health_check))
        .with_state(app_state.clone());

    // Rate limiter for public API routes (~60 req/s sustained, burst 100).
    // NOTE: tower_governor's per_second/per_millisecond set the *interval* at which ONE
    // token is replenished (i.e. a period), NOT a per-second rate. per_second(60) would
    // mean only 1 token every 60s. per_millisecond(16) ≈ 1 token / 16ms ≈ 62 tokens/s.
    let public_api_limiter_conf = tower_governor::governor::GovernorConfigBuilder::default()
        .per_millisecond(16)
        .burst_size(100)
        .key_extractor(middleware::ProxyAwareKeyExtractor)
        .finish()
        .ok_or_else(|| anyhow::anyhow!("Failed to build public API rate limiter"))?;
    let public_api_limiter_conf = std::sync::Arc::new(public_api_limiter_conf);
    let public_api_governor_layer = tower_governor::GovernorLayer {
        config: public_api_limiter_conf,
    };

    // Build public read-only API routes (no auth required for dashboard)
    let public_api_routes = Router::new()
        .route("/positions", get(list_positions))
        .route("/positions/:trade_uuid", get(get_position))
        .route("/trades", get(list_trades))
        .route("/trades/export", get(export_trades))
        .route("/metrics/strategy", get(get_strategy_performance))
        .route("/metrics/performance", get(get_performance_metrics))
        .route("/metrics/costs", get(get_cost_metrics))
        .route(
            "/metrics/trade-latency",
            get(chimera_operator::handlers::get_trade_latency),
        )
        .route(
            "/metrics/database-performance",
            get(chimera_operator::handlers::get_database_performance),
        )
        .route(
            "/metrics/request-rate",
            get(chimera_operator::handlers::get_request_rate),
        )
        .route(
            "/metrics/rpc-latency",
            get(chimera_operator::handlers::get_rpc_latency),
        )
        .route(
            "/risk/portfolio",
            get(chimera_operator::handlers::get_portfolio_risk),
        )
        .route(
            "/portfolio/nav-history",
            get(chimera_operator::handlers::get_nav_history),
        )
        .route(
            "/risk/stop-loss",
            get(chimera_operator::handlers::get_stop_loss_metrics),
        )
        .route(
            "/risk/profit-target",
            get(chimera_operator::handlers::get_profit_target_metrics),
        )
        .route(
            "/risk/position-size",
            get(chimera_operator::handlers::get_position_size_analysis),
        )
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
        .route(
            "/signals/quality",
            get(chimera_operator::handlers::get_signal_quality),
        )
        .route(
            "/signals/sources",
            get(chimera_operator::handlers::get_signal_sources),
        )
        .route("/market/regime", get(get_market_regime))
        .route("/market/conditions", get(get_market_conditions))
        // Scout intelligence endpoints
        .route("/scout/status", get(get_scout_status))
        .route("/scout/wqs-distribution", get(get_wqs_distribution))
        .route("/scout/metrics", get(get_scout_metrics))
        // Scout integration features
        .route("/scout/budget", get(get_budget_status))
        .route("/scout/cache", get(get_cache_stats))
        .route("/scout/conviction", get(get_conviction_allocation))
        // C4: Pre-registered profitability go/no-go verdict (paper-only)
        .route(
            "/profitability/verdict",
            get(chimera_operator::handlers::profitability_verdict),
        )
        .with_state(api_state.clone())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            2 * 1024 * 1024,
        ))
        .layer(public_api_governor_layer.clone());

    // Build operations API routes (use OperationsState)
    let operations_routes = Router::new()
        .route("/operations/resources", get(get_resources))
        .route("/operations/secrets", get(get_secrets))
        .route("/operations/rate-limit", get(get_rate_limit_status))
        .route("/operations/health-checks", get(get_health_check_details))
        .with_state(operations_state.clone());

    // Build protected API routes (auth required — includes reads that expose sensitive config)
    let protected_api_routes = Router::new()
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
        // Reconciliation API endpoints
        .route(
            "/reconciliation/status",
            get(chimera_operator::handlers::get_reconciliation_status),
        )
        .route(
            "/reconciliation/history",
            get(chimera_operator::handlers::get_reconciliation_history),
        )
        .route(
            "/reconciliation/stats",
            get(chimera_operator::handlers::get_reconciliation_stats),
        )
        .route(
            "/reconciliation/trigger",
            post(chimera_operator::handlers::trigger_reconciliation),
        )
        .route(
            "/reconciliation/discrepancies/:id/resolve",
            post(chimera_operator::handlers::resolve_discrepancy),
        )
        // Admin cache management
        .route(
            "/admin/caches/clear",
            post(chimera_operator::handlers::clear_monitoring_caches),
        )
        // State-changing operations — protected with bearer auth (these were
        // previously exposed on the unauthenticated public router: retrying a
        // dead-lettered trade can flip it back into the trading queue).
        .route("/incidents/dead-letter/:trade_uuid/retry", post(retry_dead_letter_item))
        .route("/scout/run", post(trigger_scout_run))
        .route("/debug/backtest-smoke", post(debug_backtest_smoke))
        .with_state(api_state.clone())
        .layer(axum_middleware::from_fn_with_state(
            auth_state.clone(),
            bearer_auth,
        ));

    // Create webhook state (token_parser already created above)

    // Refresh total_capital_sol from the live wallet balance every 60 seconds so that
    // compounding gains and drawdown recovery propagate into heat capacity without restart.
    {
        use chimera_operator::engine::transaction_builder::load_wallet_keypair;
        use solana_client::nonblocking::rpc_client::RpcClient as NonblockingRpcClient;
        use solana_sdk::signature::Signer;

        let heat_clone = Arc::clone(&portfolio_heat);
        let cb_clone = Arc::clone(&circuit_breaker);
        let rpc_url = config.rpc.primary_url.clone();
        match vault::load_secrets_with_fallback()
            .ok()
            .and_then(|s| load_wallet_keypair(&s).ok())
        {
            Some(keypair) => {
                let pubkey = keypair.pubkey();
                tokio::spawn(async move {
                    let rpc = NonblockingRpcClient::new(rpc_url);
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        let balance_result = chimera_operator::metrics::timed_rpc(
                            "primary",
                            "getBalance",
                            rpc.get_balance(&pubkey),
                        )
                        .await;
                        match balance_result {
                            Ok(lamports) => {
                                let sol = rust_decimal::Decimal::from(lamports)
                                    / rust_decimal::Decimal::from(1_000_000_000u64);
                                heat_clone.update_capital(sol);
                                // Keep circuit breaker capital in sync so its portfolio-stop
                                // threshold reflects the live balance, not the startup value.
                                cb_clone.update_capital(sol);
                                tracing::debug!(capital_sol = ?sol, "Portfolio capital refreshed from wallet");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to fetch wallet balance for capital refresh");
                            }
                        }
                    }
                });
                tracing::info!("Portfolio capital refresh task spawned (60s interval)");
            }
            None => {
                if config.trade_mode == chimera_operator::config::TradeMode::Paper {
                    tracing::debug!(
                        "Wallet keypair unavailable in Paper mode — portfolio capital uses configured total_capital_sol (no on-chain balance to track)"
                    );
                } else {
                    tracing::warn!(
                        "Wallet keypair unavailable — portfolio capital will not auto-refresh from wallet balance. Import the vault keypair before go-live."
                    );
                }
            }
        }
    }

    // Force-liquidation safety task: if an external capital drain causes portfolio heat
    // to exceed 150% of the configured limit, exit oldest positions until back in range.
    // Runs every 60 seconds — slow enough to not interfere with normal trading, fast
    // enough to act before a margin-call-like cascade.
    {
        let fl_heat = Arc::clone(&portfolio_heat);
        let fl_db = db_pool.clone();
        let fl_engine = _engine_handle.clone();
        let fl_token = cancel_token.clone();
        task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = fl_token.cancelled() => break,
                    _ = interval.tick() => {
                        let overexposed = match fl_heat.is_critically_overexposed().await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(error = %e, "Heat overexposure check failed");
                                false
                            }
                        };
                        if !overexposed {
                            continue;
                        }
                        tracing::warn!("HEAT_OVEREXPOSED: capital drain detected — force-exiting oldest positions");
                        let positions = match fl_db.get_active_positions_with_entry().await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::error!(error = %e, "Force-liquidation: DB query failed");
                                continue;
                            }
                        };
                        let mut simulated_exposure = match fl_heat.calculate_heat().await {
                            Ok(h) => h.total_exposure_sol,
                            Err(_) => rust_decimal::Decimal::ZERO,
                        };
                        let threshold_sol = fl_heat.get_critical_threshold_sol();

                        for pos in positions {
                            if simulated_exposure <= threshold_sol {
                                break;
                            }
                            let signal = build_exit_signal(&pos, rust_decimal::Decimal::ONE);
                            if let Err(e) = fl_engine.queue_signal(signal, None).await {
                                tracing::error!(error = %e, trade_uuid = %pos.trade_uuid, "Force-liquidation signal failed — will retry next cycle");
                                continue;
                            }
                            tracing::warn!(
                                trade_uuid = %pos.trade_uuid,
                                token = %pos.token_address,
                                "Force-exited position (heat overexposure)"
                            );
                            let entry_size = pos.entry_amount_sol;
                            simulated_exposure -= entry_size;
                        }
                    }
                }
            }
        }));
        tracing::info!("Force-liquidation task spawned (60s interval, triggers at 150% heat)");
    }

    // Profitability verdict refresh task: evaluate gates periodically and cache result
    // for live trading enforcement in SignalProcessor.
    if api_state.config.read().await.profitability_gate.enabled {
        let verdict_cache = Arc::clone(&api_state.profitability_verdict);
        let db_pool = db_pool.clone();
        let run_id = run_context.clone();
        let decision_recorder = api_state.decision_recorder.clone();
        let total_capital_sol = {
            let cfg = api_state.config.read().await;
            cfg.position_sizing.total_capital_sol
                .to_f64()
                .unwrap_or(0.0)
                .max(1.0)
        };
        let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(
            api_state.config.read().await.profitability_gate.refresh_interval_seconds,
        ));
        task_handles.push(tokio::spawn(async move {
            loop {
                refresh_interval.tick().await;
                let pg_pool = match db_pool.pool() {
                    chimera_operator::db_abstraction::DbPool::PostgreSQL(p) => p,
                };
                match refresh_verdict(&pg_pool, &run_id, &decision_recorder, total_capital_sol).await {
                    Ok(Some(new_verdict)) => {
                        let old_verdict = {
                            let mut cache = verdict_cache.write().await;
                            std::mem::replace(&mut *cache, Some(new_verdict.clone()))
                        };
                        if old_verdict.is_none() || old_verdict.as_ref().map(|v| &v.verdict) != Some(&new_verdict.verdict) {
                            tracing::warn!(
                                verdict = %new_verdict.verdict,
                                from = old_verdict.map(|v| v.verdict),
                                "Profitability verdict changed"
                            );
                        }
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "Profitability verdict evaluation failed: no cached verdict available"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Profitability verdict evaluation failed, retaining previous verdict"
                        );
                    }
                }
            }
        }));
        tracing::info!(
            "Profitability verdict refresh task spawned ({}s interval, enabled: {})",
            api_state.config.read().await.profitability_gate.refresh_interval_seconds,
            api_state.config.read().await.profitability_gate.enabled
        );
    } else {
        tracing::info!("Profitability verdict gating disabled");
    }

    let position_sizer = Arc::new(PositionSizer::new(
        db_pool.clone(),
        Arc::new(config.position_sizing.clone()),
    ));
    tracing::info!(
        "Position sizer initialized (Kelly sizing: {})",
        config.position_sizing.use_kelly_sizing
    );

    // C3: Shadow-fill calibration. Build a dedicated TransactionBuilder for
    // Jupiter quote capture (the selection engine only needs the quote path,
    // not the executor's signed-swap path). The LatencyTracker records
    // decide() latency percentiles so delayed requotes are scheduled at a
    // realistic offset. Both are shared with SelectionService below.
    let latency_tracker = Arc::new(chimera_operator::engine::LatencyTracker::new(2048));
    let shadow_quote_client = {
        let rpc = solana_client::nonblocking::rpc_client::RpcClient::new(
            config.rpc.primary_url.clone(),
        );
        chimera_operator::engine::transaction_builder::TransactionBuilder::new(
            Arc::new(rpc),
            Arc::new(config.clone()),
        )
        .map(Arc::new)
        .map_err(|e| {
            tracing::warn!(error = %e, "Shadow-fill quote client build failed — paper PnL calibration disabled");
        })
        .ok()
    };
    if shadow_quote_client.is_some() {
        tracing::info!("✓ Shadow-fill quote client initialized (C3 paper PnL calibration)");
    }

    // B1: Unified selection engine shared by both ingress paths (direct
    // webhook + Helius monitoring). Built once with every capability the two
    // handlers collectively need so both run the identical decision pipeline.
    // C1: the shared selection_config + decision_recorder were built right
    // after db init so /health and selection share one run identity.
    let selection_service = Arc::new(
        crate::engine::SelectionService::new(
            db_pool.clone(),
            token_parser.clone(),
            Some(portfolio_heat.clone()),
            Some(signal_aggregator.clone()),
            Some(market_regime_detector.clone()),
            helius_client.clone(),
            Some(position_sizer.clone()),
            selection_config,
        )
        .with_dexscreener(dexscreener_client.clone())
        .with_toxic_detector(toxic_flow_detector.clone())
        .with_mute_detector(rejection_mute_detector.clone())
        .with_decision_recorder(decision_recorder.clone())
        .with_shadow_fill_opt(shadow_quote_client.clone(), latency_tracker.clone())
        .with_wallet_performance(wallet_performance_tracker.clone())
        .with_shadow_trader(shadow_trader.clone()),
    );

    let webhook_state = Arc::new(WebhookState {
        db: db_pool.clone(),
        engine: _engine_handle.clone(),
        token_parser: token_parser.clone(),
        circuit_breaker: circuit_breaker.clone(),
        portfolio_heat: Some(portfolio_heat.clone()),
        signal_aggregator: Some(signal_aggregator.clone()),
        market_regime: Some(market_regime_detector.clone()),
        helius_client: helius_client.clone(),
        position_sizer: Some(position_sizer),
        total_capital_sol: config.position_sizing.total_capital_sol,
        max_position_sol: config.position_sizing.max_size_sol,
        shield_signal_quality_threshold: config.strategy.shield_signal_quality_threshold,
        spear_signal_quality_threshold: config.strategy.spear_signal_quality_threshold,
        shield_percent: config.strategy.shield_percent,
        spear_percent: config.strategy.spear_percent,
        min_liquidity_shield_usd: config.token_safety.min_liquidity_shield_usd,
        min_liquidity_spear_usd: config.token_safety.min_liquidity_spear_usd,
        selection: selection_service.clone(),
    });

    // Build HMAC secrets for webhook verification
    let mut hmac_secrets = Vec::new();
    if !config.security.webhook_secret.is_empty() {
        hmac_secrets.push(config.security.webhook_secret.clone());
    }
    // Try to load from vault if available
    if let Ok(secrets) = vault::load_secrets_with_fallback() {
        if !secrets.webhook_secret.is_empty() {
            hmac_secrets.push(secrets.webhook_secret.clone());
        }
        if let Some(prev) = &secrets.webhook_secret_previous {
            if !prev.is_empty() {
                hmac_secrets.push(prev.clone());
            }
        }
    }
    // Add previous secret from config if available
    if let Some(prev) = &config.security.webhook_secret_previous {
        if !prev.is_empty() && !hmac_secrets.contains(prev) {
            hmac_secrets.push(prev.clone());
        }
    }
    let hmac_state = Arc::new(middleware::HmacState::with_rotation(
        hmac_secrets,
        config.security.max_timestamp_drift_secs,
    )?);

    // Build rate limiter for webhook routes
    let governor_conf = tower_governor::governor::GovernorConfigBuilder::default()
        .per_second(config.security.webhook_rate_limit as u64)
        .burst_size(config.security.webhook_burst_size)
        .key_extractor(middleware::ProxyAwareKeyExtractor)
        .finish()
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to build rate limiter — webhook_rate_limit must be > 0")
        })?;
    let governor_conf = std::sync::Arc::new(governor_conf);
    let governor_layer = tower_governor::GovernorLayer {
        config: governor_conf,
    };

    // Build webhook routes with rate limiting and HMAC middleware
    let webhook_routes = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(webhook_state.clone())
        // Rate limit the unauthenticated public /webhook endpoint (DoS/abuse
        // protection). Previously disabled for paper-trading testing.
        .layer(governor_layer.clone())
        .layer(axum_middleware::from_fn_with_state(
            hmac_state.clone(),
            middleware::hmac_verify,
        ));

    // Build auth routes
    // jwt_secret already defined above
    let auth_routes = Router::new()
        .route("/auth/wallet", post(wallet_auth))
        .route("/auth/refresh", post(refresh_token))
        .with_state(Arc::new(WalletAuthState {
            db: db_pool.clone(),
            jwt_secret,
            // FIX 11: Initialize auth nonce store for replay protection
            seen_auth_nonces: std::sync::Arc::new(parking_lot::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }));

    // Build WebSocket routes — authentication handled within the handler via query parameter
    let ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());

    // Build metrics routes
    let metrics_routes = metrics_router().with_state(metrics_state.clone());

    // Rate limiting is enabled on webhook routes with ProxyAwareKeyExtractor

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build monitoring routes - will be created after engine is initialized
    // Use _engine_handle which is created earlier
    let config_arc = Arc::new(config.clone());
    tracing::info!("Attempting to create MonitoringState...");
    let monitoring_routes = match MonitoringState::new(
        db_pool.clone(),
        _engine_handle.clone(),
        config_arc.clone(),
        Some(token_fetcher.clone()),
    )
    .map(|ms| {
        ms.with_circuit_breaker(circuit_breaker.clone())
            .with_token_parser(token_parser.clone())
            .with_portfolio_heat(portfolio_heat.clone())
            .with_exit_detector(exit_detector.clone())
            .with_selection(selection_service.clone())
    }) {
        Ok(monitoring_state) => {
            let monitoring_state_arc = Arc::new(monitoring_state);
            tracing::info!(
                "Monitoring state initialized successfully, registering monitoring routes"
            );
            // Helius webhook and status are public (Helius calls from external service)
            let monitoring_public = Router::new()
                .route("/monitoring/status", get(get_monitoring_status))
                .route("/monitoring/helius-webhook", post(helius_webhook_handler))
                .with_state(monitoring_state_arc.clone());
            // Enable/disable wallet monitoring require operator role
            let monitoring_protected = Router::new()
                .route(
                    "/monitoring/wallets/:wallet_address/enable",
                    post(enable_wallet_monitoring),
                )
                .route(
                    "/monitoring/wallets/:wallet_address/disable",
                    post(disable_wallet_monitoring),
                )
                // Webhook lifecycle management routes (require operator role)
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
                // Wallet monitoring state (requires readonly+)
                .route(
                    "/monitoring/wallets/states",
                    get(get_wallet_monitoring_states),
                )
                .with_state(monitoring_state_arc)
                .layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    bearer_auth,
                ));
            monitoring_public.merge(monitoring_protected)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to initialize MonitoringState, monitoring routes disabled");
            Router::new()
        }
    };

    // Root-level WebSocket for web dashboard — authentication handled within handler via query parameter
    let root_ws_routes = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state.clone());

    // Create full router with all routes and middleware
    // Note: Layer order matters - bottom layers are applied first (innermost)
    let app = Router::new()
        .route("/ping", get(|| async { "pong" }))
        .route("/health", get(health_simple))
        .merge(root_ws_routes)
        .nest("/api/v1", health_routes)
        .nest("/api/v1", public_api_routes)
        .nest("/api/v1", protected_api_routes)
        .nest("/api/v1", operations_routes)
        .nest("/api/v1", webhook_routes)
        .nest("/api/v1", auth_routes)
        .nest("/api/v1", ws_routes)
        .nest("/api/v1", monitoring_routes)
        .merge(metrics_routes)
        .with_state(app_state.clone())
        .layer(cors)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            10 * 1024 * 1024,
        ))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                )
            }),
        );
    // Rate limiting is applied per-route (webhook routes have governor_layer)

    tracing::info!("Full router created with all routes and middleware");

    // Start server
    let addr: SocketAddr = match format!("{}:{}", config.server.host, config.server.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            tracing::error!(error = %e, host = %config.server.host, port = %config.server.port, "Invalid server address");
            return Err(anyhow::anyhow!(
                "Invalid server address {}:{} - check config: {}",
                config.server.host,
                config.server.port,
                e
            ));
        }
    };

    tracing::info!(%addr, "Starting server with FULL router");

    // B3: Load previously-detected toxic wallets from database on startup
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            if let Err(e) = toxic_flow_detector.load_from_database(&pg_pool).await {
                tracing::warn!(error = %e, "Failed to load toxic wallets on startup");
            }
        }
    }

    // Rejection-rate mute: load active mutes from database on startup
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            if let Err(e) = rejection_mute_detector.load_from_database(&pg_pool).await {
                tracing::warn!(error = %e, "Failed to load muted wallets on startup");
            }
        }
    }

    // B3: Periodic toxic-wallet persistence (every 5 minutes)
    {
        let persist_detector = toxic_flow_detector.clone();
        let persist_db = db_pool.clone();
        let persist_token = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // consume first immediate tick
            loop {
                tokio::select! {
                    _ = persist_token.cancelled() => break,
                    _ = interval.tick() => {
                        use chimera_operator::db_abstraction::DbPool;
                        if let DbPool::PostgreSQL(pg_pool) = persist_db.pool() {
                            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
                            if let Err(e) = persist_detector.persist_to_database(&pg_pool, &run_id).await {
                                tracing::warn!(error = %e, "Periodic toxic wallet persist failed");
                            }
                        }
                    }
                }
            }
        });
    }

    // Rejection-rate mute: periodic persistence (every 5 minutes)
    {
        let persist_mute = rejection_mute_detector.clone();
        let persist_db = db_pool.clone();
        let persist_token = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // consume first immediate tick
            loop {
                tokio::select! {
                    _ = persist_token.cancelled() => break,
                    _ = interval.tick() => {
                        use chimera_operator::db_abstraction::DbPool;
                        if let DbPool::PostgreSQL(pg_pool) = persist_db.pool() {
                            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
                            if let Err(e) = persist_mute.persist_to_database(&pg_pool, &run_id).await {
                                tracing::warn!(error = %e, "Periodic muted-wallet persist failed");
                            }
                        }
                    }
                }
            }
        });
    }

    // Dune PnL monitor: periodic check for losing ACTIVE wallets
    if config.dune.enabled {
        let dune_monitor = dune_pnl_monitor;
        let dune_token = cancel_token.clone();
        tokio::spawn(async move {
            dune_monitor.run(dune_token).await;
        });
    }

    let shutdown_token = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, %addr, "Failed to bind server port — is it already in use?");
                return;
            }
        };
        if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                shutdown_token.cancelled().await;
            })
            .await
        {
            tracing::error!(error = %e, "Server exited with error");
        }
    });

    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("Shutdown signal received"),
        Err(err) => tracing::error!("Unable to listen for shutdown signal: {}", err),
    }

    cancel_token.cancel();
    if let Err(e) = server_handle.await {
        tracing::error!(error = %e, "Server task panicked during shutdown");
    }

    // Wait for remaining background tasks with a timeout
    for handle in task_handles {
        if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            tracing::warn!(error = %e, "Background task did not complete within 5s shutdown window");
        }
    }

    // B3: Final toxic-wallet persistence on shutdown
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
            if let Err(e) = toxic_flow_detector.persist_to_database(&pg_pool, &run_id).await {
                tracing::warn!(error = %e, "Final toxic wallet persist failed on shutdown");
            } else {
                tracing::info!("Toxic wallet state persisted on shutdown");
            }
        }
    }

    // Rejection-rate mute: final persistence on shutdown
    {
        use chimera_operator::db_abstraction::DbPool;
        if let DbPool::PostgreSQL(pg_pool) = db_pool.pool() {
            let run_id = format!("v{}", env!("CARGO_PKG_VERSION"));
            if let Err(e) = rejection_mute_detector.persist_to_database(&pg_pool, &run_id).await {
                tracing::warn!(error = %e, "Final muted-wallet persist failed on shutdown");
            }
        }
    }

    tracing::info!("Chimera Operator shut down successfully");

    Ok(())
}

/// Refresh the profitability verdict by evaluating all gates and caching the result.
async fn refresh_verdict(
    pool: &sqlx::Pool<sqlx::Postgres>,
    run_context: &Arc<chimera_operator::engine::RunContext>,
    decision_recorder: &Option<Arc<chimera_operator::engine::DecisionRecorder>>,
    total_capital_sol: f64,
) -> Result<Option<CachedVerdict>, anyhow::Error> {
    use chimera_operator::handlers::evaluate_gates;

    let run_id = run_context.run_id.clone();

    let outcomes = fetch_outcomes(pool, &run_id).await?;
    let missing_outcomes = count_missing_outcomes(pool, &run_id).await?;
    let invalid_pnl = count_invalid_pnl(pool, &run_id).await?;

    let (completeness_rate, completeness_ok) = if let Some(recorder) = decision_recorder {
        let rate = recorder.completeness();
        (rate, rate >= 0.99)
    } else {
        (1.0, true)
    };

    let (gates, verdict) = evaluate_gates(
        outcomes,
        missing_outcomes,
        invalid_pnl,
        completeness_rate,
        completeness_ok,
        total_capital_sol,
    );

    Ok(Some(CachedVerdict {
        verdict: verdict.to_string(),
        gates,
        computed_at: std::time::Instant::now(),
    }))
}

/// Build an EXIT signal from an active position entry (for stop-loss / profit-target exits)
fn build_exit_signal(pos: &ActivePositionEntry, fraction: rust_decimal::Decimal) -> Signal {
    use rust_decimal::prelude::*;
    let base_amount = if pos.entry_amount_sol.is_zero() {
        rust_decimal::Decimal::from_str("0.01").unwrap_or(rust_decimal::Decimal::ONE)
    } else {
        pos.entry_amount_sol
    };
    let amount = (base_amount * fraction)
        .max(rust_decimal::Decimal::from_str("0.001").unwrap_or(rust_decimal::Decimal::ZERO));
    let payload = SignalPayload {
        strategy: Strategy::Exit,
        token: pos.token_symbol.clone(),
        token_address: Some(pos.token_address.clone()),
        action: Action::Sell,
        amount_sol: amount,
        wallet_address: pos.wallet_address.clone(),
        trade_uuid: Some(pos.trade_uuid.clone()),
        exit_fraction: Some(fraction),
    };
    tracing::debug!(
        trade_uuid = %pos.trade_uuid,
        token = %pos.token_address,
        token_symbol = %pos.token_symbol,
        wallet = %pos.wallet_address,
        action = ?payload.action,
        side = "SELL",
        amount_sol = %amount,
        exit_fraction = ?fraction,
        price_basis = %pos.entry_price,
        "EXIT signal built (managed exit)"
    );
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
}

/// Build an exit signal for an absolute SOL amount.
/// Unlike `build_exit_signal` (which takes a fraction of the original position),
/// this takes an explicit SOL amount — eliminating the oversell bug where
/// the prior `ExitPercent` was applied against the original entry instead of the remaining balance.
/// The `exit_fraction` is computed as amount_sol / entry_amount_sol so the engine's
/// `close_position` (which multiplies exit_fraction by entry_amount) produces the correct amount.
fn build_exit_signal_amount(
    pos: &ActivePositionEntry,
    amount_sol: rust_decimal::Decimal,
) -> Signal {
    use rust_decimal::prelude::*;
    let amount = amount_sol
        .max(rust_decimal::Decimal::from_str("0.001").unwrap_or(rust_decimal::Decimal::ZERO));
    let base = if pos.entry_amount_sol.is_zero() {
        rust_decimal::Decimal::from_str("0.01").unwrap_or(rust_decimal::Decimal::ONE)
    } else {
        pos.entry_amount_sol
    };
    let fraction = if !base.is_zero() {
        (amount / base).min(Decimal::ONE)
    } else {
        Decimal::ONE
    };
    let payload = SignalPayload {
        strategy: Strategy::Exit,
        token: pos.token_symbol.clone(),
        token_address: Some(pos.token_address.clone()),
        action: Action::Sell,
        amount_sol: amount,
        wallet_address: pos.wallet_address.clone(),
        trade_uuid: Some(pos.trade_uuid.clone()),
        exit_fraction: Some(fraction),
    };
    tracing::debug!(
        trade_uuid = %pos.trade_uuid,
        token = %pos.token_address,
        token_symbol = %pos.token_symbol,
        wallet = %pos.wallet_address,
        action = ?payload.action,
        side = "SELL",
        amount_sol = %amount,
        exit_fraction = ?fraction,
        price_basis = %pos.entry_price,
        "EXIT signal built (managed exit by amount)"
    );
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
}

/// Initialize tracing/logging with rotating file output. Falls back to stderr
/// when the log directory is not writable (dev environments without /app/data/logs).
///
/// Logs are written to the host-mounted volume `./data/logs` (docker-compose
/// binds `./data:/app/data`), so they never accumulate inside the container's
/// writable layer. Daily rotation (operator.log.YYYY-MM-DD) keeps disk usage
/// bounded and — unlike the previous single-file `File::create` — never
/// truncates the log on restart.
fn init_tracing() {
    // Ensure log directory exists (the production container mount).
    let log_dir = std::env::var("CHIMERA_LOG_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/app/data/logs".into());
    std::fs::create_dir_all(&log_dir).ok();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "chimera_operator=debug,tower_http=debug".into());

    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("operator.log")
        .max_log_files(14)
        .build(&log_dir);

    match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            // Keep the writer thread alive for the process lifetime — dropping
            // the guard would shut it down and silently stop file logging.
            std::mem::forget(guard);
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().json().with_writer(writer))
                .init();
            tracing::info!(
                log_dir = %log_dir,
                "File logging configured (daily rotation): {}/operator.log.YYYY-MM-DD",
                log_dir
            );
        }
        Err(e) => {
            eprintln!(
                "WARN: cannot create log dir {} ({}): falling back to stderr",
                log_dir, e
            );
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(std::io::stderr),
                )
                .init();
        }
    }
}

/// Load and validate configuration
fn load_config() -> anyhow::Result<AppConfig> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Hard-fail if dev mode is active in a production environment. CHIMERA_ENV=production
    // must not coexist with CHIMERA_DEV_MODE — the latter skips token safety and config
    // validation, creating a silent security bypass that is hard to detect post-deploy.
    if chimera_operator::utils::is_dev_mode()
        && std::env::var("CHIMERA_ENV").as_deref() == Ok("production")
    {
        return Err(anyhow::anyhow!(
            "CHIMERA_DEV_MODE is set in a production environment (CHIMERA_ENV=production). \
             Unset CHIMERA_DEV_MODE before deploying to production."
        ));
    }

    let config = AppConfig::load_config().map_err(|e| {
        tracing::error!(error = %e, "Failed to load configuration");
        anyhow::anyhow!("Configuration error: {}", e)
    })?;

    // Validate configuration
    if let Err(e) = config.validate() {
        // In development, allow missing webhook secret
        if chimera_operator::utils::is_dev_mode() {
            tracing::warn!("Running in dev mode - skipping configuration validation");
        } else {
            return Err(anyhow::anyhow!("Configuration validation failed: {}", e));
        }
    }

    // Security warning for dangerous honeypot detection setting
    if config.token_safety.allow_unlisted_heuristic {
        tracing::error!(
            "⚠️  SECURITY RISK: allow_unlisted_heuristic is ENABLED. This bypasses DexScreener/Jupiter validation and may lead to trading honeypots. Set allow_unlisted_heuristic: false in config.yaml for production safety."
        );
    }

    Ok(config)
}

/// Generate daily trading summary from database
async fn generate_daily_summary(
    db: &dyn db_abstraction::Database,
) -> anyhow::Result<(rust_decimal::Decimal, u32, f64)> {
    // Get yesterday's date range
    let now = Utc::now();
    let yesterday_start = (now - chrono::Duration::days(1))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();
    let yesterday_end = (now - chrono::Duration::days(1))
        .format("%Y-%m-%dT23:59:59Z")
        .to_string();

    // Query trades from yesterday
    let trades = db
        .get_trades_filtered(
            Some(&yesterday_start),
            Some(&yesterday_end),
            Some("CLOSED"),
            None,
            None, // No wallet_address filter for daily summary
            1000,
            0,
        )
        .await?;

    if trades.is_empty() {
        return Ok((rust_decimal::Decimal::ZERO, 0, 0.0));
    }

    let trade_count = trades.len() as u32;
    let mut total_pnl_usd = rust_decimal::Decimal::ZERO;
    let mut winning_trades = 0u32;

    for trade in &trades {
        if let Some(pnl_usd) = trade.pnl_usd {
            total_pnl_usd += pnl_usd;
            if pnl_usd > rust_decimal::Decimal::ZERO {
                winning_trades += 1;
            }
        }
    }

    let win_rate = if trade_count > 0 {
        (winning_trades as f64 / trade_count as f64) * 100.0
    } else {
        0.0
    };

    Ok((total_pnl_usd, trade_count, win_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_jwt_secret_too_short() {
        let result = validate_jwt_secret("short");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_validate_jwt_secret_non_hex() {
        let result = validate_jwt_secret(&"g".repeat(64));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("hexadecimal"));
    }

    #[test]
    fn test_validate_jwt_secret_weak_pattern() {
        let result = validate_jwt_secret(&"0".repeat(64));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("weak pattern"));
    }

    #[test]
    fn test_validate_jwt_secret_valid() {
        let secret = "1a2b3c4d5e6f78901a2b3c4d5e6f78901a2b3c4d5e6f78901a2b3c4d5e6f7890";
        let result = validate_jwt_secret(secret);
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_jwt_secret() {
        use crate::tools::generate_jwt_secret::generate_jwt_secret;
        let secret = generate_jwt_secret().unwrap();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(validate_jwt_secret(&secret).is_ok());
    }

    #[test]
    fn test_version() {
        // Ensure version is set
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }

    #[test]
    fn test_auto_promote_config_defaults() {
        // Defaults must be safe: disabled unless explicitly opted in, sensible
        // WQS floor (60 = "regular" conviction), 7-day TTL.
        let m = chimera_operator::config::MonitoringConfig::default();
        assert!(!m.auto_promote_enabled, "auto_promote must default to false");
        assert_eq!(m.auto_promote_min_wqs, 60.0);
        assert_eq!(m.auto_promote_ttl_hours, 168);
        assert_eq!(m.auto_promote_max_age_days, 7);
        assert_eq!(m.max_active_wallets, 20);
    }

    #[test]
    fn test_wallet_boost_config_defaults() {
        let m = chimera_operator::config::MonitoringConfig::default();
        assert!(!m.wallet_boost_enabled, "wallet_boost must default to false");
        assert_eq!(m.wallet_boost_min_sample, 15);
        assert_eq!(m.wallet_boost_window_trades, 20);
        assert_eq!(m.wallet_boost_window_days, 30);
        assert_eq!(m.wallet_boost_min_winrate, 0.40);
        assert_eq!(m.wallet_boost_recency_days, 7);
        assert_eq!(m.wallet_boost_size_sol, rust_decimal::Decimal::new(50, 2)); // 0.50
        assert_eq!(m.wallet_boost_min_net_sol, rust_decimal::Decimal::new(1, 2)); // 0.01
    }

    #[test]
    fn test_wick_protection_max_loss_default() {
        // The large-loss wick override must default to -10% so fast pump.fun
        // dumps in the first 60s don't ride unprotected to -12%..-14%.
        let pm = chimera_operator::config::ProfitManagementConfig::default();
        assert_eq!(
            pm.wick_protection_max_loss_percent,
            rust_decimal::Decimal::new(-100, 1) // -10.0
        );
    }
}
