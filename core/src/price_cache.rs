//! Price Cache for real-time token price tracking
//!
//! Provides cached token prices for:
//! - Unrealized PnL calculations (circuit breaker)
//! - Position value display
//! - Drawdown calculations
//!
//! Uses Jupiter Price API for price fetching.
//! Cache refresh interval: 5 seconds for active positions.

use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::time::interval;

/// Default cache TTL in seconds.
/// Must be >= STALENESS_THRESHOLD_SECS: `get_price_usd` checks both the
/// staleness flag AND the TTL, so a TTL shorter than the staleness window
/// would expire prices before the staleness guard gets a chance to extend them.
const DEFAULT_CACHE_TTL_SECS: i64 = 90;

/// Price update interval for active tokens.
/// Deliberately shorter than the staleness threshold so a single rate-limited
/// or skipped cycle never blinds the exit system. At 15s this stays well under
/// the Jupiter free-tier budget (60 req/min shared bucket) while keeping the
/// effective cadence >= 3x the staleness window even when a cycle is skipped.
const PRICE_UPDATE_INTERVAL_SECS: u64 = 15;

/// Decimals cache TTL in seconds (24 hours - decimals are immutable for minted tokens)
const DECIMALS_TTL_SECS: i64 = 86400;

/// Staleness threshold in seconds: if a token's cached price is older than this
/// window, it is considered stale and `get_price_usd` returns None.
/// MUST be strictly greater than `PRICE_UPDATE_INTERVAL_SECS`. When both were
/// 30s, any rate-limited / skipped cycle made prices stale exactly when the next
/// fetch was due, so the position monitor ran blind for 30-60s after entry —
/// the first price it saw was already 5-8% below entry, triggering guaranteed
/// stop-loss/momentum exits. 90s covers 3+ update intervals with margin while
/// still bounding how old a price can be before risk checks refuse it.
pub const STALENESS_THRESHOLD_SECS: i64 = 90;

/// Upper age bound for the `get_sol_price_usd_fallback` path: an expired SOL
/// price older than this is refused even as a "last known" value so stale
/// prices are not fed into market-condition checks.
const FALLBACK_MAX_AGE_SECS: i64 = 3600;

/// Price entry in cache
#[derive(Debug, Clone)]
pub struct PriceEntry {
    /// Price in USD (using Decimal for precision)
    pub price_usd: Decimal,
    /// When this price was fetched
    pub fetched_at: DateTime<Utc>,
    /// Price source
    pub source: PriceSource,
    /// Token decimals from Jupiter (optional - not all tokens may have this)
    pub decimals: Option<u8>,
}

/// Price data source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// Jupiter Price API
    Jupiter,
    /// Pyth Oracle
    Pyth,
    /// Fallback/cached value
    Cached,
}

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jupiter => write!(f, "Jupiter"),
            Self::Pyth => write!(f, "Pyth"),
            Self::Cached => write!(f, "Cached"),
        }
    }
}

/// FIX [B-H7]: Combined inner state to allow atomic updates of prices + price_history
/// under a single lock, preventing torn reads between the two maps.
struct PriceCacheInner {
    /// Cached prices by token address
    prices: HashMap<String, PriceEntry>,
    /// Price history for volatility calculation (token -> VecDeque of (timestamp, price))
    price_history: HashMap<String, VecDeque<(DateTime<Utc>, Decimal)>>,
    /// Decimals cache from Jupiter (token -> (decimals, fetched_at))
    decimals: HashMap<String, (u8, Instant)>,
    /// Cache hit counter (for performance monitoring) — atomic so hot-path
    /// lookups can use the read lock only
    cache_hits: AtomicU64,
    /// Cache miss counter (for performance monitoring)
    cache_misses: AtomicU64,
    /// Decimals cache hit counter (for performance monitoring)
    decimals_cache_hits: AtomicU64,
    /// Decimals cache miss counter (for performance monitoring)
    decimals_cache_misses: AtomicU64,
}

/// Price cache for token prices
pub struct PriceCache {
    /// Combined inner state (prices + price_history) under one lock for atomic updates
    inner: Arc<RwLock<PriceCacheInner>>,
    /// Cache TTL
    ttl: Duration,
    /// Tokens to actively track
    active_tokens: Arc<RwLock<Vec<String>>>,
    /// Whether the updater is running
    updater_running: Arc<RwLock<bool>>,
    /// SOL mint address (for market condition filtering)
    sol_mint: String,
    /// Reusable HTTP client (FIX [R-L4]: built once, not per-fetch)
    http_client: reqwest::Client,
    /// Jupiter Price API base URL (configurable)
    jupiter_price_api_url: String,
}

impl PriceCache {
    /// Build the shared reusable HTTP client
    ///
    /// Returns an error if the client cannot be built (e.g., invalid timeout configuration).
    /// This prevents silent fallback to a default client with incorrect settings.
    fn build_http_client() -> Result<reqwest::Client, PriceCacheError> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| PriceCacheError::HttpError(format!("Failed to build HTTP client: {}", e)))
    }

    /// Create a new price cache with default TTL
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new() -> Result<Self, PriceCacheError> {
        Ok(Self {
            inner: Arc::new(RwLock::new(PriceCacheInner {
                prices: HashMap::new(),
                price_history: HashMap::new(),
                decimals: HashMap::new(),
                cache_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                decimals_cache_hits: AtomicU64::new(0),
                decimals_cache_misses: AtomicU64::new(0),
            })),
            ttl: Duration::seconds(DEFAULT_CACHE_TTL_SECS),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            updater_running: Arc::new(RwLock::new(false)),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
            http_client: Self::build_http_client()?,
            jupiter_price_api_url: "https://api.jup.ag/price".to_string(),
        })
    }

    /// Create with custom Jupiter Price API URL
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn with_jupiter_price_api(jupiter_price_api_url: String) -> Result<Self, PriceCacheError> {
        Ok(Self {
            inner: Arc::new(RwLock::new(PriceCacheInner {
                prices: HashMap::new(),
                price_history: HashMap::new(),
                decimals: HashMap::new(),
                cache_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                decimals_cache_hits: AtomicU64::new(0),
                decimals_cache_misses: AtomicU64::new(0),
            })),
            ttl: Duration::seconds(DEFAULT_CACHE_TTL_SECS),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            updater_running: Arc::new(RwLock::new(false)),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
            http_client: Self::build_http_client()?,
            jupiter_price_api_url,
        })
    }

    /// Create with custom TTL
    ///
    /// Returns an error if the HTTP client cannot be built or the TTL would
    /// violate the module invariant `ttl >= STALENESS_THRESHOLD_SECS` (a TTL
    /// below the staleness window would expire prices before the staleness
    /// guard can extend them).
    pub fn with_ttl(ttl_secs: i64) -> Result<Self, PriceCacheError> {
        if ttl_secs < STALENESS_THRESHOLD_SECS {
            return Err(PriceCacheError::HttpError(format!(
                "ttl_secs {} must be >= STALENESS_THRESHOLD_SECS ({})",
                ttl_secs, STALENESS_THRESHOLD_SECS
            )));
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(PriceCacheInner {
                prices: HashMap::new(),
                price_history: HashMap::new(),
                decimals: HashMap::new(),
                cache_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                decimals_cache_hits: AtomicU64::new(0),
                decimals_cache_misses: AtomicU64::new(0),
            })),
            ttl: Duration::seconds(ttl_secs),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            updater_running: Arc::new(RwLock::new(false)),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
            http_client: Self::build_http_client()?,
            jupiter_price_api_url: "https://api.jup.ag/price".to_string(),
        })
    }

    /// Get price for a token
    pub fn get_price(&self, token_address: &str) -> Option<PriceEntry> {
        // Read-only hot path: read lock only (counters are atomics), so
        // concurrent readers never contend with each other or with the
        // background updater's writes.
        let inner = self.inner.read();

        let entry = match inner.prices.get(token_address) {
            Some(entry) => entry.clone(),
            None => {
                inner.cache_misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        // Check if expired
        let age = Utc::now().signed_duration_since(entry.fetched_at);
        if age > self.ttl {
            inner.cache_misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        inner.cache_hits.fetch_add(1, Ordering::Relaxed);
        Some(entry)
    }

    /// Get price in USD (convenience method).
    /// FIX [R-M9]: Always check staleness even for untracked tokens — if stale, return None.
    pub fn get_price_usd(&self, token_address: &str) -> Option<Decimal> {
        if self.is_price_stale(token_address) {
            tracing::debug!(
                token = token_address,
                "get_price_usd: price is stale, returning None"
            );
            return None;
        }
        self.get_price(token_address).map(|e| e.price_usd)
    }

    /// Returns `true` if the cached price for the token has exceeded
    /// [`STALENESS_THRESHOLD_SECS`], regardless of whether the token is actively
    /// tracked. Returns `false` if the token has a recent price or has never been
    /// seen (no expectation of data).
    ///
    /// FIX [R-M9]: Previously only reported staleness for actively-tracked tokens,
    /// meaning an untracked-but-cached stale price could silently be returned.
    pub fn is_price_stale(&self, token_address: &str) -> bool {
        let inner = self.inner.read();
        match inner.prices.get(token_address) {
            Some(entry) => {
                let age = Utc::now().signed_duration_since(entry.fetched_at);
                age.num_seconds() > STALENESS_THRESHOLD_SECS
            }
            // No cached entry — not stale (just missing)
            None => false,
        }
    }

    /// Returns `true` if the token is actively tracked but has not received a
    /// fresh price within [`STALENESS_THRESHOLD_SECS`].
    pub fn is_tracked_price_stale(&self, token_address: &str) -> bool {
        // If we're not actively tracking this token, we have no expectation
        // of fresh data — don't report staleness.
        let is_tracked = self.active_tokens.read().iter().any(|t| t == token_address);
        if !is_tracked {
            return false;
        }
        self.is_price_stale(token_address)
    }

    /// Set price for a token.
    /// FIX [B-H7]: Updates both prices and price_history atomically under one lock.
    pub fn set_price(
        &self,
        token_address: &str,
        price_usd: Decimal,
        source: PriceSource,
        decimals: Option<u8>,
    ) {
        let now = Utc::now();
        // Acquire a single write lock and update both maps atomically.
        let mut inner = self.inner.write();
        inner.prices.insert(
            token_address.to_string(),
            PriceEntry {
                price_usd,
                fetched_at: now,
                source,
                decimals,
            },
        );

        // Update price history for volatility calculation (keep last 24 hours)
        let token_history = inner
            .price_history
            .entry(token_address.to_string())
            .or_default();
        token_history.push_back((now, price_usd));

        // Keep only last 24 hours (assuming updates every 5 seconds = ~17,280 entries max)
        let cutoff = now - Duration::hours(24);
        while let Some(front) = token_history.front() {
            if front.0 < cutoff {
                token_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Set price for a token with a custom timestamp (test only).
    #[cfg(feature = "test-utils")]
    pub fn set_price_with_time(
        &self,
        token_address: &str,
        price_usd: Decimal,
        source: PriceSource,
        time: DateTime<Utc>,
        decimals: Option<u8>,
    ) {
        let mut inner = self.inner.write();
        inner.prices.insert(
            token_address.to_string(),
            PriceEntry {
                price_usd,
                fetched_at: time,
                source,
                decimals,
            },
        );

        let token_history = inner
            .price_history
            .entry(token_address.to_string())
            .or_default();
        token_history.push_back((time, price_usd));

        let cutoff = time - Duration::hours(24);
        while let Some(front) = token_history.front() {
            if front.0 < cutoff {
                token_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate volatility for a token (24h window)
    ///
    /// Returns volatility as percentage (0.0-100.0)
    /// Returns None if insufficient data (< 2 price points)
    pub fn calculate_volatility(&self, token_address: &str) -> Option<f64> {
        let inner = self.inner.read();
        let token_history = inner.price_history.get(token_address)?;

        if token_history.len() < 2 {
            return None;
        }

        // Calculate price changes using Decimal for precision
        let prices: Vec<Decimal> = token_history.iter().map(|(_, price)| *price).collect();
        let mut price_changes = Vec::new();

        for i in 1..prices.len() {
            if prices[i - 1] > Decimal::ZERO {
                let change = ((prices[i] - prices[i - 1]) / prices[i - 1]) * Decimal::from(100);
                price_changes.push(change);
            }
        }

        if price_changes.is_empty() {
            return None;
        }

        // Calculate mean using Decimal
        let sum: Decimal = price_changes.iter().sum();
        let count = Decimal::from(price_changes.len());
        let mean = sum / count;

        // Calculate standard deviation using Decimal
        let variance: Decimal = price_changes
            .iter()
            .map(|x| {
                let diff = *x - mean;
                diff * diff
            })
            .sum::<Decimal>()
            / count;

        // Convert to f64 for sqrt (volatility is a statistical metric, not a financial amount)
        let variance_f64 = variance.to_f64().unwrap_or(0.0);
        let std_dev = variance_f64.sqrt();

        // Return absolute volatility (as percentage)
        Some(std_dev.abs())
    }

    /// Get SOL price in USD
    pub fn get_sol_price_usd(&self) -> Option<Decimal> {
        self.get_price_usd(&self.sol_mint)
    }

    /// Get last known non-zero SOL price in USD, even if the primary entry is expired.
    ///
    /// Guards: never returns a zero price (a zero would be treated as a valid
    /// SOL price by risk logic), and bounds the age to
    /// [`FALLBACK_MAX_AGE_SECS`] so an hours-old price is not fed into
    /// market-condition checks.
    pub fn get_sol_price_usd_fallback(&self) -> Option<Decimal> {
        if let Some(price) = self.get_sol_price_usd() {
            if !price.is_zero() {
                return Some(price);
            }
        }
        let inner = self.inner.read();
        let entry = inner.prices.get(&self.sol_mint)?;
        if entry.price_usd.is_zero() {
            return None;
        }
        let age = Utc::now().signed_duration_since(entry.fetched_at);
        if age.num_seconds() > FALLBACK_MAX_AGE_SECS {
            return None;
        }
        Some(entry.price_usd)
    }

    /// Get SOL price volatility (for market condition filtering)
    pub fn get_sol_volatility(&self) -> Option<f64> {
        self.calculate_volatility(&self.sol_mint)
    }

    /// Get token decimals from Jupiter cache.
    /// Returns None if token not in cache or cache entry expired.
    pub fn get_decimals(&self, token_address: &str) -> Option<u8> {
        // Lookup path: read lock for the lookup + fallback; the write lock is
        // reserved for the expired-entry removal and counter updates (atomics).
        {
            let inner = self.inner.read();

            // Check decimals cache first
            if let Some((decimals, fetched_at)) = inner.decimals.get(token_address) {
                let elapsed = fetched_at.elapsed().as_secs() as i64;
                if elapsed < DECIMALS_TTL_SECS {
                    inner.decimals_cache_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(*decimals);
                }
            }

            // Fallback: check if we have it in a recent price entry
            if let Some(decimals) = inner.prices.get(token_address).and_then(|e| e.decimals) {
                inner.decimals_cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(decimals);
            }
        }

        // Cache expired — remove the entry (best effort) and record a miss.
        let mut inner = self.inner.write();
        inner.decimals.remove(token_address);
        inner.decimals_cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Add token to active tracking
    pub fn track_token(&self, token_address: &str) {
        let mut tokens = self.active_tokens.write();
        if !tokens.contains(&token_address.to_string()) {
            tokens.push(token_address.to_string());
            tracing::debug!(token = token_address, "Added token to price tracking");
        }
    }

    /// Eagerly fetch a fresh price for a single token and write it into the cache.
    ///
    /// Called right after a position opens so the position monitor has a live
    /// price from second 0 instead of waiting up to `PRICE_UPDATE_INTERVAL_SECS`
    /// for the next background cycle. Without this, the first price the monitor
    /// sees can already be 5-8% below entry (pump tokens dump within seconds),
    /// which is exactly what triggered the guaranteed-loss exits observed in logs.
    pub async fn eager_fetch_token(&self, token_address: &str) {
        // If we already have a fresh price, nothing to do.
        if self.get_price_usd(token_address).is_some() {
            return;
        }
        let tokens = vec![token_address.to_string()];
        match self.fetch_prices_jupiter(&tokens, None).await {
            Ok((prices, decimals_map)) if !prices.is_empty() => {
                let _ = self.apply_price_updates(prices, decimals_map);
            }
            Ok(_) => {
                tracing::warn!(token = token_address, "Eager price fetch returned 0 prices");
            }
            Err(PriceCacheError::RateLimited) => {
                // Primary API is rate-limited (position opens coincide with
                // trading bursts that consume the shared Jupiter bucket) — fall
                // back to lite-api so the position monitor is not left blind.
                let lite_url = "https://lite-api.jup.ag/price";
                match self.fetch_prices_jupiter(&tokens, Some(lite_url)).await {
                    Ok((prices, decimals_map)) if !prices.is_empty() => {
                        tracing::debug!(
                            token = token_address,
                            "Eager fetch served by lite-api fallback"
                        );
                        let _ = self.apply_price_updates(prices, decimals_map);
                    }
                    Ok(_) => {
                        tracing::warn!(
                            token = token_address,
                            "Eager price fetch fallback returned 0 prices"
                        );
                    }
                    Err(fallback_err) => {
                        tracing::warn!(token = token_address, error = %fallback_err, "Eager price fetch fallback failed");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(token = token_address, error = %e, "Eager price fetch failed");
            }
        }
    }

    /// Force-refresh a single token's price (bypassing the "already cached"
    /// short-circuit in `eager_fetch_token`) and return the fresh USD price.
    ///
    /// Used to validate marks before stop-loss exits (2026-08-08): a single
    /// bad price observation must not stop a position. Returns None when the
    /// fetch failed (both primary and lite-api fallback) — callers keep the
    /// cached mark and may still exit, staying fail-safe.
    pub async fn refresh_price_usd(&self, token_address: &str) -> Option<Decimal> {
        let tokens = vec![token_address.to_string()];
        let fetch = async {
            match self.fetch_prices_jupiter(&tokens, None).await {
                Ok((prices, decimals_map)) if !prices.is_empty() => {
                    let _ = self.apply_price_updates(prices, decimals_map);
                }
                Ok(_) => {
                    tracing::warn!(
                        token = token_address,
                        "Refresh price fetch returned 0 prices"
                    );
                }
                Err(PriceCacheError::RateLimited) => {
                    let lite_url = "https://lite-api.jup.ag/price";
                    match self.fetch_prices_jupiter(&tokens, Some(lite_url)).await {
                        Ok((prices, decimals_map)) if !prices.is_empty() => {
                            let _ = self.apply_price_updates(prices, decimals_map);
                        }
                        Ok(_) => {
                            tracing::warn!(
                                token = token_address,
                                "Refresh price fetch fallback returned 0 prices"
                            );
                        }
                        Err(fallback_err) => {
                            tracing::warn!(
                                token = token_address,
                                error = %fallback_err,
                                "Refresh price fetch fallback failed"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        token = token_address,
                        error = %e,
                        "Refresh price fetch failed"
                    );
                }
            }
        };
        fetch.await;
        self.get_price_usd(token_address)
    }

    /// Remove token from active tracking
    pub fn untrack_token(&self, token_address: &str) {
        let mut tokens = self.active_tokens.write();
        tokens.retain(|t| t != token_address);
    }

    /// Get list of tracked tokens
    pub fn tracked_tokens(&self) -> Vec<String> {
        self.active_tokens.read().clone()
    }

    /// Start the background price updater with supervision.
    /// FIX [B-H8]: If the inner update loop panics, the supervisor restarts it after 1s.
    pub async fn start_updater(self: Arc<Self>) {
        {
            let mut running = self.updater_running.write();
            if *running {
                tracing::warn!("Price updater already running");
                return;
            }
            *running = true;
        }

        tracing::info!(
            interval_secs = PRICE_UPDATE_INTERVAL_SECS,
            "Starting supervised price cache updater"
        );

        // Supervisor loop: respawn the inner update task if it panics.
        loop {
            let cache_clone = Arc::clone(&self);
            let result = tokio::spawn(async move {
                cache_clone.run_price_update_loop().await;
            })
            .await;
            if let Err(e) = result {
                tracing::error!("Price updater panicked, restarting: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            } else {
                // run_price_update_loop returned normally (e.g. on shutdown) — exit supervisor.
                break;
            }
        }
        // Supervisor exited: the updater is no longer running, so a later
        // start_updater call can start a fresh one. (In the panic-restart path
        // the flag intentionally stays set while the supervisor re-runs the loop.)
        *self.updater_running.write() = false;
    }

    /// Inner price update loop (runs until cancellation or panic).
    async fn run_price_update_loop(&self) {
        let mut update_interval =
            interval(std::time::Duration::from_secs(PRICE_UPDATE_INTERVAL_SECS));

        let mut rate_limit_cooldown = false;

        loop {
            update_interval.tick().await;

            let tokens = self.active_tokens.read().clone();
            if tokens.is_empty() {
                continue;
            }

            // After a 429, skip one tick to let the API rate-limit window reset.
            if rate_limit_cooldown {
                rate_limit_cooldown = false;
                tracing::debug!("Skipping price update (rate-limit cooldown)");
                continue;
            }

            match self.update_prices(&tokens).await {
                Err(PriceCacheError::RateLimited) => {
                    // Try lite-api fallback so the cache doesn't go stale during cooldown.
                    // Only skip the next cycle if BOTH the primary AND the fallback fail:
                    // the cooldown exists to stop hammering a rate-limited endpoint, but
                    // if lite-api delivered fresh prices we have no reason to stall a cycle.
                    let lite_url = "https://lite-api.jup.ag/price";
                    match self.fetch_prices_jupiter(&tokens, Some(lite_url)).await {
                        Ok((prices, decimals_map)) if !prices.is_empty() => {
                            tracing::info!(
                                "Lite-api fallback returned {} prices after rate-limit",
                                prices.len()
                            );
                            let _ = self.apply_price_updates(prices, decimals_map);
                        }
                        Ok((_, _)) => {
                            tracing::warn!(
                                "Lite-api fallback returned 0 prices during rate-limit cooldown"
                            );
                            rate_limit_cooldown = true;
                        }
                        Err(fallback_err) => {
                            tracing::warn!(
                                error = %fallback_err,
                                "Lite-api fallback unavailable during rate-limit cooldown"
                            );
                            rate_limit_cooldown = true;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to update prices");
                }
                Ok(_) => {}
            }
        }
    }

    /// One-time eager fetch of currently-tracked token prices.
    /// Call at startup before spawning background tasks that depend on prices
    /// (e.g. circuit-breaker USD checks) to avoid the startup race where the
    /// first evaluation runs before the background updater has fetched anything.
    pub async fn prime_prices(&self) -> Result<(), PriceCacheError> {
        let tokens = self.active_tokens.read().clone();
        if tokens.is_empty() {
            return Ok(());
        }
        self.update_prices(&tokens).await
    }

    /// Update prices for a list of tokens
    async fn update_prices(&self, tokens: &[String]) -> Result<(), PriceCacheError> {
        let mut last_err: Option<PriceCacheError> = None;
        for attempt in 0..3 {
            match self.fetch_prices_jupiter(tokens, None).await {
                Ok((prices, decimals_map)) => {
                    // If we got 0 prices but requested >0, retry with lite-api fallback
                    if prices.is_empty() && !tokens.is_empty() && attempt == 0 {
                        let lite_url = "https://lite-api.jup.ag/price";
                        tracing::warn!(
                            "Primary Jupiter price API returned 0 prices, retrying with lite-api fallback"
                        );
                        match self.fetch_prices_jupiter(tokens, Some(lite_url)).await {
                            Ok((fallback_prices, fallback_decimals_map)) => {
                                tracing::info!(
                                    "Lite-api fallback returned {} prices",
                                    fallback_prices.len()
                                );
                                return self
                                    .apply_price_updates(fallback_prices, fallback_decimals_map);
                            }
                            Err(fallback_err) => {
                                tracing::warn!(error = %fallback_err, "Lite-api fallback failed, continuing retries");
                                // Continue with retry loop
                            }
                        }
                    }

                    return self.apply_price_updates(prices, decimals_map);
                }
                Err(e) => {
                    if matches!(e, PriceCacheError::HttpError(_)) && attempt < 2 {
                        let delay = [250, 500][attempt];
                        tracing::warn!(
                            attempt = attempt + 1,
                            delay_ms = delay,
                            error = %e,
                            "Jupiter price fetch failed, retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    } else {
                        // On final HTTP failure, try lite-api fallback before giving up
                        if matches!(e, PriceCacheError::HttpError(_)) {
                            tracing::warn!(error = %e, "Primary Jupiter API exhausted, trying lite-api fallback");
                            let lite_url = "https://lite-api.jup.ag/price";
                            match self.fetch_prices_jupiter(tokens, Some(lite_url)).await {
                                Ok((fallback_prices, fallback_decimals_map)) => {
                                    tracing::info!(
                                        "Lite-api fallback returned {} prices after HTTP error",
                                        fallback_prices.len()
                                    );
                                    return self.apply_price_updates(
                                        fallback_prices,
                                        fallback_decimals_map,
                                    );
                                }
                                Err(fallback_err) => {
                                    tracing::warn!(error = %fallback_err, "Lite-api fallback also failed");
                                }
                            }
                        }
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| PriceCacheError::HttpError("unknown error".into())))
    }

    /// Apply price updates from a successful fetch
    fn apply_price_updates(
        &self,
        prices: Vec<(String, Decimal, Option<u8>)>,
        decimals_map: HashMap<String, (u8, u64)>,
    ) -> Result<(), PriceCacheError> {
        let price_count = prices.len();
        for (token, price, decimals) in prices {
            self.set_price(&token, price, PriceSource::Jupiter, decimals);
        }

        if !decimals_map.is_empty() {
            let mut inner = self.inner.write();
            for (token, (decimals, _)) in decimals_map {
                inner
                    .decimals
                    .insert(token, (decimals, std::time::Instant::now()));
            }
        }

        tracing::trace!(token_count = price_count, "Updated prices");
        Ok(())
    }

    /// Fetch prices from Jupiter Price API.
    /// FIX [R-L4]: Uses the reusable `self.http_client` rather than rebuilding on every call.
    /// Returns (prices_with_decimals, decimals_map) where decimals_map maps token -> (decimals, block_id)
    /// If `url_override` is provided, uses that URL instead of `self.jupiter_price_api_url`.
    async fn fetch_prices_jupiter(
        &self,
        tokens: &[String],
        url_override: Option<&str>,
    ) -> Result<
        (
            Vec<(String, Decimal, Option<u8>)>,
            HashMap<String, (u8, u64)>,
        ),
        PriceCacheError,
    > {
        if tokens.is_empty() {
            return Ok((Vec::new(), HashMap::new()));
        }

        // Build URL with comma-separated token addresses
        let token_list = tokens.join(",");
        let url = format!(
            "{}/v3?ids={}",
            url_override.unwrap_or(self.jupiter_price_api_url.trim_end_matches('/')),
            token_list
        );

        tracing::trace!(
            token_count = tokens.len(),
            url = %url,
            "Fetching prices from Jupiter"
        );

        // Reuse the pre-built HTTP client stored in self.
        let response = crate::jupiter::with_api_key(self.http_client.get(&url))
            .send()
            .await
            .map_err(|e| {
                PriceCacheError::HttpError(format!("Jupiter price request failed: {}", e))
            })?;

        // Check for rate limiting
        if response.status() == 429 {
            return Err(PriceCacheError::RateLimited);
        }

        if !response.status().is_success() {
            return Err(PriceCacheError::HttpError(format!(
                "Jupiter API returned error: {}",
                response.status()
            )));
        }

        // Parse JSON response
        let data: JupiterPriceResponse = response.json().await.map_err(|e| {
            PriceCacheError::ParseError(format!("Failed to parse Jupiter response: {}", e))
        })?;

        // Extract prices from response and convert to Decimal
        let mut results = Vec::new();
        let mut decimals_map = HashMap::new();
        for token in tokens {
            if let Some(value) = data.data.get(token) {
                // Per-token parse: an unparseable entry only skips that token
                // (and is logged) instead of failing the whole batch.
                let price_data: JupiterPriceData = match serde_json::from_value(value.clone()) {
                    Ok(pd) => pd,
                    Err(e) => {
                        tracing::warn!(token = token, error = %e, "Token entry in Jupiter price response failed to parse — skipping");
                        continue;
                    }
                };
                // Jupiter omits `usdPrice` for tokens with no recent trade
                // (dead/untradeable — liquidity < ~$10). That is a normal
                // absence, not a parse failure: skip quietly instead of
                // spamming the log (2026-08-09: 3k+ warnings/day from
                // stale shadow-tracked tokens).
                let Some(usd_price) = price_data.usdPrice else {
                    tracing::debug!(
                        token = token,
                        "No tradeable price for token (dead/untradeable) — skipping"
                    );
                    continue;
                };
                // Jupiter returns price in USD as f64, convert to Decimal for precision
                // Try from_f64_retain first for best precision, fall back to string conversion
                let price = match Decimal::from_f64_retain(usd_price) {
                    Some(decimal) => decimal,
                    None => {
                        // Fallback: string conversion handles edge cases where from_f64_retain fails
                        match Decimal::from_str(&usd_price.to_string()) {
                            Ok(decimal) => decimal,
                            Err(_) => {
                                tracing::error!(
                                    token = token,
                                    price_f64 = usd_price,
                                    "Failed to convert Jupiter price to Decimal — both from_f64_retain and from_str failed"
                                );
                                // Skip this token rather than using a zero price
                                continue;
                            }
                        }
                    }
                };
                // Store decimals for separate cache
                decimals_map.insert(
                    token.clone(),
                    (
                        price_data.decimals.unwrap_or(9),
                        price_data.blockId.unwrap_or_default(),
                    ),
                );
                results.push((token.clone(), price, Some(price_data.decimals.unwrap_or(9))));
            } else {
                tracing::warn!(token = token, "Token not found in Jupiter price response");
                // UNTRACK absent tokens (2026-08-06): Jupiter's price API
                // doesn't list new/small tokens. Re-querying them every 5s
                // cycle burned the keyless rate quota (53K 'not found' lines
                // + 40K eager-fetch failures per day) — starving the
                // honeypot check at execution time (429 -> fail-closed ->
                // every admitted signal dead-lettered). The token can be
                // re-tracked if it later appears (e.g. a new signal).
                {
                    let mut tokens = self.active_tokens.write();
                    tokens.retain(|t| t != token);
                }
                // Skip tokens not found in response
            }
        }

        tracing::trace!(
            fetched_count = results.len(),
            total_requested = tokens.len(),
            "Fetched prices from Jupiter"
        );

        // If we got 0 prices but requested >0, report it so the caller can
        // retry/fall back instead of silently letting the cache go stale.
        // NOTE: this is normal for dead/untradeable tokens (Jupiter omits
        // usdPrice) — the message must not blame the API key.
        if results.is_empty() && !tokens.is_empty() {
            tracing::warn!(
                token_count = tokens.len(),
                url = %url,
                "Jupiter price API returned 0 prices for {} requested tokens — \
                 all lacked a tradeable price (dead/untradeable) or failed to parse",
                tokens.len()
            );
            return Err(PriceCacheError::HttpError(format!(
                "Jupiter price API returned 0 prices for {} requested tokens",
                tokens.len()
            )));
        }

        Ok((results, decimals_map))
    }

    /// Calculate unrealized PnL for a position
    /// Uses Decimal for precision to avoid floating point errors
    pub fn calculate_unrealized_pnl(
        &self,
        token_address: &str,
        entry_price: Decimal,
        position_size: Decimal,
    ) -> Option<UnrealizedPnL> {
        let current_price_dec = self.get_price_usd(token_address)?;

        // Use Decimal for precise calculations
        let pnl_usd = if !entry_price.is_zero() {
            let price_diff = current_price_dec - entry_price;
            price_diff * position_size
        } else {
            Decimal::ZERO
        };

        let pnl_percent = if !entry_price.is_zero() {
            let price_diff = current_price_dec - entry_price;
            let ratio = price_diff / entry_price;
            ratio * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        Some(UnrealizedPnL {
            current_price: current_price_dec,
            entry_price,
            pnl_usd,
            pnl_percent,
        })
    }

    /// Get cache statistics
    pub fn stats(&self) -> PriceCacheStats {
        let inner = self.inner.read();
        let now = Utc::now();

        let mut valid_count = 0;
        let mut stale_count = 0;

        for entry in inner.prices.values() {
            let age = now.signed_duration_since(entry.fetched_at);
            if age <= self.ttl {
                valid_count += 1;
            } else {
                stale_count += 1;
            }
        }

        let total_requests =
            inner.cache_hits.load(Ordering::Relaxed) + inner.cache_misses.load(Ordering::Relaxed);
        let hit_rate = if total_requests > 0 {
            (inner.cache_hits.load(Ordering::Relaxed) as f64 / total_requests as f64) * 100.0
        } else {
            0.0
        };

        let miss_rate = if total_requests > 0 {
            (inner.cache_misses.load(Ordering::Relaxed) as f64 / total_requests as f64) * 100.0
        } else {
            0.0
        };

        PriceCacheStats {
            total_entries: inner.prices.len(),
            valid_entries: valid_count,
            stale_entries: stale_count,
            tracked_tokens: self.active_tokens.read().len(),
            total_hits: inner.cache_hits.load(Ordering::Relaxed),
            total_misses: inner.cache_misses.load(Ordering::Relaxed),
            hit_rate,
            miss_rate,
            decimals_cache_entries: inner.decimals.len(),
            decimals_cache_hits: inner.decimals_cache_hits.load(Ordering::Relaxed),
            decimals_cache_misses: inner.decimals_cache_misses.load(Ordering::Relaxed),
        }
    }

    /// Clear expired entries
    pub fn prune_expired(&self) {
        let mut inner = self.inner.write();
        let now = Utc::now();

        inner.prices.retain(|_, entry| {
            let age = now.signed_duration_since(entry.fetched_at);
            age <= self.ttl
        });
    }

    /// Read the price history map under a lock.
    /// Returns a guard that derefs to `HashMap<String, VecDeque<(DateTime<Utc>, Decimal)>>`.
    /// Used by engine modules that need read access to price history for volatility
    /// or momentum calculations. The returned guard holds the inner lock — callers
    /// must not call any other `&self` method while holding it (would deadlock).
    pub fn price_history_read(&self) -> PriceHistoryReadGuard<'_> {
        PriceHistoryReadGuard {
            guard: self.inner.read(),
        }
    }
}

/// Read guard for the price history map, exposing HashMap<String, VecDeque<...>> via Deref.
pub struct PriceHistoryReadGuard<'a> {
    guard: parking_lot::RwLockReadGuard<'a, PriceCacheInner>,
}

impl<'a> std::ops::Deref for PriceHistoryReadGuard<'a> {
    type Target = HashMap<String, VecDeque<(DateTime<Utc>, Decimal)>>;
    fn deref(&self) -> &Self::Target {
        &self.guard.price_history
    }
}

/// Unrealized PnL calculation result
#[derive(Debug, Clone)]
pub struct UnrealizedPnL {
    /// Current price
    pub current_price: Decimal,
    /// Entry price
    pub entry_price: Decimal,
    /// PnL in USD
    pub pnl_usd: Decimal,
    /// PnL as percentage
    pub pnl_percent: Decimal,
}

/// Price cache statistics
#[derive(Debug, Clone)]
pub struct PriceCacheStats {
    /// Total entries in cache
    pub total_entries: usize,
    /// Valid (non-expired) entries
    pub valid_entries: usize,
    /// Stale (expired) entries
    pub stale_entries: usize,
    /// Number of actively tracked tokens
    pub tracked_tokens: usize,
    /// Total cache hits (successful lookups)
    pub total_hits: u64,
    /// Total cache misses (failed lookups)
    pub total_misses: u64,
    /// Cache hit rate percentage
    pub hit_rate: f64,
    /// Cache miss rate percentage
    pub miss_rate: f64,
    /// Total decimals cache entries
    pub decimals_cache_entries: usize,
    /// Decimals cache hits (successful lookups)
    pub decimals_cache_hits: u64,
    /// Decimals cache misses (failed lookups)
    pub decimals_cache_misses: u64,
}

/// Jupiter Price API V3 response structure
/// The API returns a flat map where keys are token addresses and values are
/// price data. Values are kept as raw JSON and parsed per-token so unknown
/// top-level metadata fields (e.g. a request id or `timeTaken`) never fail
/// the deserialization of the entire batch.
#[derive(Debug, serde::Deserialize)]
struct JupiterPriceResponse {
    #[serde(flatten)]
    data: std::collections::HashMap<String, serde_json::Value>,
}

/// Price data for a single token from Jupiter Price API V3
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
#[allow(non_snake_case)]
struct JupiterPriceData {
    /// Price in USD (field name changed from "price" to "usdPrice" in V3).
    /// ABSENT for tokens with no recent trade (dead/untradeable) — optional
    /// since 2026-08-09 so those entries skip quietly instead of failing parse.
    #[serde(default)]
    usdPrice: Option<f64>,
    /// Block height when this price was recorded
    #[serde(default)]
    blockId: Option<u64>,
    /// Token decimals
    #[serde(default)]
    decimals: Option<u8>,
    /// Price change over 24 hours (percentage)
    #[serde(default)]
    priceChange24h: Option<f64>,
    /// When this price was first created
    #[serde(default)]
    createdAt: Option<String>,
    /// Liquidity available for this token
    #[serde(default)]
    liquidity: Option<f64>,
}

/// Price cache errors
#[derive(Debug, thiserror::Error)]
pub enum PriceCacheError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    HttpError(String),

    /// JSON parsing failed
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Rate limited
    #[error("Rate limited by price API")]
    RateLimited,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_cache_set_get() {
        let cache = PriceCache::new().expect("Failed to create price cache for test");
        cache.set_price(
            "token1",
            Decimal::from_str("1.5").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );

        let price = cache.get_price_usd("token1");
        assert!(price.is_some());
        assert_eq!(price.unwrap(), Decimal::from_str("1.5").unwrap());
    }

    #[test]
    fn test_price_cache_miss() {
        let cache = PriceCache::new().expect("Failed to create price cache for test");
        assert!(cache.get_price("nonexistent").is_none());
    }

    #[test]
    fn test_jupiter_price_response_deserialization_with_null_fields() {
        let json_data = r#"{
            "So11111111111111111111111111111111111111112": {
                "usdPrice": 180.5,
                "blockId": 1234567,
                "decimals": 9,
                "priceChange24h": 2.5,
                "createdAt": "2026-07-23T00:00:00Z",
                "liquidity": 1000000.0
            },
            "32vUHPxVShN552WwJ36vWnxCoy34eTDHRiQwL6ZA3ntP": {
                "usdPrice": 0.000046,
                "blockId": null,
                "decimals": 6,
                "priceChange24h": null,
                "createdAt": null,
                "liquidity": null
            }
        }"#;

        let res: Result<JupiterPriceResponse, _> = serde_json::from_str(json_data);
        assert!(
            res.is_ok(),
            "Failed to deserialize Jupiter price response with null fields: {:?}",
            res.err()
        );
        let data = res.unwrap();
        assert_eq!(data.data.len(), 2);
        let sol_value = data
            .data
            .get("So11111111111111111111111111111111111111112")
            .unwrap();
        let sol: JupiterPriceData = serde_json::from_value(sol_value.clone()).unwrap();
        assert_eq!(sol.usdPrice, 180.5);
        let meme_value = data
            .data
            .get("32vUHPxVShN552WwJ36vWnxCoy34eTDHRiQwL6ZA3ntP")
            .unwrap();
        let meme: JupiterPriceData = serde_json::from_value(meme_value.clone()).unwrap();
        assert_eq!(meme.usdPrice, 0.000046);
        assert!(meme.priceChange24h.is_none());
        assert!(meme.blockId.is_none());
        assert!(meme.liquidity.is_none());
    }

    /// Unknown top-level metadata fields must be tolerated: they cannot fail
    /// the deserialization of the whole batch (regression for the old
    /// `#[serde(flatten)]` into typed values).
    #[test]
    fn test_jupiter_price_response_tolerates_unknown_metadata_fields() {
        let json_data = r#"{
            "timeTaken": 12,
            "requestId": "req-123",
            "So11111111111111111111111111111111111111112": {
                "usdPrice": 180.5,
                "decimals": 9
            }
        }"#;
        let res: JupiterPriceResponse = serde_json::from_str(json_data).unwrap();
        let sol: JupiterPriceData = serde_json::from_value(
            res.data
                .get("So11111111111111111111111111111111111111112")
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert_eq!(sol.usdPrice, 180.5);
        assert!(
            res.data.get("timeTaken").is_some(),
            "metadata field is kept in the map"
        );
    }

    #[test]
    fn test_track_token() {
        let cache = PriceCache::new().expect("Failed to create price cache for test");
        cache.track_token("token1");
        cache.track_token("token2");

        let tracked = cache.tracked_tokens();
        assert_eq!(tracked.len(), 2);
        assert!(tracked.contains(&"token1".to_string()));
    }

    #[test]
    fn test_unrealized_pnl_calculation() {
        let cache = PriceCache::new().expect("Failed to create price cache for test");
        cache.set_price(
            "token1",
            Decimal::from_str("2.0").unwrap(),
            PriceSource::Jupiter,
            Some(6),
        );

        let pnl = cache.calculate_unrealized_pnl(
            "token1",
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("100.0").unwrap(),
        );
        assert!(pnl.is_some());

        let pnl = pnl.unwrap();
        assert_eq!(pnl.pnl_usd, Decimal::from_str("100.0").unwrap()); // (2.0 - 1.0) * 100 = 100
        assert_eq!(pnl.pnl_percent, Decimal::from_str("100.0").unwrap()); // 100% gain
    }

    #[test]
    fn test_stats() {
        let cache = PriceCache::new().expect("Failed to create price cache for test");
        cache.set_price(
            "token1",
            Decimal::from_str("1.0").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
        cache.track_token("token1");
        cache.track_token("token2");

        let stats = cache.stats();
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.tracked_tokens, 2);
    }

    // ==========================================================================
    // DECIMALS CACHE TESTS
    // ==========================================================================

    #[test]
    fn test_decimals_cache_hit() {
        let cache = PriceCache::new().expect("Failed to create cache");
        cache.set_price(
            "token1",
            Decimal::from_str("1.0").unwrap(),
            PriceSource::Jupiter,
            Some(6),
        );

        assert_eq!(cache.get_decimals("token1"), Some(6));
    }

    #[test]
    fn test_decimals_cache_miss() {
        let cache = PriceCache::new().expect("Failed to create cache");
        assert_eq!(cache.get_decimals("nonexistent"), None);
    }

    #[test]
    fn test_decimals_none_in_entry() {
        let cache = PriceCache::new().expect("Failed to create cache");
        cache.set_price(
            "token1",
            Decimal::from_str("1.0").unwrap(),
            PriceSource::Jupiter,
            None, // No decimals data
        );

        assert_eq!(cache.get_decimals("token1"), None);
    }

    #[test]
    fn test_decimals_fallback_to_price_entry() {
        let cache = PriceCache::new().expect("Failed to create cache");
        // Set price with decimals (this stores decimals in PriceEntry)
        cache.set_price(
            "token1",
            Decimal::from_str("1.0").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );

        // Even without separate decimals cache, we should get decimals from PriceEntry
        assert_eq!(cache.get_decimals("token1"), Some(9));
    }
}
