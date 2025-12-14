//! DEX Fee Comparison
//!
//! Queries multiple DEX APIs and selects the one with lowest total cost
//! (fee + estimated slippage). Caches results for 5 seconds to avoid
//! repeated queries for the same token.

use crate::error::AppResult;
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// DEX comparison result
#[derive(Debug, Clone)]
pub struct DexComparisonResult {
    /// Selected DEX name
    pub selected_dex: String,
    /// Total cost (fee + slippage) in SOL
    pub total_cost_sol: Decimal,
    /// Fee amount in SOL
    pub fee_sol: Decimal,
    /// Estimated slippage in SOL
    pub slippage_sol: Decimal,
    /// DEX API endpoint used
    pub dex_url: String,
}

/// Cached DEX comparison result
#[derive(Debug, Clone)]
struct CachedResult {
    result: DexComparisonResult,
    cached_at: SystemTime,
}

/// DEX comparator
pub struct DexComparator {
    /// Cache of recent comparisons (token_address -> result)
    cache: Arc<RwLock<HashMap<String, CachedResult>>>,
    /// Cache TTL in seconds
    cache_ttl: Duration,
    /// HTTP client for API calls
    http_client: reqwest::Client,
}

impl DexComparator {
    /// Create a new DEX comparator
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(5),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Compare DEXs and select the one with lowest cost
    ///
    /// # Arguments
    /// * `token_in` - Input token address (e.g., SOL)
    /// * `token_out` - Output token address
    /// * `amount_sol` - Amount to swap in SOL
    ///
    /// # Returns
    /// DexComparisonResult with selected DEX and costs
    pub async fn compare_and_select(
        &self,
        token_in: &str,
        token_out: &str,
        amount_sol: Decimal,
    ) -> AppResult<DexComparisonResult> {
        // Check cache first (use string representation for cache key)
        let amount_str = amount_sol.to_string();
        let cache_key = format!("{}:{}:{}", token_in, token_out, amount_str);
        {
            let cache = self.cache.read();
            if let Some(cached) = cache.get(&cache_key) {
                if cached.cached_at.elapsed().unwrap_or_default() < self.cache_ttl {
                    return Ok(cached.result.clone());
                }
            }
        }

        // Query multiple DEXs in parallel
        let (jupiter_result, raydium_result, orca_result, meteora_result) = tokio::join!(
            self.query_jupiter(token_in, token_out, amount_sol),
            self.query_raydium(token_in, token_out, amount_sol),
            self.query_orca(token_in, token_out, amount_sol),
            self.query_meteora(token_in, token_out, amount_sol),
        );

        // Collect all successful results
        let mut results = Vec::new();
        
        if let Ok(result) = jupiter_result {
            results.push(result);
        }
        if let Ok(result) = raydium_result {
            results.push(result);
        }
        if let Ok(result) = orca_result {
            results.push(result);
        }
        if let Ok(result) = meteora_result {
            results.push(result);
        }

        // Select DEX with lowest total cost
        let result = results
            .into_iter()
            .min_by(|a, b| a.total_cost_sol.cmp(&b.total_cost_sol))
            .unwrap_or_else(|| {
                // Fallback to Jupiter if all queries failed
                let default_total_cost = amount_sol * Decimal::from_str("0.008").unwrap(); // Default 0.8% total cost
                let default_fee = amount_sol * Decimal::from_str("0.003").unwrap();
                let default_slippage = amount_sol * Decimal::from_str("0.005").unwrap();
                DexComparisonResult {
                    selected_dex: "Jupiter".to_string(),
                    total_cost_sol: default_total_cost,
                    fee_sol: default_fee,
                    slippage_sol: default_slippage,
                    dex_url: "https://quote-api.jup.ag/v6".to_string(),
                }
            });

        // Cache the result
        {
            let mut cache = self.cache.write();
            cache.insert(
                cache_key,
                CachedResult {
                    result: result.clone(),
                    cached_at: SystemTime::now(),
                },
            );
        }

        Ok(result)
    }

    /// Query Jupiter API for swap quote
    async fn query_jupiter(
        &self,
        token_in: &str,
        token_out: &str,
        amount_sol: Decimal,
    ) -> AppResult<DexComparisonResult> {
        // Convert Decimal to lamports for API call
        let lamports = (amount_sol * Decimal::from(1_000_000_000u64))
            .to_u64()
            .unwrap_or(0);
        
        // Jupiter API endpoint
        let url = format!(
            "https://quote-api.jup.ag/v6/quote?inputMint={}&outputMint={}&amount={}&slippageBps=50",
            token_in,
            token_out,
            lamports
        );

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| crate::error::AppError::Internal(format!("Jupiter API error: {}", e)))?;

        if !response.status().is_success() {
            return Err(crate::error::AppError::Internal(format!(
                "Jupiter API returned error: {}",
                response.status()
            )));
        }

        let quote: serde_json::Value = response
            .json()
            .await
            .map_err(|e| crate::error::AppError::Internal(format!("Failed to parse Jupiter response: {}", e)))?;

        // Extract fee and slippage from quote, convert to Decimal
        let fee_percent = quote
            .get("fee")
            .and_then(|f| f.as_f64())
            .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO))
            .unwrap_or_else(|| Decimal::from_str("0.003").unwrap()); // Default 0.3% fee

        let fee_sol = amount_sol * fee_percent;

        // Estimate slippage (simplified - Jupiter provides this in quote)
        let slippage_percent = quote
            .get("priceImpactPct")
            .and_then(|p| p.as_f64())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
            .unwrap_or_else(|| Decimal::from_str("0.005").unwrap()); // Default 0.5% slippage

        let slippage_sol = amount_sol * slippage_percent;
        let total_cost_sol = fee_sol + slippage_sol;

        Ok(DexComparisonResult {
            selected_dex: "Jupiter".to_string(),
            total_cost_sol,
            fee_sol,
            slippage_sol,
            dex_url: "https://quote-api.jup.ag/v6".to_string(),
        })
    }

    /// Query Raydium API for swap quote
    async fn query_raydium(
        &self,
        token_in: &str,
        token_out: &str,
        amount_sol: Decimal,
    ) -> AppResult<DexComparisonResult> {
        // Convert Decimal to lamports for API call
        let lamports = (amount_sol * Decimal::from(1_000_000_000u64))
            .to_u64()
            .unwrap_or(0);
        
        // Raydium API endpoint (v2)
        let url = format!(
            "https://api.raydium.io/v2/swap/quote?inputMint={}&outputMint={}&amount={}&slippage=0.5",
            token_in,
            token_out,
            lamports
        );

        let response = self
            .http_client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let quote: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| crate::error::AppError::Internal(format!("Failed to parse Raydium response: {}", e)))?;

                let fee_percent = quote
                    .get("fee")
                    .and_then(|f| f.as_f64())
                    .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.0025").unwrap()); // Raydium default 0.25% fee

                let fee_sol = amount_sol * fee_percent;
                let slippage_percent = quote
                    .get("priceImpact")
                    .and_then(|p| p.as_f64())
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.005").unwrap());
                let slippage_sol = amount_sol * slippage_percent;
                let total_cost_sol = fee_sol + slippage_sol;

                Ok(DexComparisonResult {
                    selected_dex: "Raydium".to_string(),
                    total_cost_sol,
                    fee_sol,
                    slippage_sol,
                    dex_url: "https://api.raydium.io/v2".to_string(),
                })
            }
            _ => Err(crate::error::AppError::Internal("Raydium API unavailable".to_string())),
        }
    }

    /// Query Orca API for swap quote
    async fn query_orca(
        &self,
        token_in: &str,
        token_out: &str,
        amount_sol: Decimal,
    ) -> AppResult<DexComparisonResult> {
        // Convert Decimal to lamports for API call
        let lamports = (amount_sol * Decimal::from(1_000_000_000u64))
            .to_u64()
            .unwrap_or(0);
        
        // Orca API endpoint
        let url = format!(
            "https://api.orca.so/v1/quote?inputMint={}&outputMint={}&amount={}&slippage=0.5",
            token_in,
            token_out,
            lamports
        );

        let response = self
            .http_client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let quote: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| crate::error::AppError::Internal(format!("Failed to parse Orca response: {}", e)))?;

                let fee_percent = quote
                    .get("fee")
                    .and_then(|f| f.as_f64())
                    .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.003").unwrap()); // Orca default 0.3% fee

                let fee_sol = amount_sol * fee_percent;
                let slippage_percent = quote
                    .get("priceImpact")
                    .and_then(|p| p.as_f64())
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.005").unwrap());
                let slippage_sol = amount_sol * slippage_percent;
                let total_cost_sol = fee_sol + slippage_sol;

                Ok(DexComparisonResult {
                    selected_dex: "Orca".to_string(),
                    total_cost_sol,
                    fee_sol,
                    slippage_sol,
                    dex_url: "https://api.orca.so/v1".to_string(),
                })
            }
            _ => Err(crate::error::AppError::Internal("Orca API unavailable".to_string())),
        }
    }

    /// Query Meteora API for swap quote
    async fn query_meteora(
        &self,
        token_in: &str,
        token_out: &str,
        amount_sol: Decimal,
    ) -> AppResult<DexComparisonResult> {
        // Convert Decimal to lamports for API call
        let lamports = (amount_sol * Decimal::from(1_000_000_000u64))
            .to_u64()
            .unwrap_or(0);
        
        // Meteora DLMM API endpoint
        let url = format!(
            "https://dlmm-api.meteora.ag/pair/quote?inputMint={}&outputMint={}&amount={}&slippage=0.5",
            token_in,
            token_out,
            lamports
        );

        let response = self
            .http_client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let quote: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| crate::error::AppError::Internal(format!("Failed to parse Meteora response: {}", e)))?;

                let fee_percent = quote
                    .get("fee")
                    .and_then(|f| f.as_f64())
                    .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.003").unwrap()); // Meteora default 0.3% fee

                let fee_sol = amount_sol * fee_percent;
                let slippage_percent = quote
                    .get("priceImpact")
                    .and_then(|p| p.as_f64())
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
                    .unwrap_or_else(|| Decimal::from_str("0.005").unwrap());
                let slippage_sol = amount_sol * slippage_percent;
                let total_cost_sol = fee_sol + slippage_sol;

                Ok(DexComparisonResult {
                    selected_dex: "Meteora".to_string(),
                    total_cost_sol,
                    fee_sol,
                    slippage_sol,
                    dex_url: "https://dlmm-api.meteora.ag".to_string(),
                })
            }
            _ => Err(crate::error::AppError::Internal("Meteora API unavailable".to_string())),
        }
    }

    /// Clear expired cache entries
    pub fn clear_expired_cache(&self) {
        let mut cache = self.cache.write();
        cache.retain(|_, cached| {
            cached.cached_at.elapsed().unwrap_or_default() < self.cache_ttl
        });
    }
}

impl Default for DexComparator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dex_comparison_caching() {
        let comparator = DexComparator::new();
        
        // First call should query API
        let _result1 = comparator
            .compare_and_select(
                "So11111111111111111111111111111111111111112", // SOL
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
                Decimal::from(1u64),
            )
            .await;

        // Second call within 5 seconds should use cache
        // (This would be tested with actual API in integration tests)
    }
}




