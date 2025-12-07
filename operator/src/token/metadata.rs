//! Token metadata fetching from Solana RPC
//!
//! Provides:
//! - Token mint metadata (freeze/mint authority)
//! - Liquidity estimation
//! - Honeypot detection via sell simulation

use crate::error::{AppError, AppResult};
use parking_lot::RwLock;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

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
}

impl TokenMetadataFetcher {
    /// Create a new metadata fetcher
    pub fn new(rpc_url: &str) -> Self {
        let rpc_client = RpcClient::new_with_timeout(rpc_url.to_string(), Duration::from_secs(10));

        Self {
            rpc_client: Arc::new(rpc_client),
            metadata_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Create from an existing RPC client
    pub fn with_client(rpc_client: Arc<RpcClient>) -> Self {
        Self {
            rpc_client,
            metadata_cache: RwLock::new(HashMap::new()),
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

    /// Get estimated liquidity for a token in USD
    ///
    /// This is a simplified implementation that returns a placeholder.
    /// In production, you would query DEX pools (Raydium, Orca, etc.)
    pub async fn get_liquidity(&self, token_address: &str) -> AppResult<f64> {
        // TODO: Implement actual liquidity fetching from DEX pools
        // For now, we'll use a simple heuristic based on token metadata

        let metadata = self.get_metadata(token_address).await?;

        // Very basic heuristic: estimate based on supply
        // This should be replaced with actual DEX pool queries
        let estimated_liquidity = if metadata.supply > 1_000_000_000_000 {
            // High supply tokens often have liquidity
            50_000.0
        } else if metadata.supply > 1_000_000_000 {
            20_000.0
        } else {
            5_000.0
        };

        tracing::debug!(
            token = token_address,
            estimated_liquidity = estimated_liquidity,
            "Estimated token liquidity (placeholder)"
        );

        Ok(estimated_liquidity)
    }

    /// Simulate a sell transaction to detect honeypots
    ///
    /// Returns true if the token can be sold, false if it's a honeypot
    pub async fn simulate_sell(&self, token_address: &str) -> AppResult<bool> {
        // TODO: Implement actual transaction simulation
        // This would:
        // 1. Create a swap transaction (token -> SOL)
        // 2. Call simulateTransaction RPC method
        // 3. Check if simulation succeeds

        let _metadata = self.get_metadata(token_address).await?;

        // For now, return true (assume sellable)
        // In production, implement actual simulation
        tracing::debug!(
            token = token_address,
            "Honeypot simulation (placeholder - assuming sellable)"
        );

        Ok(true)
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
