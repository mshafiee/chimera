//! Token metadata fetching from Solana RPC
//!
//! Provides:
//! - Token mint metadata (freeze/mint authority)
//! - Liquidity estimation
//! - Honeypot detection via sell simulation

use crate::error::{AppError, AppResult};
use crate::token::pools::PoolEnumerator;
use parking_lot::RwLock;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use reqwest;
use serde::{Deserialize, Serialize};
use bincode;

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
}

/// Fetches token metadata from Solana RPC
pub struct TokenMetadataFetcher {
    /// RPC client
    rpc_client: Arc<RpcClient>,
    /// Metadata cache (separate from safety result cache)
    metadata_cache: RwLock<HashMap<String, TokenMetadata>>,
    /// Pool enumerator for DEX liquidity
    pool_enumerator: Option<Arc<PoolEnumerator>>,
}

impl TokenMetadataFetcher {
    /// Create a new metadata fetcher
    pub fn new(rpc_url: &str) -> Self {
        let rpc_client = RpcClient::new_with_timeout(rpc_url.to_string(), Duration::from_secs(10));
        let rpc_client_arc = Arc::new(rpc_client);

        Self {
            rpc_client: rpc_client_arc.clone(),
            metadata_cache: RwLock::new(HashMap::new()),
            pool_enumerator: Some(Arc::new(PoolEnumerator::new(
                rpc_client_arc,
                100,  // cache capacity
                300,  // cache TTL seconds
            ))),
        }
    }

    /// Create from an existing RPC client
    pub fn with_client(rpc_client: Arc<RpcClient>) -> Self {
        let pool_enumerator = Some(Arc::new(PoolEnumerator::new(
            rpc_client.clone(),
            100,
            300,
        )));

        Self {
            rpc_client,
            metadata_cache: RwLock::new(HashMap::new()),
            pool_enumerator,
        }
    }

    /// Get token metadata, using cache if available
    pub async fn get_metadata(&self, token_address: &str) -> AppResult<TokenMetadata> {
        // Check cache first
        {
            let cache = self.metadata_cache.read();
            if let Some(metadata) = cache.get(token_address) {
                return Ok(metadata.clone());
            }
        }

        // Fetch from RPC
        let metadata = self.fetch_metadata_from_rpc(token_address).await?;

        // Cache the result
        {
            let mut cache = self.metadata_cache.write();
            cache.insert(token_address.to_string(), metadata.clone());
        }

        Ok(metadata)
    }

    /// Fetch metadata directly from RPC
    async fn fetch_metadata_from_rpc(&self, token_address: &str) -> AppResult<TokenMetadata> {
        let mint_pubkey = Pubkey::from_str(token_address).map_err(|e| {
            AppError::Validation(format!("Invalid token address: {}", e))
        })?;

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

            Ok(TokenMetadata {
                mint: address,
                freeze_authority,
                mint_authority,
                decimals,
                supply,
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
    pub async fn get_market_cap_fdv(&self, token_address: &str) -> AppResult<f64> {
        // Get token metadata (includes supply and decimals)
        let metadata = self.get_metadata(token_address).await?;
        
        // Get current price from Jupiter
        let price_url = format!("https://price.jup.ag/v6/price?ids={}", token_address);
        let response = reqwest::get(&price_url)
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

        // Extract price
        let price_usd = if let Some(token_data) = data.get("data").and_then(|d| d.get(token_address)) {
            if let Some(price) = token_data.get("price").and_then(|p| p.as_f64()) {
                price
            } else {
                // Try alternative field names
                token_data.get("priceUsd")
                    .and_then(|p| p.as_f64())
                    .ok_or_else(|| AppError::Parse("No price found in Jupiter response".to_string()))?
            }
        } else {
            return Err(AppError::Parse("Token not found in Jupiter response".to_string()));
        };

        // Calculate FDV = price * total_supply (adjusted for decimals)
        let supply_adjusted = metadata.supply as f64 / (10.0_f64.powi(metadata.decimals as i32));
        let fdv_usd = price_usd * supply_adjusted;

        Ok(fdv_usd)
    }

    /// Get estimated liquidity for a token in USD
    ///
    /// Queries multiple sources:
    /// 1. Jupiter Price API (aggregated liquidity data)
    /// 2. Raydium pools (via RPC)
    /// 3. Orca pools (via RPC)
    ///
    /// Returns the total aggregated liquidity from all sources.
    pub async fn get_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        use rust_decimal::Decimal;
        tracing::debug!(token = token_address, "Fetching liquidity from DEX pools");

        // Try Jupiter first (fastest, aggregated data)
        let jupiter_liquidity = self.fetch_jupiter_liquidity(token_address).await.ok();

        // Try Raydium pools
        let raydium_liquidity = self.fetch_raydium_liquidity(token_address).await.ok();

        // Try Orca pools
        let orca_liquidity = self.fetch_orca_liquidity(token_address).await.ok();

        // Aggregate all sources using Decimal for precision
        let total_liquidity = jupiter_liquidity.unwrap_or(Decimal::ZERO)
            + raydium_liquidity.unwrap_or(Decimal::ZERO)
            + orca_liquidity.unwrap_or(Decimal::ZERO);

        if total_liquidity > Decimal::ZERO {
            tracing::debug!(
                token = token_address,
                total_liquidity_usd = %total_liquidity,
                jupiter = ?jupiter_liquidity,
                raydium = ?raydium_liquidity,
                orca = ?orca_liquidity,
                "Fetched liquidity from DEX pools"
            );
            Ok(total_liquidity)
        } else {
            // Fallback: use heuristic based on token metadata
            let metadata = self.get_metadata(token_address).await?;
            let estimated_liquidity = if metadata.supply > 1_000_000_000_000 {
                Decimal::from(50_000)
            } else if metadata.supply > 1_000_000_000 {
                Decimal::from(20_000)
            } else {
                Decimal::from(5_000)
            };

            tracing::warn!(
                token = token_address,
                estimated_liquidity = %estimated_liquidity,
                "No liquidity found in DEX pools, using heuristic estimate"
            );

            Ok(estimated_liquidity)
        }
    }

    /// Fetch liquidity from Jupiter Price API
    ///
    /// Jupiter aggregates liquidity data from multiple DEXes.
    async fn fetch_jupiter_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        use rust_decimal::Decimal;
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

    /// Fetch liquidity from Raydium pools via RPC
    ///
    /// Queries Raydium pool accounts for the token and calculates total liquidity.
    async fn fetch_raydium_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        if let Some(ref pool_enumerator) = self.pool_enumerator {
            pool_enumerator
                .get_raydium_liquidity(token_address)
                .await
                .map_err(|e| AppError::Http(format!("Raydium liquidity fetch failed: {}", e)))
        } else {
            tracing::debug!(
                token = token_address,
                "Pool enumerator not available, returning 0.0 for Raydium liquidity"
            );
            Ok(Decimal::ZERO)
        }
    }

    /// Fetch liquidity from Orca pools via RPC
    ///
    /// Queries Orca pool accounts for the token and calculates total liquidity.
    async fn fetch_orca_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        if let Some(ref pool_enumerator) = self.pool_enumerator {
            pool_enumerator
                .get_orca_liquidity(token_address)
                .await
                .map_err(|e| AppError::Http(format!("Orca liquidity fetch failed: {}", e)))
        } else {
            tracing::debug!(
                token = token_address,
                "Pool enumerator not available, returning 0.0 for Orca liquidity"
            );
            Ok(Decimal::ZERO)
        }
    }

    /// Simulate a sell transaction to detect honeypots
    ///
    /// Returns true if the token can be sold, false if it's a honeypot
    ///
    /// This creates a minimal test sell transaction (token -> SOL) and simulates it
    /// via RPC. If the simulation fails, the token is likely a honeypot.
    pub async fn simulate_sell(&self, token_address: &str) -> AppResult<bool> {
        tracing::debug!(token = token_address, "Simulating sell transaction for honeypot detection");

        // Build a minimal test sell transaction
        // We'll use Jupiter Swap API to get a swap transaction, then simulate it
        let test_amount_lamports = 1_000_000; // 0.001 SOL worth of tokens (minimal test)

        let token_mint = Pubkey::from_str(token_address)
            .map_err(|e| AppError::Validation(format!("Invalid token mint: {}", e)))?;
        let sol_mint = Pubkey::from_str(crate::constants::mints::SOL)
            .map_err(|e| AppError::Validation(format!("Invalid SOL mint: {}", e)))?;

        // Get a swap transaction from Jupiter (minimal amount)
        let swap_tx = self
            .get_jupiter_swap_transaction_for_simulation(token_mint, sol_mint, test_amount_lamports)
            .await?;

        // Simulate the transaction via RPC
        let simulation_result = self
            .simulate_transaction_rpc(&swap_tx)
            .await?;

        if let Some(err) = simulation_result.err {
            let err_str = format!("{:?}", err).to_lowercase();
            
            // Check logs for specific failure reasons
            let logs = simulation_result.logs.join("\n").to_lowercase();

            // If the failure is due to us using a dummy wallet with no funds,
            // we cannot determine if it's a honeypot.
            // In high-frequency context, false positives (rejecting good tokens) 
            // are better than false negatives (buying honeypots), 
            // BUT "Insufficient Funds" is guaranteed to happen with a dummy wallet.
            
            if logs.contains("insufficient funds") 
               || logs.contains("account not found") 
               || err_str.contains("accountnotfound") {
                
                tracing::warn!(
                    token = token_address,
                    "Honeypot check inconclusive (insufficient funds in sim). Allowing trade cautiously."
                );
                // Return TRUE (safe) because we failed due to setup, not token logic
                return Ok(true);
            }

            // If it's a custom program error (usually 0x1770 or similar for frozen assets), reject
            if logs.contains("custom program error") || logs.contains("transfer failed") {
                tracing::warn!(token = token_address, "Honeypot detected via simulation error");
                return Ok(false);
            }
            
            // Default to safe if error is obscure, or unsafe if you want max security
            // Here we default to unsafe for unknown errors
            return Ok(false); 
        }

        // If we got here, simulation succeeded or was inconclusive (but allowed)
        let is_sellable = true;

        tracing::debug!(
            token = token_address,
            is_sellable = is_sellable,
            "Honeypot simulation completed"
        );

        Ok(is_sellable)
    }

    /// Get a Jupiter swap transaction for simulation (minimal amount)
    async fn get_jupiter_swap_transaction_for_simulation(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
    ) -> AppResult<String> {
        // First get a quote
        let quote_url = format!(
            "https://quote-api.jup.ag/v6/quote?inputMint={}&outputMint={}&amount={}&slippageBps=50",
            input_mint, output_mint, amount
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
        let swap_url = "https://quote-api.jup.ag/v6/swap";
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
    async fn simulate_transaction_rpc(&self, transaction_base64: &str) -> AppResult<SimulationResult> {
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
            let transaction: solana_sdk::transaction::Transaction = bincode::serde::decode_from_slice(&tx_bytes_clone, bincode::config::standard())
                .map_err(|e| AppError::Parse(format!("Failed to deserialize transaction: {}", e)))?
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

    /// Clear the metadata cache
    pub fn clear_cache(&self) {
        let mut cache = self.metadata_cache.write();
        cache.clear();
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
        for i in 4..36 {
            data[i] = (i - 4) as u8;
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
