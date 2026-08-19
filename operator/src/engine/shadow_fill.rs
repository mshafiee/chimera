//! Shadow-fill calibration model (Phase C3).
//!
//! Models realistic fill prices for paper trading using delayed requotes so
//! paper PnL approximates live PnL. All work is fire-and-forget off the
//! trading path — quote failures leave `quote_json` NULL and never block
//! trading or skew decisions.
//!
//! ## Model (v1-delayed-requote)
//! 1. **Decision-time quote:** Jupiter quote captured at decision time.
//! 2. **Entry latency:** measured as `decide()` internal latency
//!    (`received_at` → `decided_at`). Tracked as rolling percentiles
//!    (p50/p95) so the delayed requote is scheduled at a realistic offset.
//!    `source_slot` is not yet wired through the ingress paths, so decision
//!    latency is used as the latency proxy.
//! 3. **Delayed requote:** at `decided_at + latency_p50`, a second Jupiter
//!    quote is fetched. The difference between the decision-time and delayed
//!    quote fill prices is the **modeled slippage**.
//! 4. **Non-landing probability:** a fixed estimate (default 3%) applied as a
//!    binary mask in simulation. Configurable via `CHIMERA_NONLANDING_PROB`.
//!
//! The structured payload written to `decision_records.quote_json`:
//! ```json
//! { "model_version": "v1-delayed-requote",
//!   "decision_quote": <jupiter quote>,
//!   "delayed_quote": <jupiter quote | null>,
//!   "modeled_slippage_pct": <f64>,
//!   "nonlanding_prob": <f64> }
//! ```

use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::constants::mints;
use crate::engine::transaction_builder::TransactionBuilder;

/// Slippage tolerance (bps) used for shadow-fill quotes. Generous so the
/// quote's `outAmount` (tolerance-independent) drives the modeled price.
const SHADOW_SLIPPAGE_BPS: u16 = 1000;

/// Sanity ceiling for modeled slippage (%). A move >100% between the
/// decision-time and delayed Jupiter quotes (captured milliseconds apart) is a
/// failed or vacuous quote — liquidity vanished, a tiny `outAmount`, or a
/// mid-launch pump — not slippage. Such values are discarded (NULL) so a
/// handful of bad captures can't skew the paper/live bias verdict.
const MAX_MODELED_SLIPPAGE_PCT: f64 = 100.0;

/// Raw modeled slippage between the decision-time and delayed quote fill
/// prices, as an absolute percentage. Returns `None` on missing/degenerate
/// inputs (one side absent, or a zero decision price).
fn raw_modeled_slippage(decision_price: Option<f64>, delayed_price: Option<f64>) -> Option<f64> {
    let (d, l) = (decision_price?, delayed_price?);
    if d == 0.0 {
        return None;
    }
    Some(((l - d) / d).abs() * 100.0)
}

/// Whether a modeled slippage value is sane enough to record. Non-finite
/// results and values beyond `MAX_MODELED_SLIPPAGE_PCT` are quote failures.
fn in_slippage_bounds(slip: f64) -> bool {
    slip.is_finite() && slip <= MAX_MODELED_SLIPPAGE_PCT
}

/// Default non-landing probability (binary mask) for the v1 model.
fn default_nonlanding_prob() -> f64 {
    match std::env::var("CHIMERA_NONLANDING_PROB") {
        Ok(raw) => match raw.parse::<f64>() {
            Ok(p) if (0.0..=1.0).contains(&p) => p,
            _ => {
                tracing::warn!(
                    raw = %raw,
                    "Invalid CHIMERA_NONLANDING_PROB (must be 0.0-1.0); using default 0.03"
                );
                0.03
            }
        },
        Err(_) => 0.03,
    }
}

/// Rolling latency sampler (microseconds) with p50/p95 percentile lookup.
///
/// Keeps the most recent `cap` samples. Cheap to update; percentile lookup is
/// O(n log n) over the window, called once per admitted decision.
#[derive(Default)]
pub struct LatencyTracker {
    samples: Mutex<VecDeque<u64>>,
    cap: usize,
}

impl LatencyTracker {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(cap)),
            cap,
        }
    }

    /// Record a latency sample (microseconds).
    pub fn record(&self, latency_us: u64) {
        let mut s = self.samples.lock();
        // A cap of 0 means "don't track" — guard against the default.
        if self.cap == 0 {
            return;
        }
        if s.len() >= self.cap {
            s.pop_front();
        }
        s.push_back(latency_us);
    }

    /// Percentile `p` in `[0, 100]` of recorded latencies (microseconds).
    ///
    /// The sample buffer is copied under the lock but sorted AFTER releasing
    /// it, so `record` is not blocked for the whole O(n log n) sort.
    pub fn percentile(&self, p: f64) -> u64 {
        let snapshot: Vec<u64> = {
            let s = self.samples.lock();
            s.iter().copied().collect()
        };
        if snapshot.is_empty() {
            return 0;
        }
        let mut sorted = snapshot;
        sorted.sort_unstable();
        let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn p50_us(&self) -> u64 {
        self.percentile(50.0)
    }
}

/// Fire-and-forget shadow-fill capture + delayed requote for an admitted
/// decision. Spawned from `SelectionService` and never blocks the trading
/// path. On any failure the decision record keeps NULL `quote_json`.
pub async fn capture_and_model_fill(
    quote_client: Arc<TransactionBuilder>,
    latency_tracker: Arc<LatencyTracker>,
    recorder: Arc<crate::engine::DecisionRecorder>,
    decision_id: String,
    token_address: String,
    size_sol: f64,
    decide_latency_us: u64,
    is_buy: bool,
) {
    let wsol = match solana_sdk::pubkey::Pubkey::from_str(mints::SOL) {
        Ok(p) => p,
        Err(_) => return,
    };
    let token = match solana_sdk::pubkey::Pubkey::from_str(&token_address) {
        Ok(p) => p,
        Err(_) => return,
    };
    // Validate the size before any lamport conversion: NaN/negative/inf or
    // values beyond u64 lamport range must fail loudly, not silently corrupt
    // the modeled fill.
    let amount_lamports = if !size_sol.is_finite() || size_sol <= 0.0 {
        tracing::warn!(size_sol, "Invalid size_sol for shadow-fill; skipping");
        return;
    } else {
        let lamports = (size_sol * 1e9).round();
        if lamports > u64::MAX as f64 {
            tracing::warn!(size_sol, "size_sol exceeds u64 lamport range; skipping");
            return;
        }
        lamports as u64
    };
    if amount_lamports == 0 {
        return;
    }

    let nonlanding_prob = default_nonlanding_prob();

    // Capture the decision-time deadline BEFORE the decision quote so the
    // delayed requote lands at decided_at + p50 regardless of how long the
    // quote round-trip (with its own retries) takes.
    let task_start = tokio::time::Instant::now();

    // Decision-time quote. For BUY: WSOL→TOKEN; for SELL: TOKEN→WSOL.
    let decision_quote = if is_buy {
        quote_client
            .get_jupiter_quote(wsol, token, amount_lamports, SHADOW_SLIPPAGE_BPS)
            .await
            .ok()
    } else {
        quote_client
            .get_jupiter_quote(token, wsol, amount_lamports, SHADOW_SLIPPAGE_BPS)
            .await
            .ok()
    };

    let decision_price = decision_quote.as_ref().and_then(|q| fill_price(q, is_buy));

    // Record this decision's latency so future delayed requotes use p50.
    latency_tracker.record(decide_latency_us);
    let latency_p50 = latency_tracker.p50_us();

    // Delayed requote at decided_at + latency_p50 (measured from the task
    // start, not from the decision quote's completion).
    let delayed_quote = if latency_p50 > 0 {
        let requote_deadline = task_start + std::time::Duration::from_micros(latency_p50);
        let now = tokio::time::Instant::now();
        if requote_deadline > now {
            tokio::time::sleep(requote_deadline - now).await;
        }
        if is_buy {
            quote_client
                .get_jupiter_quote(wsol, token, amount_lamports, SHADOW_SLIPPAGE_BPS)
                .await
                .ok()
        } else {
            quote_client
                .get_jupiter_quote(token, wsol, amount_lamports, SHADOW_SLIPPAGE_BPS)
                .await
                .ok()
        }
    } else {
        None
    };

    let delayed_price = delayed_quote.as_ref().and_then(|q| fill_price(q, is_buy));

    // Sanity-bound: a > MAX_MODELED_SLIPPAGE_PCT move between two quotes
    // captured milliseconds apart is a failed quote, not slippage. Discarding
    // it keeps the capture budget honest and prevents a few corrupt values
    // from blowing up the paper/live bias verdict (observed: up to 1,140%).
    let modeled_slippage_pct = raw_modeled_slippage(decision_price, delayed_price).filter(|slip| {
        let ok = in_slippage_bounds(*slip);
        if !ok {
            tracing::warn!(
                decision_id = %decision_id,
                token_address = %token_address,
                is_buy,
                modeled_slippage_pct = slip,
                max = MAX_MODELED_SLIPPAGE_PCT,
                "Shadow-fill modeled slippage exceeds sanity ceiling — treating as failed quote"
            );
        }
        ok
    });

    let payload: Value = json!({
        "model_version": "v1-delayed-requote",
        "decision_quote": decision_quote,
        "delayed_quote": delayed_quote,
        "modeled_slippage_pct": modeled_slippage_pct,
        "nonlanding_prob": nonlanding_prob,
    });

    recorder.update_fill_model(
        decision_id,
        payload,
        "v1-delayed-requote",
        modeled_slippage_pct,
    );
}

/// Extract a comparable fill price from a Jupiter quote JSON.
///
/// For BUY (WSOL→TOKEN) the price is `inAmount_lamports / outAmount_base`.
/// For SELL (TOKEN→WSOL) it is `outAmount_lamports / inAmount_base`. Both
/// reduce to a per-token-base-unit price in lamports; the ratio of delayed
/// to decision price yields the modeled slippage independent of direction.
///
/// Amounts are parsed as integers (u128) and the ratio computed from them,
/// avoiding the 2^53 precision loss of f64 parsing; non-finite ratios are
/// rejected.
fn fill_price(quote: &Value, is_buy: bool) -> Option<f64> {
    let in_amount = parse_amount(quote.get("inAmount"))?;
    let out_amount = parse_amount(quote.get("outAmount"))?;
    if in_amount == 0 || out_amount == 0 {
        return None;
    }
    let ratio = if is_buy {
        in_amount as f64 / out_amount as f64
    } else {
        out_amount as f64 / in_amount as f64
    };
    if !ratio.is_finite() {
        return None;
    }
    Some(ratio)
}

/// Parse a Jupiter amount as a non-negative integer (`u128`), accepting both
/// string and numeric JSON values. Returns `None` for absent/unparseable/
/// negative values ("NaN"/"inf" strings fail the parse).
fn parse_amount(value: Option<&Value>) -> Option<u128> {
    match value? {
        Value::String(s) => s.parse::<u128>().ok(),
        Value::Number(n) => n
            .as_u64()
            .map(u128::from)
            .or_else(|| n.as_i64().and_then(|i| u128::try_from(i).ok())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_slippage_missing_inputs_is_none() {
        assert_eq!(raw_modeled_slippage(None, Some(1.0)), None);
        assert_eq!(raw_modeled_slippage(Some(1.0), None), None);
        assert_eq!(raw_modeled_slippage(None, None), None);
    }

    #[test]
    fn raw_slippage_zero_decision_price_is_none() {
        assert_eq!(raw_modeled_slippage(Some(0.0), Some(1.0)), None);
    }

    #[test]
    fn raw_slippage_computes_positive_percent() {
        // 1.10 compared to 1.00 is +10% (absolute).
        let up = raw_modeled_slippage(Some(1.0), Some(1.1)).unwrap();
        assert!((up - 10.0).abs() < 1e-9, "expected ~10.0, got {up}");
        // Slippage below decision price also yields a positive percent.
        let down = raw_modeled_slippage(Some(1.0), Some(0.9)).unwrap();
        assert!((down - 10.0).abs() < 1e-9, "expected ~10.0, got {down}");
    }

    #[test]
    fn bounds_accept_realistic_slippage() {
        assert!(in_slippage_bounds(5.0));
        assert!(in_slippage_bounds(99.9));
    }

    #[test]
    fn bounds_reject_corrupt_and_nonfinite() {
        assert!(!in_slippage_bounds(1140.0));
        assert!(!in_slippage_bounds(101.0));
        assert!(!in_slippage_bounds(f64::INFINITY));
        assert!(!in_slippage_bounds(f64::NAN));
    }
}
