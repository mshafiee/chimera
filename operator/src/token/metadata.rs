//! Token metadata fetching from Solana RPC
//!
//! Provides:
//! - Token mint metadata (freeze/mint authority)
//! - Liquidity estimation
//! - Honeypot detection via sell simulation

use crate::error::{AppError, AppResult};
use crate::monitoring::rate_limiter::{RateLimiter, RequestPriority, RequestWeight};
use crate::token::pools::PoolEnumerator;
use bincode;
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

/// Transaction simulation result
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimulationResult {
    /// Error if simulation failed
    err: Option<serde_json::Value>,
    /// Transaction logs
    logs: Vec<String>,
    /// Compute units consumed
    units_consumed: Option<u64>,
}

/// Token metadata from on-chain
#[derive(Debug, Clone)]
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
}

/// Fetches token metadata from Solana RPC
pub struct TokenMetadataFetcher {
    /// RPC client
    rpc_client: Arc<RpcClient>,
    /// Metadata cache (separate from safety result cache)
    metadata_cache: RwLock<HashMap<String, TokenMetadata>>,
    /// FIX 12: Tracks when each token was last fetched for TTL-based cache eviction
    last_fetched: RwLock<HashMap<String, Instant>>,
    /// TTL for cached metadata entries (default: 1 hour)
    cache_ttl: Duration,
    /// Pool enumerator for DEX liquidity
    pool_enumerator: Option<Arc<PoolEnumerator>>,
    /// Optional rate limiter for RPC calls (simulation calls use higher weight)
    rate_limiter: Option<Arc<RateLimiter>>,
    /// Jupiter API base URL (e.g., https://api.jup.ag/swap/v1 or https://lite-api.jup.ag/swap/v1)
    jupiter_api_url: String,
    /// HTTP client for DexScreener API calls
    http_client: reqwest::Client,
    /// DexScreener base URL for liquidity queries
    dexscreener_base_url: String,
    /// When true, use supply heuristic for tokens not indexed by DexScreener
    allow_unlisted_heuristic: bool,
}

impl TokenMetadataFetcher {
    /// Create a new metadata fetcher
    pub fn new(rpc_url: &str) -> Self {
        Self::new_with_rate_limiter(rpc_url, None)
    }

    /// Create a new metadata fetcher with optional rate limiter
    pub fn new_with_rate_limiter(rpc_url: &str, rate_limiter: Option<Arc<RateLimiter>>) -> Self {
        Self::new_with_rate_limiter_and_jupiter(
            rpc_url,
            rate_limiter,
            "https://api.jup.ag/swap/v1".to_string(),
        )
    }

    /// Create a new metadata fetcher with optional rate limiter and Jupiter API URL
    pub fn new_with_rate_limiter_and_jupiter(
        rpc_url: &str,
        rate_limiter: Option<Arc<RateLimiter>>,
        jupiter_api_url: String,
    ) -> Self {
        let rpc_client = RpcClient::new_with_timeout(rpc_url.to_string(), Duration::from_secs(10));
        let rpc_client_arc = Arc::new(rpc_client);

        Self {
            rpc_client: rpc_client_arc.clone(),
            metadata_cache: RwLock::new(HashMap::new()),
            last_fetched: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(3600),
            pool_enumerator: Some(Arc::new(PoolEnumerator::new(
                rpc_client_arc,
                100, // cache capacity
                300, // cache TTL seconds
            ))),
            rate_limiter,
            jupiter_api_url,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            dexscreener_base_url: "https://api.dexscreener.com/latest/dex/tokens".to_string(),
            allow_unlisted_heuristic: false,
        }
    }

    /// Create from an existing RPC client
    pub fn with_client(rpc_client: Arc<RpcClient>) -> Self {
        Self::with_client_and_rate_limiter(rpc_client, None)
    }

    /// Create from an existing RPC client with optional rate limiter
    pub fn with_client_and_rate_limiter(
        rpc_client: Arc<RpcClient>,
        rate_limiter: Option<Arc<RateLimiter>>,
    ) -> Self {
        Self::with_client_rate_limiter_and_jupiter(
            rpc_client,
            rate_limiter,
            "https://api.jup.ag/swap/v1".to_string(),
        )
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
            metadata_cache: RwLock::new(HashMap::new()),
            last_fetched: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(3600),
            pool_enumerator,
            rate_limiter,
            jupiter_api_url,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            dexscreener_base_url: "https://api.dexscreener.com/latest/dex/tokens".to_string(),
            allow_unlisted_heuristic: false,
        }
    }

    /// Set whether to use supply heuristic for tokens not indexed by DexScreener.
    /// Default is false (strict mode — unlisted tokens are rejected).
    pub fn with_unlisted_heuristic(mut self, allow: bool) -> Self {
        self.allow_unlisted_heuristic = allow;
        self
    }

    /// Get token metadata, using cache if available and not stale (FIX 12: TTL eviction)
    pub async fn get_metadata(&self, token_address: &str) -> AppResult<TokenMetadata> {
        // Check cache first; evict if TTL has expired
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

        // Fetch from RPC
        let metadata = self.fetch_metadata_from_rpc(token_address).await?;

        // Update cache with fresh timestamp
        {
            let mut cache = self.metadata_cache.write();
            let mut last_fetched = self.last_fetched.write();
            cache.insert(token_address.to_string(), metadata.clone());
            last_fetched.insert(token_address.to_string(), Instant::now());
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
            const TOKEN_2022_PROGRAM: &str =
                "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
            let is_token_2022 = account
                .owner
                .to_string()
                .as_str()
                == TOKEN_2022_PROGRAM;

            let (has_transfer_hook, has_permanent_delegate) =
                if is_token_2022 && data.len() > 82 {
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
    pub async fn get_market_cap_fdv(&self, token_address: &str) -> AppResult<Decimal> {
        tokio::time::timeout(Duration::from_secs(10), self.get_market_cap_fdv_inner(token_address))
            .await
            .unwrap_or_else(|_| {
                tracing::warn!(
                    token = token_address,
                    "get_market_cap_fdv timed out after 10s"
                );
                Err(AppError::Http("slow check timeout".to_string()))
            })
    }

    /// Inner implementation for get_market_cap_fdv (called under timeout)
    async fn get_market_cap_fdv_inner(&self, token_address: &str) -> AppResult<Decimal> {
        // Get token metadata (includes supply and decimals)
        let metadata = self.get_metadata(token_address).await?;

        // Get current price from Jupiter (v2 API) — use pre-built client (FIX 5)
        let price_url = format!("https://lite-api.jup.ag/price/v2?ids={}", token_address);
        let response = self
            .http_client
            .get(&price_url)
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
    pub async fn get_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        let dex_liquidity = self
            .fetch_dexscreener_liquidity(token_address)
            .await
            .unwrap_or(Decimal::ZERO); // network failure → treat as unlisted

        if dex_liquidity > Decimal::ZERO {
            tracing::debug!(
                token = token_address,
                liquidity_usd = %dex_liquidity,
                "Fetched DexScreener liquidity"
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

    /// Fetch liquidity from Jupiter Price API
    ///
    /// Note: Jupiter's price endpoint does not return a liquidity field; this always
    /// returns `Decimal::ZERO`. Superseded by `fetch_dexscreener_liquidity`.
    #[allow(dead_code)]
    async fn fetch_jupiter_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        let url = format!("https://price.jup.ag/v6/price?ids={}", token_address);

        let response = reqwest::get(&url)
            .await
            .map_err(|e| AppError::Http(format!("Jupiter liquidity request failed: {}", e)))?;

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

        // Extract liquidity from response
        // Jupiter Price API may include liquidity data in the response
        // For now, we'll use a placeholder - Jupiter's actual liquidity endpoint may differ
        // In production, check Jupiter's API documentation for liquidity fields

        // Try to extract liquidity from response
        if let Some(token_data) = data.get("data").and_then(|d| d.get(token_address)) {
            // Check for liquidity fields (may vary by API version)
            if let Some(liq) = token_data.get("liquidity").and_then(|l| l.as_f64()) {
                return Ok(Decimal::from_f64_retain(liq).unwrap_or(Decimal::ZERO));
            }
        }

        // If no liquidity field found, return 0 (will be aggregated with other sources)
        Ok(Decimal::ZERO)
    }

    /// Fetch liquidity from Raydium pools via RPC.
    ///
    /// On-chain pool parsing is not implemented. Delegates to `PoolEnumerator` which
    /// returns an error; callers should use `fetch_dexscreener_liquidity` instead.
    #[allow(dead_code)]
    async fn fetch_raydium_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        if let Some(ref pool_enumerator) = self.pool_enumerator {
            pool_enumerator
                .get_raydium_liquidity(token_address)
                .await
                .map_err(|e| AppError::Http(format!("Raydium liquidity unavailable: {}", e)))
        } else {
            Err(AppError::Http(format!(
                "Pool enumerator not available for Raydium liquidity ({}); use DexScreener",
                token_address
            )))
        }
    }

    /// Fetch liquidity from Orca pools via RPC.
    ///
    /// On-chain pool parsing is not implemented. Delegates to `PoolEnumerator` which
    /// returns an error; callers should use `fetch_dexscreener_liquidity` instead.
    #[allow(dead_code)]
    async fn fetch_orca_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        if let Some(ref pool_enumerator) = self.pool_enumerator {
            pool_enumerator
                .get_orca_liquidity(token_address)
                .await
                .map_err(|e| AppError::Http(format!("Orca liquidity unavailable: {}", e)))
        } else {
            Err(AppError::Http(format!(
                "Pool enumerator not available for Orca liquidity ({}); use DexScreener",
                token_address
            )))
        }
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
        tracing::debug!(token = token_address, "Checking sell route for honeypot detection");

        // Use 1_000_000 base units = 1 token for 6-decimal SPL tokens.
        // 1_000 base units (0.001 tokens) falls below DEX minimum order sizes and causes
        // false-positive "no route" rejections even for perfectly safe tokens.
        let test_amount: u64 = 1_000_000;
        let sol_mint = crate::constants::mints::SOL;

        let quote_url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps=10000",
            self.jupiter_api_url, token_address, sol_mint, test_amount
        );

        let response = self.http_client.get(&quote_url).send().await.map_err(|e| {
            AppError::Http(format!("Jupiter sell-quote request failed: {}", e))
        })?;

        let status = response.status();

        if status == reqwest::StatusCode::BAD_REQUEST {
            // Jupiter returns 400 when no route exists (can't sell this token)
            tracing::warn!(token = token_address, "Honeypot: no Jupiter sell route (400)");
            return Ok(false);
        }

        if !status.is_success() {
            return Err(AppError::Validation(format!(
                "honeypot_simulation_inconclusive: Jupiter quote returned {}",
                status
            )));
        }

        let quote: serde_json::Value = response.json().await.map_err(|e| {
            AppError::Parse(format!("Failed to parse Jupiter sell quote: {}", e))
        })?;

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
            tracing::warn!(token = token_address, "Honeypot: sell quote returned zero output");
            return Ok(false);
        }

        tracing::debug!(
            token = token_address,
            out_amount = out_amount,
            "Sell route confirmed — token appears sellable"
        );
        Ok(true)
    }

    /// Get a Jupiter swap transaction for simulation (minimal amount)
    #[allow(dead_code)]
    async fn get_jupiter_swap_transaction_for_simulation(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
    ) -> AppResult<String> {
        // First get a quote (using configured URL, migrated from deprecated v6)
        let quote_url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps=50",
            self.jupiter_api_url, input_mint, output_mint, amount
        );

        let quote_response = reqwest::get(&quote_url)
            .await
            .map_err(|e| AppError::Http(format!("Jupiter quote request failed: {}", e)))?;

        if !quote_response.status().is_success() {
            return Err(AppError::Http(format!(
                "Jupiter quote API returned error: {}",
                quote_response.status()
            )));
        }

        let quote: serde_json::Value = quote_response
            .json()
            .await
            .map_err(|e| AppError::Parse(format!("Failed to parse Jupiter quote: {}", e)))?;

        // Get swap transaction
        // Note: For simulation, we don't need a real wallet - we can use a dummy pubkey
        let dummy_wallet = Pubkey::new_unique();
        let swap_url = format!("{}/swap", self.jupiter_api_url);
        let payload = serde_json::json!({
            "quoteResponse": quote,
            "userPublicKey": dummy_wallet.to_string(),
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": "auto"
        });

        let client = reqwest::Client::new();
        let swap_response = client
            .post(swap_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::Http(format!("Jupiter swap request failed: {}", e)))?;

        if !swap_response.status().is_success() {
            return Err(AppError::Http(format!(
                "Jupiter swap API returned error: {}",
                swap_response.status()
            )));
        }

        let swap_data: serde_json::Value = swap_response
            .json()
            .await
            .map_err(|e| AppError::Parse(format!("Failed to parse Jupiter swap: {}", e)))?;

        // Extract swap transaction (base64 encoded)
        let swap_tx = swap_data
            .get("swapTransaction")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Parse("No swapTransaction in Jupiter response".to_string()))?;

        Ok(swap_tx.to_string())
    }

    /// Simulate a transaction via RPC
    #[allow(dead_code)]
    async fn simulate_transaction_rpc(
        &self,
        transaction_base64: &str,
    ) -> AppResult<SimulationResult> {
        // Rate limit simulation calls (they are heavier than standard RPC calls)
        // Simulation calls typically count 5-10x more towards rate limits on Helius/RPC providers
        if let Some(ref rate_limiter) = self.rate_limiter {
            rate_limiter
                .acquire(RequestPriority::Entry, RequestWeight::SIMULATION)
                .await;
        }

        // Decode base64 transaction
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        let tx_bytes = BASE64
            .decode(transaction_base64)
            .map_err(|e| AppError::Parse(format!("Failed to decode transaction: {}", e)))?;

        // Clone RPC client for blocking call
        let rpc_client = self.rpc_client.clone();
        let tx_bytes_clone = tx_bytes.clone();

        // Run simulation in blocking task
        let result = tokio::task::spawn_blocking(move || {
            // Deserialize transaction
            let transaction: solana_sdk::transaction::Transaction =
                bincode::serde::decode_from_slice(&tx_bytes_clone, bincode::config::standard())
                    .map_err(|e| {
                        AppError::Parse(format!("Failed to deserialize transaction: {}", e))
                    })?
                    .0;

            // Use Solana RPC client's simulate_transaction method
            rpc_client
                .simulate_transaction(&transaction)
                .map_err(|e| AppError::Rpc(format!("Simulation failed: {}", e)))
        })
        .await
        .map_err(|e| AppError::Internal(format!("Task join error: {}", e)))??;

        // Parse simulation result
        // Convert TransactionError to JSON Value if present
        let err_value = result.value.err.as_ref().map(|e| {
            // TransactionError doesn't implement Serialize, so convert to string representation
            serde_json::json!({
                "error": format!("{:?}", e)
            })
        });

        let simulation_result = SimulationResult {
            err: err_value,
            logs: result.value.logs.unwrap_or_default(),
            units_consumed: result.value.units_consumed,
        };

        Ok(simulation_result)
    }

    /// Clear the metadata cache and TTL timestamps
    pub fn clear_cache(&self) {
        let mut cache = self.metadata_cache.write();
        cache.clear();
        let mut last_fetched = self.last_fetched.write();
        last_fetched.clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.metadata_cache.read().len()
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
    const TRANSFER_HOOK_TYPE: u16 = 25;    // ExtensionType::TransferHook
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
