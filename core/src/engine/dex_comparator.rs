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
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

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
        Self::with_jupiter_api_url("https://api.jup.ag/swap/v2".to_string())
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
        let cache_key = format!(
            "{}:{}:{}:{}:{}:{}",
            token_in,
            token_out,
            amount_lamports,
            slippage_bps,
            self.multi_dex,
            self.dex_labels.join(",")
        );
        {
            let cache = self.cache.read();
            if let Some(cached) = cache.get(&cache_key) {
                if cached.cached_at.elapsed().unwrap_or_default() < self.cache_ttl {
                    return Ok(cached.selection.clone());
                }
            }
        }

        // The v2 `/order` API cannot express a per-DEX restriction (`dexes=` is
        // v1-only; v2 only supports `excludeDexes`), so the per-DEX fan-out
        // only applies to v1 endpoints. On v2, query the aggregate only and
        // label it "Jupiter" — otherwise every "restricted" query would be an
        // identical aggregate quote mislabelled as a specific DEX.
        let use_v2 = self.jupiter_api_url.contains("/v2") || self.jupiter_api_url.contains("swap/v2");

        // Build the candidate set: unrestricted aggregate + one per DEX label
        // (only when multi-DEX comparison is enabled AND the endpoint supports
        // the per-DEX restriction — otherwise aggregate-only).
        let aggregate_fut = self.query_jupiter(token_in, token_out, amount_lamports, slippage_bps, None);
        let mut restricted_futs = Vec::new();
        if self.multi_dex && !use_v2 {
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

    /// Query Jupiter `/quote` (v1) or `/order` (v2), optionally restricted to a single DEX via `dexes=`.
    ///
    /// For v2, uses the `/order` endpoint which provides better pricing with all routers competing.
    /// For v1 fallback, uses the traditional `/quote` endpoint.
    async fn query_jupiter(
        &self,
        token_in: &str,
        token_out: &str,
        amount_lamports: u64,
        slippage_bps: u16,
        dexes: Option<&str>,
    ) -> AppResult<RouteSelection> {
        // Determine if we're using v2 API (includes /v2 in URL)
        let use_v2 = self.jupiter_api_url.contains("/v2") || self.jupiter_api_url.contains("swap/v2");

        let (endpoint, _quote_key) = if use_v2 {
            // v2 uses /order endpoint for Meta-Aggregator
            ("/order", "transaction")
        } else {
            // v1 uses /quote endpoint
            ("/quote", "swapTransaction")
        };

        let mut url = format!(
            "{}{}?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            self.jupiter_api_url, endpoint, token_in, token_out, amount_lamports, slippage_bps
        );

        // Add DEX restriction for v1 (v2 handles this differently via excludeDexes)
        if dexes.is_some() && !use_v2 {
            if let Some(label) = dexes {
                url.push_str("&dexes=");
                url.push_str(label);
            }
        }

        let response = crate::jupiter::with_api_key(self.http_client.get(&url))
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Jupiter API error: {}", e)))?;

        // Check the HTTP status BEFORE parsing the body: a 4xx/5xx response
        // with a non-JSON body (HTML error page / empty body) must surface the
        // real status and error text, not a misleading parse failure.
        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            let parsed: Option<serde_json::Value> = serde_json::from_str(&body_text).ok();
            let error_msg = parsed
                .as_ref()
                .and_then(|v| v.get("error").and_then(|v| v.as_str()))
                .or_else(|| parsed.as_ref().and_then(|v| v.get("errorMessage").and_then(|v| v.as_str())))
                .unwrap_or_else(|| {
                    let trimmed = body_text.trim();
                    if trimmed.is_empty() {
                        "Unknown Jupiter API error"
                    } else {
                        trimmed
                    }
                });

            tracing::warn!(
                url = %url,
                status = %status,
                error = %error_msg,
                dexes = ?dexes,
                "Jupiter API request failed"
            );

            // Return error for critical failures, but allow missing DEX labels to pass silently
            if let Some(label) = dexes {
                if status.as_u16() == 400 || status.as_u16() == 404 {
                    // Silently skip unsupported DEX labels (Jupiter returns non-2xx for unrecognized labels)
                    return Err(AppError::Internal(format!(
                        "DEX label not supported: {}",
                        label
                    )));
                }
            }

            return Err(AppError::Internal(format!(
                "Jupiter API returned error: {} - {}",
                status, error_msg
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

        // `dexes` is only honoured by the v1 endpoint; a v2 request that got
        // here with a label is aggregate routing and must be labelled as such.
        let selected_dex = match (dexes, use_v2) {
            (Some(label), false) => label.to_string(),
            _ => "Jupiter".to_string(),
        };

        // Real per-route fee: sum of routePlan[].swapInfo.feeAmount (raw token units).
        // Works for both v1 and v2 responses. Parsed as Decimal so decimal
        // strings ("123.45") and JSON numbers are both handled.
        let fee_raw: Decimal = quote
            .get("routePlan")
            .and_then(|rp| rp.as_array())
            .map(|hops| {
                hops.iter().fold(Decimal::ZERO, |acc, h| {
                    let fee = h.get("swapInfo").and_then(|s| s.get("feeAmount"));
                    match fee {
                        Some(f) => {
                            let parsed = match f {
                                serde_json::Value::String(s) => Decimal::from_str(s).ok(),
                                serde_json::Value::Number(n) => n
                                    .as_u64()
                                    .map(Decimal::from)
                                    .or_else(|| n.as_f64().and_then(Decimal::from_f64)),
                                _ => None,
                            };
                            match parsed {
                                Some(v) => acc + v,
                                None => {
                                    tracing::warn!(fee_amount = ?f, "Unparseable feeAmount in routePlan; ignoring");
                                    acc
                                }
                            }
                        }
                        None => acc,
                    }
                })
            })
            .unwrap_or(Decimal::ZERO);
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
            trade_value_sol * fee_raw / Decimal::from(in_amount)
        } else {
            Decimal::ZERO
        };

        // Price impact handling: support both v1 and v2 formats
        // v1: priceImpactPct as string (e.g., "1.5" = 1.5%)
        // v2: priceImpact as decimal (e.g., -0.015 = -1.5%)
        // A present-but-unparseable field is logged rather than silently
        // reporting zero impact.
        let slippage_fraction = if let Some(pct_str) = quote.get("priceImpactPct").and_then(|v| v.as_str()) {
            // v1 format: percentage string
            match Decimal::from_str(pct_str) {
                Ok(pct) => pct / Decimal::from(100),
                Err(_) => {
                    tracing::warn!(price_impact_pct = %pct_str, "Unparseable priceImpactPct; assuming 0%");
                    Decimal::ZERO
                }
            }
        } else if let Some(v) = quote.get("priceImpact") {
            match v.as_f64().and_then(Decimal::from_f64) {
                Some(pct_decimal) => pct_decimal.abs(),
                None => {
                    tracing::warn!(price_impact = ?v, "Unparseable priceImpact; assuming 0%");
                    Decimal::ZERO
                }
            }
        } else {
            Decimal::ZERO
        };

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
    use rust_decimal_macros::dec;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Tiny HTTP server that mocks Jupiter: dispatches each request line to the
    /// handler and responds with `(status, body)`.
    async fn mock_jupiter<F>(mut handler: F) -> String
    where
        F: FnMut(&str) -> (u16, String) + Send + 'static,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 16384];
                let Ok(n) = sock.read(&mut buf).await else {
                    continue;
                };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("").to_string();
                let (status, body) = handler(&first_line);
                let reason = match status {
                    200 => "OK",
                    400 => "Bad Request",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    _ => "Internal Server Error",
                };
                let resp = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    reason,
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{}", addr)
    }

    /// A valid v1-style quote JSON with an `outAmount` of `out`.
    fn v1_quote(out: &str) -> String {
        serde_json::json!({
            "outAmount": out,
            "inAmount": "1000000000",
            "priceImpactPct": "0.5",
            "routePlan": []
        })
        .to_string()
    }

    const SOL: &str = "So11111111111111111111111111111111111111112";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn make_comparator(base_url: String) -> DexComparator {
        DexComparator::with_options(
            base_url,
            vec!["Raydium".to_string(), "Orca".to_string(), "Meteora".to_string()],
            true,
        )
    }

    #[test]
    fn test_constructors_and_default() {
        let c = DexComparator::new().expect("new");
        assert_eq!(c.dex_labels, vec!["Raydium", "Orca", "Meteora"]);
        assert!(c.multi_dex);
        assert_eq!(c.jupiter_api_url, "https://api.jup.ag/swap/v2");

        let c = DexComparator::with_jupiter_api_url("https://x.example".into()).expect("custom url");
        assert_eq!(c.jupiter_api_url, "https://x.example");

        let c = DexComparator::with_jupiter_api_url_and_labels(
            "https://x.example".into(),
            vec!["Raydium".into()],
        )
        .expect("custom labels");
        assert_eq!(c.dex_labels, vec!["Raydium"]);
        assert!(c.multi_dex);

        let mut c = DexComparator::with_options("https://x.example".into(), vec![], false);
        assert!(!c.multi_dex);
        c.set_multi_dex(true);
        assert!(c.multi_dex);
        assert_eq!(c.cache_ttl, Duration::from_secs(5));

        let _d = DexComparator::default();
    }

    #[tokio::test]
    async fn test_select_route_restricted_dex_wins() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        let base = mock_jupiter(move |req| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            if req.contains("dexes=Raydium") {
                (200, v1_quote("12000"))
            } else if req.contains("dexes=Orca") || req.contains("dexes=Meteora") {
                (200, v1_quote("9000"))
            } else {
                (200, v1_quote("10000"))
            }
        })
        .await;

        let comparator = make_comparator(base);
        let sel = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("route");
        // 4 queries: aggregate + 3 restricted
        assert_eq!(count.load(Ordering::Relaxed), 4);
        assert_eq!(sel.selected_dex, "Raydium");
        assert_eq!(out_amount(&sel.quote), 12000);
        assert_eq!(sel.dex_url, comparator.jupiter_api_url);
    }

    #[tokio::test]
    async fn test_select_route_aggregate_wins_when_better() {
        let base = mock_jupiter(|req| {
            if req.contains("dexes=") {
                (200, v1_quote("1000"))
            } else {
                (200, v1_quote("5000"))
            }
        })
        .await;
        let comparator = make_comparator(base);
        let sel = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("route");
        assert_eq!(sel.selected_dex, "Jupiter");
    }

    #[tokio::test]
    async fn test_select_route_cache_hit_and_miss() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        let base = mock_jupiter(move |_req| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            (200, v1_quote("10000"))
        })
        .await;
        let comparator = make_comparator(base);

        let _ = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("first");
        // 4 requests: aggregate + 3 restricted
        assert_eq!(count.load(Ordering::Relaxed), 4);
        let _ = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("second");
        // Same key -> served from cache, no second fetch
        assert_eq!(count.load(Ordering::Relaxed), 4);

        // Different amount -> cache miss -> refetch
        let _ = comparator.select_route(SOL, USDC, 2_000_000_000, 50).await.expect("third");
        assert_eq!(count.load(Ordering::Relaxed), 8);
    }

    #[tokio::test]
    async fn test_select_route_expired_cache_refetches() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        let base = mock_jupiter(move |req| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            (200, v1_quote("10000"))
        })
        .await;
        let comparator = make_comparator(base);

        // Insert an expired entry directly (private field access in-module)
        let key = format!(
            "{}:{}:{}:{}:{}:{}",
            SOL,
            USDC,
            1_000_000_000u64,
            50u16,
            true,
            "Raydium,Orca,Meteora"
        );
        comparator.cache.write().insert(
            key.clone(),
            CachedResult {
                selection: RouteSelection {
                    selected_dex: "Jupiter".into(),
                    quote: serde_json::json!({}),
                    total_cost_sol: Decimal::ZERO,
                    fee_sol: Decimal::ZERO,
                    slippage_sol: Decimal::ZERO,
                    dex_url: String::new(),
                },
                cached_at: SystemTime::now() - Duration::from_secs(60),
            },
        );

        let sel = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("route");
        assert_eq!(count.load(Ordering::Relaxed), 4);
        assert_eq!(sel.selected_dex, "Jupiter");
        // Fresh entry is now cached
        let cache = comparator.cache.read();
        let cached = cache.get(&key).unwrap();
        assert_eq!(cached.selection.selected_dex, "Jupiter");
    }

    #[tokio::test]
    async fn test_select_route_v2_aggregate_only() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        let base = mock_jupiter(move |req| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            let _ = &req;
            assert!(req.contains("/order"), "v2 must use /order, got: {}", req);
            assert!(!req.contains("dexes="), "v2 must not fan out to dexes, got: {}", req);
            (200, v1_quote("10000"))
        })
        .await;

        // v2 detection: URL contains /v2
        let comparator = make_comparator(format!("{}/swap/v2", base));
        let sel = comparator
            .select_route(SOL, USDC, 1_000_000_000, 50)
            .await
            .expect("route");
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(sel.selected_dex, "Jupiter");
    }

    #[tokio::test]
    async fn test_select_route_multi_dex_disabled() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        let base = mock_jupiter(move |req| {
            count_clone.fetch_add(1, Ordering::Relaxed);
            assert!(!req.contains("dexes="), "no fan-out when multi_dex=false: {}", req);
            (200, v1_quote("10000"))
        })
        .await;
        let comparator =
            DexComparator::with_options(format!("{}/swap/v1", base), vec!["Raydium".into()], false);
        let sel = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.expect("route");
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(sel.selected_dex, "Jupiter");
    }

    #[tokio::test]
    async fn test_select_route_all_fail() {
        let base = mock_jupiter(|_| (500, "boom".to_string())).await;
        let comparator = make_comparator(base);
        let err = comparator.select_route(SOL, USDC, 1_000_000_000, 50).await.unwrap_err();
        assert!(err.to_string().contains("No viable DEX route"));
    }

    #[tokio::test]
    async fn test_query_jupiter_v1_full_parsing() {
        let quote = serde_json::json!({
            "outAmount": "999000000",
            "inAmount": "1000000000",
            "priceImpactPct": "1.5",
            "routePlan": [
                { "swapInfo": { "feeAmount": "500000" } },
                { "swapInfo": { "feeAmount": 250000 } },
                { "swapInfo": { "feeAmount": 1.5 } },
                { "swapInfo": {} },
                { "swapInfo": { "feeAmount": "garbage" } }
            ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        let sel = comparator
            .query_jupiter(SOL, USDC, 1_000_000_000, 50, Some("Raydium"))
            .await
            .expect("route");
        assert_eq!(sel.selected_dex, "Raydium");
        // fee = 500000 + 250000 + 1.5 (u64 path, u64 path, f64 path; missing & garbage ignored)
        // fee_raw is in raw token units; in_amount = 1e9; trade_value = 1 SOL
        // 750001.5 / 1e9 = 0.0007500015
        assert_eq!(sel.fee_sol, dec!(0.0007500015));
        // slippage = 1 SOL * 1.5% = 0.015
        assert_eq!(sel.slippage_sol, dec!(0.015));
        assert_eq!(sel.total_cost_sol, dec!(0.0157500015));
    }

    #[tokio::test]
    async fn test_query_jupiter_v2_parsing() {
        let quote = serde_json::json!({
            "outAmount": "500000000",
            "inAmount": 1000000000,
            "priceImpact": -0.02,
            "routePlan": [ { "swapInfo": { "feeAmount": 300000 } } ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(format!("{}/swap/v2", base));
        // v2 with a dex label: labelled "Jupiter" (dexes is v1-only); token_in is
        // not SOL, so trade value = outAmount/1e9 = 0.5 SOL; slippage = 0.5 * 2% = 0.01
        let sel = comparator
            .query_jupiter(USDC, SOL, 1_000_000_000, 50, Some("Raydium"))
            .await
            .expect("route");
        assert_eq!(sel.selected_dex, "Jupiter");
        assert_eq!(sel.slippage_sol, dec!(0.01));
    }

    #[tokio::test]
    async fn test_query_jupiter_sell_side_value() {
        // SELL: token_out == SOL -> trade value from outAmount
        let quote = serde_json::json!({
            "outAmount": "2000000000",
            "inAmount": "1000000000",
            "priceImpactPct": "1",
            "routePlan": [ { "swapInfo": { "feeAmount": "10000000" } } ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        let sel = comparator.query_jupiter(USDC, SOL, 1_000_000_000, 50, None).await.expect("route");
        // trade value = 2 SOL; fee = 2 * 1e7 / 1e9 = 0.02; slippage = 2 * 1% = 0.02
        assert_eq!(sel.fee_sol, dec!(0.02));
        assert_eq!(sel.slippage_sol, dec!(0.02));
    }

    #[tokio::test]
    async fn test_query_jupiter_in_amount_zero_and_missing_impact() {
        // inAmount == 0 -> fee_sol forced to ZERO; no impact fields -> zero slippage
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "inAmount": "0",
            "routePlan": [ { "swapInfo": { "feeAmount": "500000" } } ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        let sel = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.expect("route");
        assert_eq!(sel.fee_sol, Decimal::ZERO);
        assert_eq!(sel.slippage_sol, Decimal::ZERO);

        // inAmount missing -> falls back to amount_lamports
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "priceImpactPct": "0.5",
            "routePlan": [ { "swapInfo": { "feeAmount": "1000000000" } } ]
        })
        .to_string();
        let base2 = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator2 = make_comparator(base2);
        let sel2 = comparator2.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.expect("route");
        // fee = 1 * 1e9 / 1e9 = 1 SOL
        assert_eq!(sel2.fee_sol, Decimal::ONE);
    }

    #[tokio::test]
    async fn test_query_jupiter_unparseable_impact_fields() {
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "inAmount": "1000000000",
            "priceImpactPct": "not-a-number",
            "routePlan": []
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        let sel = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.expect("route");
        assert_eq!(sel.slippage_sol, Decimal::ZERO);

        // priceImpact present but unparseable (string)
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "inAmount": "1000000000",
            "priceImpact": "nope",
            "routePlan": []
        })
        .to_string();
        let base2 = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator2 = make_comparator(base2);
        let sel2 = comparator2.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.expect("route");
        assert_eq!(sel2.slippage_sol, Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_query_jupiter_dex_not_supported() {
        let base = mock_jupiter(|_| (400, r#"{"error":"dexes contains unsupported value"}"#.into())).await;
        let comparator = make_comparator(base);
        let err = comparator
            .query_jupiter(SOL, USDC, 1_000_000_000, 50, Some("FakeDex"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("DEX label not supported"));
    }

    #[tokio::test]
    async fn test_query_jupiter_generic_error_bodies() {
        // JSON error field
        let base = mock_jupiter(|_| (500, r#"{"error":"upstream failed"}"#.into())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("upstream failed"));

        // errorMessage field
        let base = mock_jupiter(|_| (500, r#"{"errorMessage":"bad request"}"#.into())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("bad request"));

        // Plain-text body
        let base = mock_jupiter(|_| (500, "gateway timeout".into())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("gateway timeout"));

        // Empty body -> fallback message
        let base = mock_jupiter(|_| (500, String::new())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("Unknown Jupiter API error"));
    }

    #[tokio::test]
    async fn test_query_jupiter_parse_error() {
        let base = mock_jupiter(|_| (200, "not json at all".into())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("Failed to parse Jupiter response"));
    }

    #[tokio::test]
    async fn test_query_jupiter_zero_out_amount() {
        let base = mock_jupiter(|_| (200, serde_json::json!({ "foo": 1 }).to_string())).await;
        let comparator = make_comparator(base);
        let err = comparator.query_jupiter(SOL, USDC, 1_000_000_000, 50, None).await.unwrap_err();
        assert!(err.to_string().contains("missing/zero outAmount"));
    }

    #[test]
    fn test_clear_expired_cache() {
        let comparator = make_comparator("https://x.example".into());
        let fresh = CachedResult {
            selection: RouteSelection {
                selected_dex: "Jupiter".into(),
                quote: serde_json::json!({}),
                total_cost_sol: Decimal::ZERO,
                fee_sol: Decimal::ZERO,
                slippage_sol: Decimal::ZERO,
                dex_url: String::new(),
            },
            cached_at: SystemTime::now(),
        };
        let stale = CachedResult {
            selection: fresh.selection.clone(),
            cached_at: SystemTime::now() - Duration::from_secs(60),
        };
        comparator.cache.write().insert("fresh".into(), fresh);
        comparator.cache.write().insert("stale".into(), stale);

        comparator.clear_expired_cache();
        let cache = comparator.cache.read();
        assert!(cache.contains_key("fresh"));
        assert!(!cache.contains_key("stale"));
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

    #[tokio::test]
    async fn test_query_jupiter_unparseable_fee_type_is_ignored() {
        // feeAmount that is neither a string nor a number (a bool) is skipped
        // (logged, not summed) — the `_ => None` arm of the fee parser.
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "inAmount": "1000000000",
            "priceImpactPct": "0.5",
            "routePlan": [ { "swapInfo": { "feeAmount": true } } ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        let sel = comparator
            .query_jupiter(SOL, USDC, 1_000_000_000, 50, None)
            .await
            .expect("route");
        assert_eq!(sel.fee_sol, Decimal::ZERO);
    }

    #[tokio::test]
    async fn test_query_jupiter_non_sol_token_pair_uses_input_as_proxy() {
        // Neither input nor output is SOL → trade value falls back to the input
        // amount (cost accounting only).
        let quote = serde_json::json!({
            "outAmount": "1000000000",
            "inAmount": "1000000000",
            "priceImpactPct": "0.5",
            "routePlan": [ { "swapInfo": { "feeAmount": "1000000000" } } ]
        })
        .to_string();
        let base = mock_jupiter(move |_req| (200, quote.clone())).await;
        let comparator = make_comparator(base);
        // USDC -> some other token: neither side is SOL.
        let sel = comparator
            .query_jupiter(USDC, "4wBqp9c6bXWw1U9pJ5W9BmVtV9X7iV2iGJ8dJq2H9aX", 1_000_000_000, 50, None)
            .await
            .expect("route");
        // trade value = 1e9/1e9 = 1 SOL proxy; fee = 1 * 1e9/1e9 = 1 SOL.
        assert_eq!(sel.fee_sol, Decimal::ONE);
        assert_eq!(sel.slippage_sol, dec!(0.005));
    }
}
