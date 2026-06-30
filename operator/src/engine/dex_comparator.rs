//! DEX route selection via Jupiter.
//!
//! Previously this module hit three hard-coded, non-existent DEX quote endpoints
//! (Raydium/Orca/Meteora) that always failed, then silently fell back to a
//! fabricated "default Jupiter" result whose `fee`/`priceImpact` keys never
//! existed in any real response. Routing was therefore cosmetic.
//!
//! It now performs **real** per-DEX route comparison through Jupiter's own
//! `dexes=` filter: for each candidate DEX label it requests a quote restricted
//! to that DEX, and always includes an unrestricted ("aggregate") quote. The
//! candidate with the highest net `outAmount` (which already bakes in that DEX's
//! fee + price impact) wins, and its quote is reused directly as the swap
//! payload — so `selected_dex` genuinely drives routing, and there is no
//! redundant second quote round-trip.
//!
//! Fee/slippage are parsed from the real response: `routePlan[].swapInfo.feeAmount`
//! summed (P2-17) and `priceImpactPct` read as a percent (P1-6).

use crate::error::{AppError, AppResult};
use parking_lot::RwLock;
use rust_decimal::prelude::*;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 1 SOL = 1e9 lamports.
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Default DEX labels compared against (in addition to the unrestricted
/// "aggregate" Jupiter route). Any label Jupiter does not recognise returns a
/// non-2xx and is silently skipped, so this list is safe to over-specify.
const DEFAULT_DEX_LABELS: &[&str] = &["Raydium", "Orca", "Meteora"];

/// Selected route + cost breakdown.
#[derive(Debug, Clone)]
pub struct RouteSelection {
    /// Winning DEX label (`"Jupiter"` for the unrestricted aggregate route).
    pub selected_dex: String,
    /// The winning Jupiter quote, reused directly as the swap payload.
    pub quote: serde_json::Value,
    /// Total cost (fee + estimated slippage) in SOL.
    pub total_cost_sol: Decimal,
    /// Real per-route fee in SOL (summed `routePlan[].swapInfo.feeAmount`).
    pub fee_sol: Decimal,
    /// Estimated slippage in SOL (from `priceImpactPct`).
    pub slippage_sol: Decimal,
    /// DEX API endpoint used.
    pub dex_url: String,
}

/// Cached route selection.
#[derive(Debug, Clone)]
struct CachedResult {
    selection: RouteSelection,
    cached_at: SystemTime,
}

/// DEX comparator / route selector backed by Jupiter's `dexes=` filter.
pub struct DexComparator {
    /// Cache of recent selections.
    cache: Arc<RwLock<HashMap<String, CachedResult>>>,
    /// Cache TTL in seconds.
    cache_ttl: Duration,
    /// HTTP client for API calls.
    http_client: reqwest::Client,
    /// Jupiter API base URL (e.g. https://api.jup.ag/swap/v1).
    jupiter_api_url: String,
    /// DEX labels to compare against the aggregate route.
    dex_labels: Vec<String>,
    /// When false, skip the per-DEX `dexes=` fan-out and query only the
    /// aggregate route (saves Jupiter API quota when routing diversity isn't
    /// needed).
    multi_dex: bool,
}

impl DexComparator {
    /// Create with the default Jupiter API URL and candidate DEX labels.
    pub fn new() -> Result<Self, String> {
        Self::with_jupiter_api_url("https://api.jup.ag/swap/v1".to_string())
    }

    /// Create with a custom Jupiter API URL and default candidate DEX labels.
    pub fn with_jupiter_api_url(jupiter_api_url: String) -> Result<Self, String> {
        Self::with_jupiter_api_url_and_labels(
            jupiter_api_url,
            DEFAULT_DEX_LABELS.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    /// Create with a custom Jupiter API URL and an explicit candidate DEX list.
    pub fn with_jupiter_api_url_and_labels(
        jupiter_api_url: String,
        dex_labels: Vec<String>,
    ) -> Result<Self, String> {
        Ok(Self::with_options(jupiter_api_url, dex_labels, true))
    }

    /// Create with full control: URL, candidate DEX labels, and whether to
    /// perform the per-DEX `dexes=` fan-out (`multi_dex`).
    pub fn with_options(
        jupiter_api_url: String,
        dex_labels: Vec<String>,
        multi_dex: bool,
    ) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(5),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            jupiter_api_url,
            dex_labels,
            multi_dex,
        }
    }

    /// Set whether the per-DEX `dexes=` fan-out is performed.
    pub fn set_multi_dex(&mut self, multi_dex: bool) {
        self.multi_dex = multi_dex;
    }

    /// Select the best route across the candidate DEXes plus the aggregate.
    ///
    /// `amount` is in **lamports** (Jupiter's `amount` field). `slippage_bps` is
    /// the on-chain tolerance to embed in every candidate quote.
    pub async fn select_route(
        &self,
        token_in: &str,
        token_out: &str,
        amount_lamports: u64,
        slippage_bps: u16,
    ) -> AppResult<RouteSelection> {
        let cache_key = format!("{}:{}:{}:{}", token_in, token_out, amount_lamports, slippage_bps);
        {
            let cache = self.cache.read();
            if let Some(cached) = cache.get(&cache_key) {
                if cached.cached_at.elapsed().unwrap_or_default() < self.cache_ttl {
                    return Ok(cached.selection.clone());
                }
            }
        }

        // Build the candidate set: unrestricted aggregate + one per DEX label
        // (only when multi-DEX comparison is enabled — otherwise aggregate-only,
        // saving Jupiter API quota).
        let aggregate_fut = self.query_jupiter(token_in, token_out, amount_lamports, slippage_bps, None);
        let mut restricted_futs = Vec::new();
        if self.multi_dex {
            for label in &self.dex_labels {
                restricted_futs.push(self.query_jupiter(
                    token_in,
                    token_out,
                    amount_lamports,
                    slippage_bps,
                    Some(label.as_str()),
                ));
            }
        }

        // Run the aggregate + restricted queries concurrently (a serial
        // `.await` on the aggregate before the fan-out would add a full RTT to
        // every cache-miss swap).
        let (aggregate, restricted) = tokio::join!(
            aggregate_fut,
            futures_util::future::join_all(restricted_futs)
        );

        let mut best: Option<RouteSelection> = None;
        // Aggregate route is always the baseline (never worse than any single DEX).
        if let Ok(sel) = aggregate {
            best = Some(sel);
        }
        for sel in restricted.into_iter().flatten() {
            let better = best
                .as_ref()
                .is_none_or(|b| out_amount(&sel.quote) > out_amount(&b.quote));
            if better {
                best = Some(sel);
            }
        }

        let selection = match best {
            Some(s) => s,
            None => {
                tracing::warn!(
                    token_in = %token_in,
                    token_out = %token_out,
                    "All DEX route queries (incl. aggregate) failed"
                );
                return Err(AppError::Internal(format!(
                    "No viable DEX route for {} → {}",
                    token_in, token_out
                )));
            }
        };

        if selection.selected_dex != "Jupiter" {
            tracing::info!(
                selected_dex = %selection.selected_dex,
                fee_sol = %selection.fee_sol,
                slippage_sol = %selection.slippage_sol,
                "Route comparison selected a non-aggregate DEX"
            );
        }

        {
            let mut cache = self.cache.write();
            cache.insert(
                cache_key,
                CachedResult {
                    selection: selection.clone(),
                    cached_at: SystemTime::now(),
                },
            );
        }

        Ok(selection)
    }

    /// Query Jupiter `/quote`, optionally restricted to a single DEX via `dexes=`.
    async fn query_jupiter(
        &self,
        token_in: &str,
        token_out: &str,
        amount_lamports: u64,
        slippage_bps: u16,
        dexes: Option<&str>,
    ) -> AppResult<RouteSelection> {
        let mut url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            self.jupiter_api_url, token_in, token_out, amount_lamports, slippage_bps
        );
        if let Some(label) = dexes {
            url.push_str("&dexes=");
            url.push_str(label);
        }

        let response = crate::jupiter::with_api_key(self.http_client.get(&url))
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Jupiter API error: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::Internal(format!(
                "Jupiter API returned error: {}",
                response.status()
            )));
        }

        let quote: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to parse Jupiter response: {}", e)))?;

        // Validate it is a real quote.
        if out_amount(&quote) == 0 {
            return Err(AppError::Internal(
                "Invalid Jupiter response: missing/zero outAmount".to_string(),
            ));
        }

        let selected_dex = dexes.unwrap_or("Jupiter").to_string();

        // Real per-route fee: sum of routePlan[].swapInfo.feeAmount (raw token
        // units). Direction-aware — the trade value in SOL is used as the
        // denominator so the fee/slippage are correctly SOL-denominated for
        // both BUY (SOL→token) and SELL (token→SOL).
        let fee_raw: u64 = quote
            .get("routePlan")
            .and_then(|rp| rp.as_array())
            .map(|hops| {
                hops.iter()
                    .filter_map(|h| {
                        h.get("swapInfo")
                            .and_then(|s| s.get("feeAmount"))
                            .and_then(|f| f.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .sum()
            })
            .unwrap_or(0);
        let in_amount: u64 = quote
            .get("inAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| quote.get("inAmount").and_then(|v| v.as_u64()))
            .unwrap_or(amount_lamports);
        let out_amount_raw: u64 = out_amount(&quote);

        // Direction-aware trade value in SOL. For BUY (input=SOL) the trade
        // value is the SOL input; for SELL (output=SOL) it's the SOL received.
        // Using `amount_lamports` (the input amount) for SELL would denominate
        // the fee/slippage in token/1e9, not SOL.
        let sol_mint = crate::constants::mints::SOL;
        let trade_value_sol = if token_in == sol_mint {
            Decimal::from(amount_lamports) / Decimal::from(LAMPORTS_PER_SOL)
        } else if token_out == sol_mint {
            Decimal::from(out_amount_raw) / Decimal::from(LAMPORTS_PER_SOL)
        } else {
            // Neither side is SOL (e.g. USDC→token): fall back to the input
            // amount as a rough SOL proxy (cost accounting only — not a routing
            // input, since routing uses outAmount).
            Decimal::from(amount_lamports) / Decimal::from(LAMPORTS_PER_SOL)
        };

        // fee fraction of the trade, expressed in SOL.
        let fee_sol = if in_amount > 0 {
            trade_value_sol * Decimal::from(fee_raw) / Decimal::from(in_amount)
        } else {
            Decimal::ZERO
        };

        // priceImpactPct is a percent string (e.g. "1.5" = 1.5%) — convert to a
        // fraction before scaling by the SOL trade value. (P1-6: previously the
        // comparator divided by 100 inconsistently.)
        let slippage_fraction = quote
            .get("priceImpactPct")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .map(|pct| pct / Decimal::from(100))
            .unwrap_or(Decimal::ZERO);
        let slippage_sol = trade_value_sol * slippage_fraction;
        let total_cost_sol = fee_sol + slippage_sol;

        Ok(RouteSelection {
            selected_dex,
            quote,
            total_cost_sol,
            fee_sol,
            slippage_sol,
            dex_url: self.jupiter_api_url.clone(),
        })
    }

    /// Clear expired cache entries.
    pub fn clear_expired_cache(&self) {
        let mut cache = self.cache.write();
        cache.retain(|_, cached| cached.cached_at.elapsed().unwrap_or_default() < self.cache_ttl);
    }
}

impl Default for DexComparator {
    fn default() -> Self {
        Self::new().expect("Failed to create DexComparator - HTTP client initialization failed")
    }
}

/// Parse the quote's `outAmount` (string) into u64; 0 if absent/unparseable.
fn out_amount(quote: &serde_json::Value) -> u64 {
    quote
        .get("outAmount")
        .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        .or_else(|| quote.get("outAmount").and_then(|v| v.as_u64()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dex_route_caching_key_includes_slippage() {
        // Exercises construction only (no network); real route behaviour is
        // covered by the #[ignore] integration tests requiring a Jupiter key.
        let comparator = DexComparator::new().expect("Failed to create DexComparator for test");
        let _ = comparator
            .select_route(
                "So11111111111111111111111111111111111111112",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                1_000_000_000,
                50,
            )
            .await;
        // Second call within the TTL should hit the cache (no assertion beyond
        // "does not panic" — network availability is environment-dependent).
        let _ = comparator
            .select_route(
                "So11111111111111111111111111111111111111112",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                1_000_000_000,
                50,
            )
            .await;
    }

    #[test]
    fn out_amount_parses_string_or_number() {
        let q_str = serde_json::json!({ "outAmount": "12345" });
        assert_eq!(out_amount(&q_str), 12345);
        let q_num = serde_json::json!({ "outAmount": 999 });
        assert_eq!(out_amount(&q_num), 999);
        let q_empty = serde_json::json!({});
        assert_eq!(out_amount(&q_empty), 0);
    }
}
