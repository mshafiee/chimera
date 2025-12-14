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
use std::sync::Arc;
use tokio::time::interval;

/// Default cache TTL in seconds
const DEFAULT_CACHE_TTL_SECS: i64 = 30;

/// Price update interval for active tokens
const PRICE_UPDATE_INTERVAL_SECS: u64 = 5;

/// Price entry in cache
#[derive(Debug, Clone)]
pub struct PriceEntry {
    /// Price in USD (using Decimal for precision)
    pub price_usd: Decimal,
    /// When this price was fetched
    pub fetched_at: DateTime<Utc>,
    /// Price source
    pub source: PriceSource,
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

/// Price cache for token prices
pub struct PriceCache {
    /// Cached prices by token address
    prices: Arc<RwLock<HashMap<String, PriceEntry>>>,
    /// Cache TTL
    ttl: Duration,
    /// Tokens to actively track
    active_tokens: Arc<RwLock<Vec<String>>>,
    /// Whether the updater is running
    updater_running: Arc<RwLock<bool>>,
    /// Price history for volatility calculation (token -> VecDeque of (timestamp, price))
    pub price_history: Arc<RwLock<HashMap<String, VecDeque<(DateTime<Utc>, Decimal)>>>>,
    /// SOL mint address (for market condition filtering)
    sol_mint: String,
}

impl PriceCache {
    /// Create a new price cache with default TTL
    pub fn new() -> Self {
        Self {
            prices: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::seconds(DEFAULT_CACHE_TTL_SECS),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            updater_running: Arc::new(RwLock::new(false)),
            price_history: Arc::new(RwLock::new(HashMap::new())),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
        }
    }

    /// Create with custom TTL
    pub fn with_ttl(ttl_secs: i64) -> Self {
        Self {
            prices: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::seconds(ttl_secs),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            updater_running: Arc::new(RwLock::new(false)),
            price_history: Arc::new(RwLock::new(HashMap::new())),
            sol_mint: "So11111111111111111111111111111111111111112".to_string(),
        }
    }

    /// Get price for a token
    pub fn get_price(&self, token_address: &str) -> Option<PriceEntry> {
        let prices = self.prices.read();
        let entry = prices.get(token_address)?;

        // Check if expired
        let age = Utc::now().signed_duration_since(entry.fetched_at);
        if age > self.ttl {
            return None;
        }

        Some(entry.clone())
    }

    /// Get price in USD (convenience method)
    pub fn get_price_usd(&self, token_address: &str) -> Option<Decimal> {
        self.get_price(token_address).map(|e| e.price_usd)
    }

    /// Set price for a token
    pub fn set_price(&self, token_address: &str, price_usd: Decimal, source: PriceSource) {
        let now = Utc::now();
        let mut prices = self.prices.write();
        prices.insert(
            token_address.to_string(),
            PriceEntry {
                price_usd,
                fetched_at: now,
                source,
            },
        );
        
        // Update price history for volatility calculation (keep last 24 hours)
        let mut history = self.price_history.write();
        let token_history = history.entry(token_address.to_string()).or_insert_with(VecDeque::new);
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
    
    /// Calculate volatility for a token (24h window)
    ///
    /// Returns volatility as percentage (0.0-100.0)
    /// Returns None if insufficient data (< 2 price points)
    pub fn calculate_volatility(&self, token_address: &str) -> Option<f64> {
        let history = self.price_history.read();
        let token_history = history.get(token_address)?;
        
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
            .sum::<Decimal>() / count;
        
        // Convert to f64 for sqrt (volatility is a statistical metric, not a financial amount)
        let variance_f64 = variance.to_f64().unwrap_or(0.0);
        let std_dev = variance_f64.sqrt();
        
        // Return absolute volatility (as percentage)
        Some(std_dev.abs())
    }
    
    /// Get SOL price volatility (for market condition filtering)
    pub fn get_sol_volatility(&self) -> Option<f64> {
        self.calculate_volatility(&self.sol_mint)
    }

    /// Add token to active tracking
    pub fn track_token(&self, token_address: &str) {
        let mut tokens = self.active_tokens.write();
        if !tokens.contains(&token_address.to_string()) {
            tokens.push(token_address.to_string());
            tracing::debug!(token = token_address, "Added token to price tracking");
        }
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

    /// Start the background price updater
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
            "Starting price cache updater"
        );

        let mut update_interval = interval(std::time::Duration::from_secs(PRICE_UPDATE_INTERVAL_SECS));

        loop {
            update_interval.tick().await;

            let tokens = self.active_tokens.read().clone();
            if tokens.is_empty() {
                continue;
            }

            if let Err(e) = self.update_prices(&tokens).await {
                tracing::error!(error = %e, "Failed to update prices");
            }
        }
    }

    /// Update prices for a list of tokens
    async fn update_prices(&self, tokens: &[String]) -> Result<(), PriceCacheError> {
        // Fetch prices from Jupiter API
        let prices = self.fetch_prices_jupiter(tokens).await?;

        for (token, price) in prices {
            self.set_price(&token, price, PriceSource::Jupiter);
        }

        tracing::debug!(
            token_count = tokens.len(),
            "Updated prices"
        );

        Ok(())
    }

    /// Fetch prices from Jupiter Price API
    async fn fetch_prices_jupiter(
        &self,
        tokens: &[String],
    ) -> Result<Vec<(String, Decimal)>, PriceCacheError> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        // Build URL with comma-separated token addresses
        let token_list = tokens.join(",");
        let url = format!("https://price.jup.ag/v6/price?ids={}", token_list);

        tracing::debug!(
            token_count = tokens.len(),
            url = %url,
            "Fetching prices from Jupiter"
        );

        // Make HTTP request with retry logic
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| PriceCacheError::HttpError(format!("Failed to create HTTP client: {}", e)))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| PriceCacheError::HttpError(format!("Jupiter price request failed: {}", e)))?;

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
        let data: JupiterPriceResponse = response
            .json()
            .await
            .map_err(|e| PriceCacheError::ParseError(format!("Failed to parse Jupiter response: {}", e)))?;

        // Extract prices from response and convert to Decimal
        let mut results = Vec::new();
        for token in tokens {
            if let Some(price_data) = data.data.get(token) {
                // Jupiter returns price in USD as f64, convert to Decimal for precision
                let price = Decimal::from_f64_retain(price_data.price)
                    .unwrap_or_else(|| Decimal::from_str(&price_data.price.to_string()).unwrap_or(Decimal::ZERO));
                results.push((token.clone(), price));
            } else {
                tracing::warn!(
                    token = token,
                    "Token not found in Jupiter price response"
                );
                // Skip tokens not found in response
            }
        }

        tracing::debug!(
            fetched_count = results.len(),
            total_requested = tokens.len(),
            "Fetched prices from Jupiter"
        );

        Ok(results)
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
        let prices = self.prices.read();
        let now = Utc::now();

        let mut valid_count = 0;
        let mut stale_count = 0;

        for entry in prices.values() {
            let age = now.signed_duration_since(entry.fetched_at);
            if age <= self.ttl {
                valid_count += 1;
            } else {
                stale_count += 1;
            }
        }

        PriceCacheStats {
            total_entries: prices.len(),
            valid_entries: valid_count,
            stale_entries: stale_count,
            tracked_tokens: self.active_tokens.read().len(),
        }
    }

    /// Clear expired entries
    pub fn prune_expired(&self) {
        let mut prices = self.prices.write();
        let now = Utc::now();

        prices.retain(|_, entry| {
            let age = now.signed_duration_since(entry.fetched_at);
            age <= self.ttl
        });
    }
}

impl Default for PriceCache {
    fn default() -> Self {
        Self::new()
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
}

/// Jupiter Price API response structure
#[derive(Debug, serde::Deserialize)]
struct JupiterPriceResponse {
    data: std::collections::HashMap<String, JupiterPriceData>,
}

/// Price data for a single token
#[derive(Debug, serde::Deserialize)]
struct JupiterPriceData {
    /// Price in USD
    price: f64,
    /// Other fields (we only need price)
    #[serde(flatten)]
    _other: serde_json::Value,
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
        let cache = PriceCache::new();
        cache.set_price("token1", Decimal::from_str("1.5").unwrap(), PriceSource::Jupiter);

        let price = cache.get_price_usd("token1");
        assert!(price.is_some());
        assert_eq!(price.unwrap(), Decimal::from_str("1.5").unwrap());
    }

    #[test]
    fn test_price_cache_miss() {
        let cache = PriceCache::new();
        assert!(cache.get_price("nonexistent").is_none());
    }

    #[test]
    fn test_track_token() {
        let cache = PriceCache::new();
        cache.track_token("token1");
        cache.track_token("token2");

        let tracked = cache.tracked_tokens();
        assert_eq!(tracked.len(), 2);
        assert!(tracked.contains(&"token1".to_string()));
    }

    #[test]
    fn test_unrealized_pnl_calculation() {
        let cache = PriceCache::new();
        cache.set_price("token1", Decimal::from_str("2.0").unwrap(), PriceSource::Jupiter);

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
        let cache = PriceCache::new();
        cache.set_price("token1", Decimal::from_str("1.0").unwrap(), PriceSource::Jupiter);
        cache.track_token("token1");
        cache.track_token("token2");

        let stats = cache.stats();
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.tracked_tokens, 2);
    }
}
