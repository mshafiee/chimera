//! DexScreener market-data client (B3).
//!
//! Fetches 24h volume and liquidity per token from the DexScreener public API.
//! Short-TTL in-memory cache, own rate limiter, fail-open on every error path
//! (returns `None` → caller warns and continues; never blocks a trade).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use reqwest::Client;
use rust_decimal::Decimal;

use crate::engine::volume_cache::VolumeCache;
use crate::monitoring::rate_limiter::{RateLimiter, RequestPriority};

/// Per-token market snapshot.
#[derive(Debug, Clone)]
pub struct TokenMarketData {
    pub liquidity_usd: Decimal,
    pub volume_24h_usd: Decimal,
}

/// Cached entry with insertion time.
struct CacheEntry {
    data: TokenMarketData,
    at: Instant,
}

/// DexScreener client wrapping the public `/latest/dex/tokens/{address}` endpoint.
pub struct DexScreenerClient {
    http: Client,
    base_url: String,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    ttl: Duration,
    rate_limiter: Arc<RateLimiter>,
    /// Shared VolumeCache — fed with fresh 24h volume samples on every fetch.
    volume_cache: Arc<VolumeCache>,
}

impl DexScreenerClient {
    pub fn new(rate_limiter: Arc<RateLimiter>, volume_cache: Arc<VolumeCache>) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            base_url: "https://api.dexscreener.com/latest/dex/tokens".to_string(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(60),
            rate_limiter,
            volume_cache,
        }
    }

    /// Fetch market data for a token. Fail-open: any error returns `None`.
    /// Results are cached for `ttl` seconds. On a successful fetch, the
    /// 24h volume is recorded into the shared `VolumeCache`.
    pub async fn get_market_data(&self, token_address: &str) -> Option<TokenMarketData> {
        // Fast path: check cache
        {
            let cache = self.cache.read();
            if let Some(entry) = cache.get(token_address) {
                if entry.at.elapsed() < self.ttl {
                    return Some(entry.data.clone());
                }
            }
        }

        // Rate-limit before HTTP call
        self.rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        let url = format!("{}/{}", self.base_url, token_address);
        let response = match self.http.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                tracing::debug!(
                    token = token_address,
                    status = %r.status(),
                    "DexScreener returned non-success; fail-open"
                );
                return None;
            }
            Err(e) => {
                tracing::debug!(
                    token = token_address,
                    error = %e,
                    "DexScreener request failed; fail-open"
                );
                return None;
            }
        };

        let data: serde_json::Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(
                    token = token_address,
                    error = %e,
                    "DexScreener JSON parse failed; fail-open"
                );
                return None;
            }
        };

        let pairs = match data.get("pairs").and_then(|p| p.as_array()) {
            Some(p) => p,
            None => return None,
        };

        // Take the max across all Solana pairs (most liquid pool dominates).
        let mut max_liq: f64 = 0.0;
        let mut max_vol: f64 = 0.0;
        for pair in pairs {
            if pair.get("chainId").and_then(|c| c.as_str()) != Some("solana") {
                continue;
            }
            if let Some(liq) = pair
                .get("liquidity")
                .and_then(|l| l.get("usd"))
                .and_then(|u| u.as_f64())
            {
                max_liq = max_liq.max(liq);
            }
            if let Some(vol) = pair
                .get("volume")
                .and_then(|v| v.get("h24"))
                .and_then(|h| h.as_f64())
            {
                max_vol = max_vol.max(vol);
            }
        }

        let result = TokenMarketData {
            liquidity_usd: Decimal::from_f64_retain(max_liq).unwrap_or(Decimal::ZERO),
            volume_24h_usd: Decimal::from_f64_retain(max_vol).unwrap_or(Decimal::ZERO),
        };

        // Feed the shared VolumeCache with the fresh sample
        if max_vol > 0.0 {
            self.volume_cache
                .record_volume(token_address, result.volume_24h_usd);
        }

        // Update cache
        {
            let mut cache = self.cache.write();
            cache.insert(
                token_address.to_string(),
                CacheEntry {
                    data: result.clone(),
                    at: Instant::now(),
                },
            );
        }

        Some(result)
    }

    /// Convenience: return just the 24h volume, or `None` on failure.
    pub async fn get_volume_24h(&self, token_address: &str) -> Option<Decimal> {
        self.get_market_data(token_address)
            .await
            .map(|d| d.volume_24h_usd)
    }
}
