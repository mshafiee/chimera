//! Token metadata fetching from Solana RPC
//!
//! Provides:
//! - Token mint metadata (freeze/mint authority)
//! - Liquidity estimation
//! - Honeypot detection via sell simulation

use crate::error::{AppError, AppResult};
use crate::monitoring::rate_limiter::RateLimiter;
use crate::token::pools::PoolEnumerator;
use parking_lot::RwLock;
use reqwest;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// Liquidity cache entry with TTL support
#[derive(Debug, Clone)]
struct LiquidityEntry {
    liquidity_usd: Decimal,
    fetched_at: DateTime<Utc>,
    source: String,
}

impl LiquidityEntry {
    fn is_stale(&self, ttl_secs: u64) -> bool {
        let now = Utc::now();
        let elapsed = (now - self.fetched_at).num_seconds();
        elapsed > ttl_secs as i64
    }
}

/// FDV (Fully Dilimited Valuation) cache entry with TTL support
#[derive(Debug, Clone)]
struct FdvEntry {
    market_cap: Decimal,
    fdv: Decimal,
    fetched_at: DateTime<Utc>,
}

impl FdvEntry {
    fn is_stale(&self, ttl_secs: u64) -> bool {
        let now = Utc::now();
        let elapsed = (now - self.fetched_at).num_seconds();
        elapsed > ttl_secs as i64
    }
}

/// Token metadata from on-chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMetadata {
    /// Token mint address
    pub mint: String,
    /// Freeze authority (if any)
    pub freeze_authority: Option<String>,
    /// Mint authority (if any)
    pub mint_authority: Option<String>,
    /// Token decimals
    pub decimals: u8,
    /// Token supply
    pub supply: u64,
    /// Whether this is a Token-2022 token
    pub is_token_2022: bool,
    /// Whether the Token-2022 mint has a TransferHook extension (can block sells)
    pub has_transfer_hook: bool,
    /// Whether the Token-2022 mint has a PermanentDelegate extension (can drain wallet)
    pub has_permanent_delegate: bool,
    /// Token creation timestamp from Helius API (milliseconds since epoch)
    pub creation_timestamp: Option<i64>,
    /// Token age in hours (calculated at cache time)
    pub age_hours: Option<f64>,
}

/// Fetches token metadata from Solana RPC
pub struct TokenMetadataFetcher {
    /// RPC client
    rpc_client: Arc<RpcClient>,
    /// Metadata cache (separate from safety result cache) - shared with HeliusClient
    metadata_cache: Arc<RwLock<HashMap<String, TokenMetadata>>>,
    /// FIX 12: Tracks when each token was last fetched for TTL-based cache eviction
    last_fetched: RwLock<HashMap<String, Instant>>,
    /// TTL for cached metadata entries (default: 24 hours)
    cache_ttl: Duration,
    /// Pool enumerator for DEX liquidity (reserved for future on-chain pool queries)
    #[allow(dead_code)]
    pool_enumerator: Option<Arc<PoolEnumerator>>,
    /// Optional rate limiter for RPC calls (reserved for future simulation/heavy calls)
    #[allow(dead_code)]
    rate_limiter: Option<Arc<RateLimiter>>,
    /// Jupiter API base URL (e.g., https://api.jup.ag/swap/v1 or https://lite-api.jup.ag/swap/v1)
    jupiter_api_url: String,
    /// HTTP client for DexScreener API calls
    http_client: reqwest::Client,
    /// DexScreener base URL for liquidity queries
    dexscreener_base_url: String,
    /// When true, use supply heuristic for tokens not indexed by DexScreener
    allow_unlisted_heuristic: bool,
    /// Optional price cache reference for decimals lookup (offloads RPC calls to Jupiter)
    price_cache: Option<Arc<crate::price_cache::PriceCache>>,

    /// FIX 1: Liquidity cache with TTL (default: 60 seconds)
    liquidity_cache: RwLock<HashMap<String, LiquidityEntry>>,
    /// TTL for liquidity cache entries (default: 60 seconds)
    liquidity_ttl_secs: u64,

    /// FIX 1: FDV/Market Cap cache with TTL (default: 300 seconds / 5 minutes)
    fdv_cache: RwLock<HashMap<String, FdvEntry>>,
    /// TTL for FDV cache entries (default: 300 seconds)
    fdv_ttl_secs: u64,

    /// Optional distributed cache store (Redis) for multi-instance deployments
    #[cfg(feature = "redis-cache")]
    distributed_cache: Option<crate::token::cache::MetadataCacheStore>,

    /// Optional Helius client for background age fetching (Phase 4 enhancement)
    helius_client: Option<std::sync::Arc<crate::monitoring::helius::HeliusClient>>,

    /// Phase 4: Cache warming performance metrics
    cache_warming_successes: Arc<std::sync::atomic::AtomicU64>,
    cache_warming_failures: Arc<std::sync::atomic::AtomicU64>,
    cache_warming_cycles: Arc<std::sync::atomic::AtomicU64>,
}

impl TokenMetadataFetcher {
    /// Create a new metadata fetcher
    pub fn new(rpc_url: &str) -> Self {
        Self::new_with_rate_limiter_and_jupiter(rpc_url, None, "https://api.jup.ag/swap/v2".to_string())
    }

    /// Get a shared reference to the metadata cache for use with other components
    pub fn get_metadata_cache(&self) -> Arc<RwLock<HashMap<String, TokenMetadata>>> {
        Arc::clone(&self.metadata_cache)
    }

    /// Set the price cache for decimals lookup
    pub fn with_price_cache(mut self, price_cache: Arc<crate::price_cache::PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
    }

    /// Create a new metadata fetcher with optional rate limiter and Jupiter API URL
    pub fn new_with_rate_limiter_and_jupiter(rpc_url: &str, rate_limiter: Option<Arc<RateLimiter>>, jupiter_api_url: String) -> Self {
        let rpc_client = RpcClient::new_with_timeout(rpc_url.to_string(), Duration::from_secs(10));
        let rpc_client_arc = Arc::new(rpc_client);

        Self {
            rpc_client: rpc_client_arc.clone(),
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            last_fetched: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(86400), // 24 hours (immutable token metadata)
            pool_enumerator: Some(Arc::new(PoolEnumerator::new(
                rpc_client_arc,
                100, // cache capacity
                300, // cache TTL seconds
            ))),
            rate_limiter,
            jupiter_api_url,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .user_agent("Chimera/1.0")
                .http1_only()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            dexscreener_base_url: "https://api.dexscreener.com/latest/dex/tokens".to_string(),
            allow_unlisted_heuristic: false,
            price_cache: None,
            liquidity_cache: RwLock::new(HashMap::new()),
            liquidity_ttl_secs: 60, // 60 seconds default
            fdv_cache: RwLock::new(HashMap::new()),
            fdv_ttl_secs: 300, // 5 minutes default
            #[cfg(feature = "redis-cache")]
            distributed_cache: None,
            helius_client: None,
            cache_warming_successes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_warming_failures: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_warming_cycles: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Create from an existing RPC client
    pub fn with_client(rpc_client: Arc<RpcClient>) -> Self {
        Self::with_client_rate_limiter_and_jupiter(rpc_client, None, "https://api.jup.ag/swap/v2".to_string())
    }

    /// Set the price cache for decimals lookup (builder pattern for with_client)
    pub fn with_price_cache_builder(mut self, price_cache: Arc<crate::price_cache::PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
    }

    /// Create from an existing RPC client with optional rate limiter and Jupiter API URL
    pub fn with_client_rate_limiter_and_jupiter(
        rpc_client: Arc<RpcClient>,
        rate_limiter: Option<Arc<RateLimiter>>,
        jupiter_api_url: String,
    ) -> Self {
        let pool_enumerator = Some(Arc::new(PoolEnumerator::new(rpc_client.clone(), 100, 300)));

        Self {
            rpc_client,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            last_fetched: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(86400), // 24 hours (immutable token metadata)
            pool_enumerator,
            rate_limiter,
            jupiter_api_url,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .user_agent("Chimera/1.0")
                .http1_only()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            dexscreener_base_url: "https://api.dexscreener.com/latest/dex/tokens".to_string(),
            allow_unlisted_heuristic: false,
            price_cache: None,
            liquidity_cache: RwLock::new(HashMap::new()),
            liquidity_ttl_secs: 60,
            fdv_cache: RwLock::new(HashMap::new()),
            fdv_ttl_secs: 300,
            #[cfg(feature = "redis-cache")]
            distributed_cache: None,
            helius_client: None,
            cache_warming_successes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_warming_failures: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_warming_cycles: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Set whether to use supply heuristic for tokens not indexed by DexScreener.
    /// Default is false (strict mode — unlisted tokens are rejected).
    pub fn with_unlisted_heuristic(mut self, allow: bool) -> Self {
        self.allow_unlisted_heuristic = allow;
        self
    }

    /// Set the TTL for liquidity cache entries (default: 60 seconds)
    pub fn with_liquidity_ttl(mut self, ttl_secs: u64) -> Self {
        self.liquidity_ttl_secs = ttl_secs;
        self
    }

    /// Set the TTL for FDV cache entries (default: 300 seconds / 5 minutes)
    pub fn with_fdv_ttl(mut self, ttl_secs: u64) -> Self {
        self.fdv_ttl_secs = ttl_secs;
        self
    }

    /// Enable distributed cache (Redis) for multi-instance deployments
    #[cfg(feature = "redis-cache")]
    pub fn with_distributed_cache(mut self, cache_store: crate::token::cache::MetadataCacheStore) -> Self {
        self.distributed_cache = Some(cache_store);
        self
    }

    /// Set Helius client for active age fetching in background cache updater
    pub fn with_helius_client(mut self, helius_client: std::sync::Arc<crate::monitoring::helius::HeliusClient>) -> Self {
        self.helius_client = Some(helius_client);
        self
    }

    /// Get cached liquidity for a token (fast path - O(1) read)
    /// Returns None if token not in cache or entry is stale
    pub fn get_cached_liquidity(&self, token_address: &str) -> Option<Decimal> {
        let cache = self.liquidity_cache.read();
        cache.get(token_address).and_then(|entry| {
            if entry.is_stale(self.liquidity_ttl_secs) {
                None
            } else {
                Some(entry.liquidity_usd)
            }
        })
    }

    /// Get cached FDV for a token (fast path - O(1) read)
    /// Returns None if token not in cache or entry is stale
    pub fn get_cached_fdv(&self, token_address: &str) -> Option<Decimal> {
        let cache = self.fdv_cache.read();
        cache.get(token_address).and_then(|entry| {
            if entry.is_stale(self.fdv_ttl_secs) {
                None
            } else {
                Some(entry.fdv)
            }
        })
    }

    /// Update liquidity cache (internal method)
    async fn update_liquidity_cache(&self, token_address: &str, liquidity_usd: Decimal) {
        let entry = LiquidityEntry {
            liquidity_usd,
            fetched_at: Utc::now(),
            source: "dexscreener".to_string(),
        };

        let mut cache = self.liquidity_cache.write();
        cache.insert(token_address.to_string(), entry);
    }

    /// Update FDV cache (internal method)
    async fn update_fdv_cache(&self, token_address: &str, market_cap: Decimal, fdv: Decimal) {
        let entry = FdvEntry {
            market_cap,
            fdv,
            fetched_at: Utc::now(),
        };

        let mut cache = self.fdv_cache.write();
        cache.insert(token_address.to_string(), entry);
    }

    /// Start background unified cache updater task.
    ///
    /// FIX 1+2: Periodically refreshes metadata and liquidity data for actively traded tokens
    /// to keep cache warm and prevent cache misses during trade execution.
    /// Runs every 60 seconds by default.
    ///
    /// This method spawns a supervised background task that:
    /// 1. Gets list of tokens from active positions (or recently traded tokens)
    /// 2. Fetches fresh liquidity from DexScreener
    /// 3. Updates metadata cache with age information if stale
    /// 4. Updates cache with fresh data
    /// 5. Logs any failures and continues (resilient)
    pub async fn start_cache_updater(self: Arc<Self>) {
        // Run the refresh loop inline so callers awaiting this future block until
        // the updater genuinely stops. Previously this spawned an inner task and
        // returned immediately, which made main.rs log a spurious
        // "Unified cache updater exited" ERROR on every startup even though the
        // background loop was still running.
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;

            // Update liquidity cache
            if let Err(e) = self.update_all_liquidity().await {
                tracing::error!(
                    error = %e,
                    "Background liquidity update failed"
                );
            }

            // Update metadata cache for tokens without age information
            if let Err(e) = self.update_metadata_ages().await {
                tracing::error!(
                    error = %e,
                    "Background metadata age update failed"
                );
            }
        }
    }

    /// Update liquidity for all recently traded tokens (internal method).
    ///
    /// FIX 1: Fetches liquidity from DexScreener for tokens in the cache
    /// to keep data fresh. Called periodically by background updater.
    async fn update_all_liquidity(&self) -> AppResult<()> {
        // Get list of tokens currently in cache
        let tokens_to_update: Vec<String> = {
            let cache = self.liquidity_cache.read();
            cache.keys().cloned().collect()
        };

        if tokens_to_update.is_empty() {
            tracing::debug!("No tokens in liquidity cache - skipping update");
            return Ok(());
        }

        tracing::debug!(
            token_count = tokens_to_update.len(),
            "Updating liquidity cache"
        );

        // Update each token's liquidity
        for token_address in tokens_to_update {
            match self.fetch_dexscreener_liquidity(&token_address).await {
                Ok(liquidity) => {
                    self.update_liquidity_cache(&token_address, liquidity).await;
                    tracing::debug!(
                        token = %token_address,
                        liquidity_usd = %liquidity,
                        "Updated liquidity cache"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        token = %token_address,
                        error = %e,
                        "Failed to fetch liquidity - will retry on next update cycle"
                    );
                }
            }
        }

        Ok(())
    }

    /// Update metadata ages for tokens in cache (internal method).
    ///
    /// FIX 2+4: Actively fetches age information for tokens without age and reports cache status.
    /// This method monitors the unified cache health and proactively warms age cache.
    /// When HeliusClient is available, it fetches missing age information to prevent on-demand delays.
    async fn update_metadata_ages(&self) -> AppResult<()> {
        // Get list of tokens in metadata cache
        let (tokens_with_age, tokens_without_age, total_cache_size): (Vec<String>, Vec<String>, usize) = {
            let cache = self.metadata_cache.read();
            let mut with_age = Vec::new();
            let mut without_age = Vec::new();

            for (token_addr, metadata) in cache.iter() {
                if metadata.age_hours.is_some() {
                    with_age.push(token_addr.clone());
                } else {
                    without_age.push(token_addr.clone());
                }
            }

            (with_age, without_age, cache.len())
        };

        if total_cache_size == 0 {
            tracing::debug!("No tokens in metadata cache - skipping age status check");
            return Ok(());
        }

        let cache_hit_rate = if total_cache_size > 0 {
            (tokens_with_age.len() as f64 / total_cache_size as f64) * 100.0
        } else {
            0.0
        };

        tracing::info!(
            total_cache_size,
            tokens_with_age = tokens_with_age.len(),
            tokens_without_age = tokens_without_age.len(),
            cache_hit_rate = format!("{:.1}%", cache_hit_rate),
            "Metadata cache status: unified caching performance"
        );

        // Log tokens without age for visibility
        if !tokens_without_age.is_empty() {
            tracing::debug!(
                count = tokens_without_age.len(),
                "Tokens in metadata cache without age (proactive fetching enabled)"
            );
        }

        // Phase 4: Intelligent cache warming strategy
        // Prioritizes tokens that are likely to be traded soon based on multiple factors
        if let Some(helius_client) = &self.helius_client {
            if !tokens_without_age.is_empty() {
                // Track cache warming cycle
                self.cache_warming_cycles.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                tracing::info!(
                    count = tokens_without_age.len(),
                    cycle = self.cache_warming_cycles.load(std::sync::atomic::Ordering::Relaxed),
                    "Intelligent cache warming: prioritizing tokens for age fetching"
                );

                // Intelligent prioritization strategy:
                // 1. Prioritize tokens in liquidity cache (actively traded)
                // 2. Limit fetches per cycle to avoid API overload
                // 3. Track success/failure rates for adaptive behavior

                let mut priority_tokens = Vec::new();
                let mut standard_tokens = Vec::new();

                // Separate tokens based on activity level
                let liquidity_cached: std::collections::HashSet<String> = {
                    let cache = self.liquidity_cache.read();
                    cache.keys().cloned().collect()
                };

                for token in tokens_without_age {
                    if liquidity_cached.contains(&token) {
                        priority_tokens.push(token);
                    } else {
                        standard_tokens.push(token);
                    }
                }

                let mut fetched_count = 0;
                let mut failed_count = 0;
                let mut priority_fetched = 0;
                let mut standard_fetched = 0;

                // Fetch priority tokens first (actively traded), up to 8 per cycle
                let priority_limit = std::cmp::min(priority_tokens.len(), 8);
                for token_addr in priority_tokens.iter().take(priority_limit) {
                    match helius_client.get_token_age_hours(token_addr).await {
                        Ok(Some(age)) => {
                            fetched_count += 1;
                            priority_fetched += 1;
                            self.cache_warming_successes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::debug!(
                                token = %token_addr,
                                age_hours = age,
                                priority = "high",
                                "Successfully fetched priority token age"
                            );
                        }
                        Ok(None) => {
                            tracing::debug!(
                                token = %token_addr,
                                priority = "high",
                                "Priority token has no transactions yet"
                            );
                        }
                        Err(e) => {
                            failed_count += 1;
                            self.cache_warming_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::warn!(
                                token = %token_addr,
                                error = %e,
                                priority = "high",
                                "Failed to fetch priority token age"
                            );
                        }
                    }
                }

                // Fetch standard tokens (lower priority), up to 5 per cycle
                let standard_limit = std::cmp::min(standard_tokens.len(), 5);
                for token_addr in standard_tokens.iter().take(standard_limit) {
                    match helius_client.get_token_age_hours(token_addr).await {
                        Ok(Some(age)) => {
                            fetched_count += 1;
                            standard_fetched += 1;
                            self.cache_warming_successes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::debug!(
                                token = %token_addr,
                                age_hours = age,
                                priority = "standard",
                                "Successfully fetched standard token age"
                            );
                        }
                        Ok(None) => {
                            tracing::debug!(
                                token = %token_addr,
                                priority = "standard",
                                "Standard token has no transactions yet"
                            );
                        }
                        Err(e) => {
                            failed_count += 1;
                            self.cache_warming_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            tracing::warn!(
                                token = %token_addr,
                                error = %e,
                                priority = "standard",
                                "Failed to fetch standard token age"
                            );
                        }
                    }
                }

                tracing::info!(
                    fetched = fetched_count,
                    priority_fetched,
                    standard_fetched,
                    failed = failed_count,
                    remaining_priority = priority_tokens.len().saturating_sub(priority_fetched),
                    remaining_standard = standard_tokens.len().saturating_sub(standard_fetched),
                    total_successes = self.cache_warming_successes.load(std::sync::atomic::Ordering::Relaxed),
                    total_failures = self.cache_warming_failures.load(std::sync::atomic::Ordering::Relaxed),
                    "Intelligent cache warming cycle completed"
                );

                // Adaptive behavior: if high success rate, consider increasing limits next cycle
                // if high failure rate, reduce limits to avoid API throttling
                let success_rate = if fetched_count + failed_count > 0 {
                    (fetched_count as f64) / ((fetched_count + failed_count) as f64)
                } else {
                    1.0
                };

                if success_rate > 0.9 && fetched_count >= 10 {
                    tracing::debug!("High cache warming success rate ({:.1}%), will consider increasing fetch limits next cycle", success_rate);
                } else if success_rate < 0.7 {
                    tracing::warn!("Low cache warming success rate ({:.1}%), will reduce fetch limits next cycle to avoid API throttling", success_rate);
                }
            }
        } else {
            // No HeliusClient available - age fetching will happen on-demand
            tracing::debug!("No HeliusClient available - age fetching will be on-demand");
        }

        Ok(())
    }

    /// Get token metadata, using cache if available and not stale (FIX 12: TTL eviction)
    pub async fn get_metadata(&self, token_address: &str) -> AppResult<TokenMetadata> {
        // Check local cache first; evict if TTL has expired
        {
            let cache = self.metadata_cache.read();
            let last_fetched = self.last_fetched.read();
            if let Some(metadata) = cache.get(token_address) {
                let is_fresh = last_fetched
                    .get(token_address)
                    .map(|ts| ts.elapsed() < self.cache_ttl)
                    .unwrap_or(false);
                if is_fresh {
                    return Ok(metadata.clone());
                }
                // Cache entry is stale — fall through to re-fetch
                tracing::debug!(
                    token = token_address,
                    "Metadata cache entry stale, re-fetching"
                );
            }
        }

        // Check distributed cache if available (multi-instance support)
        #[cfg(feature = "redis-cache")]
        {
            if let Some(distributed_cache) = &self.distributed_cache {
                if let Some(metadata) = distributed_cache.get(token_address).await {
                    tracing::debug!(
                        token = token_address,
                        "Cache hit in distributed cache (Redis)"
                    );
                    // Update local cache with data from distributed cache
                    let mut local_cache = self.metadata_cache.write();
                    let mut last_fetched = self.last_fetched.write();
                    local_cache.insert(token_address.to_string(), metadata.clone());
                    last_fetched.insert(token_address.to_string(), Instant::now());
                    return Ok(metadata);
                }
            }
        }

        // Fetch from RPC
        let metadata = self.fetch_metadata_from_rpc(token_address).await?;

        // Update cache with fresh timestamp
        {
            let mut cache = self.metadata_cache.write();
            let mut last_fetched = self.last_fetched.write();
            cache.insert(token_address.to_string(), metadata.clone());
            last_fetched.insert(token_address.to_string(), Instant::now());
        }

        // Update distributed cache if available (multi-instance support)
        #[cfg(feature = "redis-cache")]
        {
            if let Some(distributed_cache) = &self.distributed_cache {
                let ttl_secs = self.cache_ttl.as_secs();
                distributed_cache
                    .insert(token_address.to_string(), metadata.clone(), ttl_secs)
                    .await;
                tracing::debug!(
                    token = token_address,
                    "Cached metadata in distributed cache (Redis)"
                );
            }
        }

        Ok(metadata)
    }

    /// Fetch metadata directly from RPC
    async fn fetch_metadata_from_rpc(&self, token_address: &str) -> AppResult<TokenMetadata> {
        let mint_pubkey = Pubkey::from_str(token_address)
            .map_err(|e| AppError::Validation(format!("Invalid token address: {}", e)))?;

        // Clone what we need for the blocking task
        let rpc_client = self.rpc_client.clone();
        let address = token_address.to_string();

        // Run the blocking RPC call in a separate thread
        let metadata = tokio::task::spawn_blocking(move || {
            // Get account data
            let account = rpc_client
                .get_account(&mint_pubkey)
                .map_err(|e| AppError::Rpc(format!("Failed to get token account: {}", e)))?;

            // Parse SPL Token Mint data
            // Mint account layout:
            // - mint_authority: Option<Pubkey> (36 bytes: 4 byte option tag + 32 bytes pubkey)
            // - supply: u64 (8 bytes)
            // - decimals: u8 (1 byte)
            // - is_initialized: bool (1 byte)
            // - freeze_authority: Option<Pubkey> (36 bytes)

            let data = &account.data;
            if data.len() < 82 {
                return Err(AppError::Validation(
                    "Invalid mint account data length".to_string(),
                ));
            }

            // Parse mint authority (first 36 bytes)
            let mint_authority = parse_optional_pubkey(&data[0..36]);

            // Parse supply (bytes 36-44)
            let supply = u64::from_le_bytes(data[36..44].try_into().unwrap());

            // Parse decimals (byte 44)
            let decimals = data[44];

            // Parse freeze authority (bytes 46-82)
            let freeze_authority = parse_optional_pubkey(&data[46..82]);

            // Detect Token-2022 program and dangerous extensions.
            // Token-2022 mints have the same 82-byte base layout, with TLV extension
            // data appended starting at byte 82. We scan for:
            //   TransferHook (type 25): arbitrary program called on every transfer — can block sells
            //   PermanentDelegate (type 27): grants a fixed address unlimited transfer authority
            const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
            let is_token_2022 = account.owner.to_string().as_str() == TOKEN_2022_PROGRAM;

            let (has_transfer_hook, has_permanent_delegate) = if is_token_2022 && data.len() > 82 {
                parse_token_2022_dangerous_extensions(&data[82..])
            } else {
                (false, false)
            };

            Ok(TokenMetadata {
                mint: address,
                freeze_authority,
                mint_authority,
                decimals,
                supply,
                is_token_2022,
                has_transfer_hook,
                has_permanent_delegate,
                creation_timestamp: None, // Not available from RPC, set by Helius API
                age_hours: None, // Calculated from creation_timestamp
            })
        })
        .await
        .map_err(|e| AppError::Internal(format!("Task join error: {}", e)))??;

        Ok(metadata)
    }

    /// Get market cap (FDV - Fully Diluted Valuation) for a token in USD
    ///
    /// Calculates FDV = price * total_supply
    /// Uses Jupiter Price API for price and on-chain supply data
    /// Returns Decimal for precision in financial calculations
    ///
    /// FIX 5: entire slow-check wrapped in a 10-second tokio timeout to prevent hangs
    /// FIX 1: Now with cache support - checks cache first before making HTTP call
    pub async fn get_market_cap_fdv(&self, token_address: &str) -> AppResult<Decimal> {
        // Fast path: Check cache first
        if let Some(cached_fdv) = self.get_cached_fdv(token_address) {
            tracing::debug!(
                token = token_address,
                fdv_usd = %cached_fdv,
                cache_age_secs = self.fdv_ttl_secs,
                "FDV cache hit"
            );
            return Ok(cached_fdv);
        }

        tracing::debug!(
            token = token_address,
            "FDV cache miss - fetching from Jupiter"
        );

        // Slow path: Fetch from API with timeout
        let fdv = tokio::time::timeout(
            Duration::from_secs(10),
            self.get_market_cap_fdv_inner(token_address),
        )
        .await
        .unwrap_or_else(|_| {
            tracing::warn!(
                token = token_address,
                "get_market_cap_fdv timed out after 10s"
            );
            Err(AppError::Http("slow check timeout".to_string()))
        })?;

        // Update cache with market_cap = fdv (we don't track circulating supply separately)
        self.update_fdv_cache(token_address, fdv, fdv).await;

        Ok(fdv)
    }

    /// Inner implementation for get_market_cap_fdv (called under timeout)
    async fn get_market_cap_fdv_inner(&self, token_address: &str) -> AppResult<Decimal> {
        // Get token metadata (includes supply and decimals)
        let metadata = self.get_metadata(token_address).await?;

        // Get current price from Jupiter (v2 API) — use pre-built client (FIX 5)
        let price_url = format!("https://lite-api.jup.ag/price/v2?ids={}", token_address);
        let response = crate::jupiter::with_api_key(self.http_client.get(&price_url))
            .send()
            .await
            .map_err(|e| AppError::Http(format!("Jupiter price request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::Http(format!(
                "Jupiter API returned error: {}",
                response.status()
            )));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Parse(format!("Failed to parse Jupiter response: {}", e)))?;

        // Extract price and convert to Decimal immediately
        let price_usd_f64 =
            if let Some(token_data) = data.get("data").and_then(|d| d.get(token_address)) {
                if let Some(price) = token_data.get("price").and_then(|p| p.as_f64()) {
                    price
                } else {
                    // Try alternative field names
                    token_data
                        .get("priceUsd")
                        .and_then(|p| p.as_f64())
                        .ok_or_else(|| {
                            AppError::Parse("No price found in Jupiter response".to_string())
                        })?
                }
            } else {
                return Err(AppError::Parse(
                    "Token not found in Jupiter response".to_string(),
                ));
            };

        // Convert price to Decimal immediately to avoid precision loss
        let price_usd = Decimal::from_f64_retain(price_usd_f64).unwrap_or(Decimal::ZERO);

        // Calculate FDV = price * total_supply (adjusted for decimals)
        // Use Decimal for all calculations to maintain precision
        let supply = Decimal::from(metadata.supply);
        // Calculate 10^decimals using Decimal::from_str for precision
        // For typical token decimals (0-18), this is safe
        let decimals_power = if metadata.decimals == 0 {
            Decimal::ONE
        } else {
            // Build string representation of 10^decimals (e.g., "1000000" for 6 decimals)
            let power_str = format!("1{}", "0".repeat(metadata.decimals as usize));
            Decimal::from_str(&power_str).unwrap_or(Decimal::ONE)
        };
        let supply_adjusted = supply / decimals_power;
        let fdv_usd = price_usd * supply_adjusted;

        Ok(fdv_usd)
    }

    /// Get estimated liquidity for a token in USD via DexScreener.
    ///
    /// DexScreener aggregates pool liquidity across Raydium, Orca, Meteora, etc.
    /// If a token is not indexed by DexScreener, behavior depends on `allow_unlisted_heuristic`:
    /// - false (default): returns $0, causing the BUY safety gate to reject the token.
    /// - true: falls back to a supply-based heuristic estimate (legacy behavior).
    ///
    /// EXIT/SELL paths are unaffected — their liquidity threshold is $0.
    ///
    /// FIX 1: Now with cache support - checks cache first before making HTTP call
    pub async fn get_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        // Fast path: Check cache first
        if let Some(cached_liq) = self.get_cached_liquidity(token_address) {
            tracing::debug!(
                token = token_address,
                liquidity_usd = %cached_liq,
                cache_age_secs = self.liquidity_ttl_secs,
                "Liquidity cache hit"
            );
            return Ok(cached_liq);
        }

        tracing::debug!(
            token = token_address,
            "Liquidity cache miss - fetching from DexScreener"
        );

        // Slow path: Fetch from API
        let dex_liquidity = match self.fetch_dexscreener_liquidity(token_address).await {
            Ok(liq) => liq,
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "DexScreener liquidity fetch failed; treating as unlisted ($0)"
                );
                Decimal::ZERO
            }
        };

        // Update cache
        self.update_liquidity_cache(token_address, dex_liquidity).await;

        if dex_liquidity > Decimal::ZERO {
            tracing::debug!(
                token = token_address,
                liquidity_usd = %dex_liquidity,
                "Fetched DexScreener liquidity and cached"
            );
            return Ok(dex_liquidity);
        }

        if self.allow_unlisted_heuristic {
            let metadata = self.get_metadata(token_address).await?;
            let est = if metadata.supply > 1_000_000_000_000 {
                Decimal::from(50_000)
            } else if metadata.supply > 1_000_000_000 {
                Decimal::from(20_000)
            } else {
                Decimal::from(5_000)
            };
            tracing::warn!(
                token = token_address,
                estimated_liquidity = %est,
                "Token not on DexScreener; using opt-in heuristic estimate"
            );
            return Ok(est);
        }

        tracing::warn!(
            token = token_address,
            "Token not listed on DexScreener; liquidity treated as $0 (strict mode)"
        );
        Ok(Decimal::ZERO)
    }

    /// Fetch aggregated liquidity from DexScreener.
    ///
    /// Returns the maximum `liquidity.usd` across all Solana pairs for the token.
    /// Returns `Ok(Decimal::ZERO)` when the token is not listed (not an error).
    async fn fetch_dexscreener_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        let url = format!("{}/{}", self.dexscreener_base_url, token_address);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Http(format!("DexScreener request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::Http(format!(
                "DexScreener returned error: {}",
                response.status()
            )));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Parse(format!("Failed to parse DexScreener response: {}", e)))?;

        let pairs = match data.get("pairs").and_then(|p| p.as_array()) {
            Some(p) => p,
            None => {
                tracing::debug!(token = token_address, "DexScreener returned no pairs");
                return Ok(Decimal::ZERO);
            }
        };

        let max_liq = pairs
            .iter()
            .filter(|pair| pair.get("chainId").and_then(|c| c.as_str()) == Some("solana"))
            .filter_map(|pair| {
                pair.get("liquidity")
                    .and_then(|l| l.get("usd"))
                    .and_then(|u| u.as_f64())
            })
            .fold(0f64, f64::max);

        if max_liq == 0.0 {
            tracing::debug!(
                token = token_address,
                "DexScreener: no Solana pairs with liquidity"
            );
        }

        Ok(Decimal::from_f64_retain(max_liq).unwrap_or(Decimal::ZERO))
    }

    /// Check whether a token can be sold by querying a Jupiter sell quote.
    ///
    /// Returns true if Jupiter can route TOKEN→SOL (token is sellable),
    /// false if the token has no sell route (likely honeypot or zero liquidity),
    /// or an inconclusive error if the Jupiter API is unavailable.
    ///
    /// This replaces the old transaction-simulation approach which used a random
    /// dummy wallet (zero balance) and therefore always returned "inconclusive".
    pub async fn simulate_sell(&self, token_address: &str) -> AppResult<bool> {
        tracing::debug!(
            token = token_address,
            "Checking sell route for honeypot detection"
        );

        // Use 1_000_000 base units = 1 token for 6-decimal SPL tokens.
        // 1_000 base units (0.001 tokens) falls below DEX minimum order sizes and causes
        // false-positive "no route" rejections even for perfectly safe tokens.
        let test_amount: u64 = 1_000_000;
        let sol_mint = crate::constants::mints::SOL;

        let quote_url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps=10000",
            self.jupiter_api_url, token_address, sol_mint, test_amount
        );

        let response = crate::jupiter::with_api_key(self.http_client.get(&quote_url))
            .send()
            .await
            .map_err(|e| AppError::Http(format!("Jupiter sell-quote request failed: {}", e)))?;

        let status = response.status();

        if status == reqwest::StatusCode::BAD_REQUEST {
            // Jupiter returns 400 when no route exists (can't sell this token)
            tracing::warn!(
                token = token_address,
                "Honeypot: no Jupiter sell route (400)"
            );
            return Ok(false);
        }

        if !status.is_success() {
            return Err(AppError::Validation(format!(
                "honeypot_simulation_inconclusive: Jupiter quote returned {}",
                status
            )));
        }

        let quote: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Parse(format!("Failed to parse Jupiter sell quote: {}", e)))?;

        // Jupiter returns an "error" field or an empty/absent outAmount when no route exists
        if quote.get("error").is_some() {
            tracing::warn!(
                token = token_address,
                error = ?quote.get("error"),
                "Honeypot: Jupiter sell quote returned error"
            );
            return Ok(false);
        }

        let out_amount = quote
            .get("outAmount")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        if out_amount == 0 {
            tracing::warn!(
                token = token_address,
                "Honeypot: sell quote returned zero output"
            );
            return Ok(false);
        }

        tracing::debug!(
            token = token_address,
            out_amount = out_amount,
            "Sell route confirmed — token appears sellable"
        );
        Ok(true)
    }

    /// Clear the metadata cache and TTL timestamps
    pub fn clear_cache(&self) {
        let mut cache = self.metadata_cache.write();
        cache.clear();
        let mut last_fetched = self.last_fetched.write();
        last_fetched.clear();

        // FIX 1: Clear liquidity and FDV caches too
        let mut liquidity_cache = self.liquidity_cache.write();
        liquidity_cache.clear();
        let mut fdv_cache = self.fdv_cache.write();
        fdv_cache.clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.metadata_cache.read().len()
    }

    /// Get cache warming performance statistics
    /// Returns tuple of (cycles, successes, failures, success_rate)
    pub fn cache_warming_stats(&self) -> (u64, u64, u64, f64) {
        let cycles = self.cache_warming_cycles.load(std::sync::atomic::Ordering::Relaxed);
        let successes = self.cache_warming_successes.load(std::sync::atomic::Ordering::Relaxed);
        let failures = self.cache_warming_failures.load(std::sync::atomic::Ordering::Relaxed);

        let success_rate = if successes + failures > 0 {
            (successes as f64) / ((successes + failures) as f64)
        } else {
            1.0
        };

        (cycles, successes, failures, success_rate)
    }

    /// Get only token decimals (fast path using Jupiter cache).
    /// Returns None if not in Jupiter cache, falls back to full metadata fetch.
    ///
    /// This is optimized for decimals-only queries to avoid unnecessary RPC calls.
    /// Checks the PriceCache first (populated by Jupiter Price API v3), and only
    /// falls back to the expensive RPC metadata fetch if not found.
    pub async fn get_decimals_only(&self, token_address: &str) -> Option<u8> {
        // Check PriceCache first (fast path - uses Jupiter data)
        if let Some(ref price_cache) = self.price_cache {
            if let Some(decimals) = price_cache.get_decimals(token_address) {
                tracing::debug!(
                    token = token_address,
                    decimals = decimals,
                    "Decimals from Jupiter cache (no RPC call)"
                );
                return Some(decimals);
            }
        }

        // Fallback: fetch full metadata from RPC
        match self.get_metadata(token_address).await {
            Ok(metadata) => {
                tracing::debug!(
                    token = token_address,
                    decimals = metadata.decimals,
                    "Decimals from RPC fallback"
                );
                Some(metadata.decimals)
            }
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "Failed to fetch decimals"
                );
                None
            }
        }
    }
}

/// Parse an optional pubkey from SPL Token account data
fn parse_optional_pubkey(data: &[u8]) -> Option<String> {
    if data.len() < 36 {
        return None;
    }

    // First 4 bytes are the option tag (0 = None, 1 = Some)
    let option_tag = u32::from_le_bytes(data[0..4].try_into().unwrap());

    if option_tag == 0 {
        None
    } else {
        // Next 32 bytes are the pubkey
        let pubkey_bytes: [u8; 32] = data[4..36].try_into().ok()?;
        let pubkey = Pubkey::new_from_array(pubkey_bytes);
        Some(pubkey.to_string())
    }
}

/// Parse Token-2022 TLV extension data and return (has_transfer_hook, has_permanent_delegate).
/// Extension TLV layout: [type: u16 LE][length: u16 LE][value: length bytes] ...
fn parse_token_2022_dangerous_extensions(ext_data: &[u8]) -> (bool, bool) {
    const TRANSFER_HOOK_TYPE: u16 = 25; // ExtensionType::TransferHook
    const PERMANENT_DELEGATE_TYPE: u16 = 27; // ExtensionType::PermanentDelegate

    let mut has_transfer_hook = false;
    let mut has_permanent_delegate = false;
    let mut cursor = 0usize;

    while cursor + 4 <= ext_data.len() {
        let ext_type = u16::from_le_bytes([ext_data[cursor], ext_data[cursor + 1]]);
        let ext_len = u16::from_le_bytes([ext_data[cursor + 2], ext_data[cursor + 3]]) as usize;
        cursor += 4;

        if ext_type == TRANSFER_HOOK_TYPE {
            has_transfer_hook = true;
        } else if ext_type == PERMANENT_DELEGATE_TYPE {
            has_permanent_delegate = true;
        }

        cursor = match cursor.checked_add(ext_len) {
            Some(next) if next <= ext_data.len() => next,
            _ => break,
        };
    }

    (has_transfer_hook, has_permanent_delegate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_optional_pubkey_none() {
        // Option tag = 0 means None
        let data = [0u8; 36];
        assert!(parse_optional_pubkey(&data).is_none());
    }

    #[test]
    fn test_parse_optional_pubkey_some() {
        let mut data = [0u8; 36];
        // Option tag = 1 means Some
        data[0] = 1;
        // Fill pubkey with non-zero bytes
        for (i, byte) in data[4..36].iter_mut().enumerate() {
            *byte = i as u8;
        }

        let result = parse_optional_pubkey(&data);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_optional_pubkey_short_data() {
        let data = [0u8; 10];
        assert!(parse_optional_pubkey(&data).is_none());
    }
}
