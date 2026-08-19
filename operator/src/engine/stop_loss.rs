//! Hard and dynamic stop-loss system
//!
//! Implements:
//! - Hard stop-loss at -25% (never let losses run)
//! - Dynamic stops (tighter for low-WQS wallets, wider for high-WQS)
//! - Portfolio-level stop (pause all trading if daily loss >5%)
//! - ATR-based stop-loss optimization with market regime adjustment
//! - Market regime detection (BULL/BEAR/VOLATILE/NEUTRAL)

use crate::config::ProfitManagementConfig;
use crate::db_abstraction::Database;
use crate::engine::smart_exit::should_defer_exit;
use crate::monitoring::SignalAggregator;
use crate::price_cache::PriceCache;
use crate::token::TokenParser;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Market regime for stop-loss adjustment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketRegime {
    Bull,     // Widen stops (follow trends)
    Bear,     // Tighten stops (preserve capital)
    Volatile, // Widen stops significantly (avoid wick-outs)
    Neutral,  // Standard parameters
}

impl MarketRegime {
    /// Get regime multiplier for ATR-based stops
    pub fn atr_multiplier(&self) -> Decimal {
        match self {
            MarketRegime::Bull => dec!(1.5),
            MarketRegime::Bear => dec!(1.0),
            MarketRegime::Volatile => dec!(2.0),
            MarketRegime::Neutral => dec!(1.25),
        }
    }

    /// Parse from string (for config loading)
    pub fn parse_regime(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "BULL" => MarketRegime::Bull,
            "BEAR" => MarketRegime::Bear,
            "VOLATILE" => MarketRegime::Volatile,
            other => {
                // A config typo silently mapping to Neutral hides the error —
                // surface it so the operator notices.
                tracing::warn!(
                    value = other,
                    "Unknown market regime in config — defaulting to NEUTRAL"
                );
                MarketRegime::Neutral
            }
        }
    }
}

/// Stop-loss manager
pub struct StopLossManager {
    db: Arc<dyn Database>,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
    /// Optional in-memory consensus cache (avoids per-position DB query every 5 s).
    /// Set via `set_signal_aggregator` after construction.
    signal_aggregator: Arc<RwLock<Option<Arc<SignalAggregator>>>>,
    /// Optional token parser for the pre-graduation exit rail (bonding-curve
    /// completion checks). Set via `set_token_parser` after construction.
    token_parser: Arc<RwLock<Option<Arc<TokenParser>>>>,
    /// Per-position live-fill deferral budget (Phase 1 smart exit). Keyed by
    /// trade_uuid: how many consecutive monitor ticks a protective exit has been
    /// deferred while waiting for a better fill. Bounded by
    /// `ProfitManagementConfig::defer_max_ticks` so a persistently bad fill never
    /// strands a position. In-memory only — a restart re-evaluates positions
    /// fresh, which is safe for a ~15s window.
    defer_counts: RwLock<HashMap<String, u64>>,
}

/// Stop-loss action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopLossAction {
    /// No action
    None,
    /// Exit position (stop-loss hit)
    Exit,
}

impl StopLossManager {
    pub fn new(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            signal_aggregator: Arc::new(RwLock::new(None)),
            token_parser: Arc::new(RwLock::new(None)),
            defer_counts: RwLock::new(HashMap::new()),
        }
    }

    /// Wire in the signal aggregator after construction so consensus checks read from
    /// the in-memory cache instead of issuing a DB query on every position tick.
    pub async fn set_signal_aggregator(&self, agg: Arc<SignalAggregator>) {
        *self.signal_aggregator.write().await = Some(agg);
    }

    /// Wire in the token parser after construction so the pre-graduation exit
    /// rail can read pump.fun bonding-curve state.
    pub async fn set_token_parser(&self, parser: Arc<TokenParser>) {
        *self.token_parser.write().await = Some(parser);
    }

    /// Decide whether a protective exit (stop-loss / recovery gate) should be
    /// DEFERRED this tick because the LIVE sell fill would realize a materially
    /// worse loss than the price-cache reading.
    ///
    /// This closes the realize-vs-price gap: the shadow `mirror_main` exit
    /// (which mirrors these rails against the cached price) predicts ~46.6% win
    /// on admitted signals, but real closes realize ~21% win because protective
    /// exits fire on the stale cache price and sell into a bad fill.
    ///
    /// Fail-safe: returns `false` (do not defer → exit now) when disabled, no
    /// token_parser is wired, no live fill is decidable, or the position is
    /// past the bounded defer budget. A catastrophic loss (at/beyond −25%, the
    /// hard-stop floor) never defers via `should_defer_exit`.
    async fn protective_stop_should_defer(
        &self,
        trade_uuid: &str,
        token_address: &str,
        entry_price_usd: Decimal,
        cache_loss_pct: Decimal,
    ) -> bool {
        // Config gate: preserve today's behavior unless deferral is enabled and
        // a quote client is wired in.
        if self.config.defer_max_ticks == 0 || self.config.exit_skew_pct <= Decimal::ZERO {
            return false;
        }
        let Some(parser) = self.token_parser.read().await.clone() else {
            return false;
        };

        // Decode the entry to build a test amount (mirrors profit_targets'
        // quote_confirms_profit). Unknown decimals -> cannot quote -> fail-safe.
        let decimals = match self.price_cache.get_price(token_address) {
            Some(entry) => entry.decimals,
            None => None,
        };
        let Some(decimals) = decimals else {
            return false;
        };
        let Some(test_amount) = 10u64.checked_pow(decimals as u32) else {
            return false;
        };
        let out_sol = match parser.sell_quote_out_sol(token_address, test_amount).await {
            Ok(Some(v)) => v,
            _ => return false,
        };
        let sol_price_usd = match self.price_cache.get_sol_price_usd() {
            Some(v) if v > Decimal::ZERO => v,
            _ => return false,
        };
        if entry_price_usd.is_zero() {
            return false;
        }

        // Implied live fill price per token (USD) and its implied loss.
        // `test_amount` = 10^decimals SPL units = exactly 1 token, so `out_sol`
        // is the SOL received for 1 token → `out_sol * sol_price_usd` is USD per
        // token, comparable to `entry_price_usd`. (Matches profit_targets'
        // `quote_confirms_profit`, which uses `out_sol * sol_price_usd`.)
        let fill_price_usd = out_sol * sol_price_usd;
        let live_loss_pct =
            ((fill_price_usd - entry_price_usd) / entry_price_usd) * Decimal::from(100);

        let defer = should_defer_exit(
            cache_loss_pct,
            Some(live_loss_pct),
            dec!(-25), // hard-stop catastrophe floor (matches is_hard_stop)
            self.config.exit_skew_pct,
        );
        if !defer {
            return false;
        }

        // Bounded defer budget: count consecutive deferred ticks per position so
        // a persistently bad fill never strands the position indefinitely.
        let mut counts = self.defer_counts.write().await;
        let count = counts.entry(trade_uuid.to_string()).or_insert(0);
        *count += 1;
        if *count > self.config.defer_max_ticks {
            *count = 0; // reset so a later re-entry starts a fresh budget
            return false;
        }
        true
    }

    /// Calculate ATR (Average True Range) from a close-price series.
    ///
    /// The input is a close-price series only (no OHLC bars), so the per-period
    /// range is the close-to-close move — a proxy for True Range. Only the
    /// last `period` samples define the lookback window.
    pub fn calculate_atr(&self, price_history: &[Decimal], period: usize) -> Option<Decimal> {
        if period == 0 || price_history.len() < period + 1 {
            return None;
        }

        // Only the last `period` closes participate: with closes-only data the
        // per-period range is |close_i - close_{i-1}|.
        let window = &price_history[price_history.len() - period - 1..];
        let mut ranges = Vec::with_capacity(period);
        for pair in window.windows(2) {
            ranges.push((pair[1] - pair[0]).abs());
        }

        let sum: Decimal = ranges.iter().sum();
        Some(sum / Decimal::from(ranges.len()))
    }

    /// Get current market regime from configuration
    pub fn get_market_regime(&self) -> MarketRegime {
        MarketRegime::parse_regime(&self.config.market_regime)
    }

    /// Calculate ATR-based stop-loss price
    ///
    /// # Arguments
    /// * `entry_price` - Position entry price
    /// * `atr_value` - Current ATR value
    /// * `regime` - Market regime for adjustment
    ///
    /// # Returns
    /// Stop-loss price calculated as: entry_price - (atr * multiplier * regime_factor)
    pub fn calculate_atr_stop_loss(
        &self,
        entry_price: Decimal,
        atr_value: Decimal,
        regime: MarketRegime,
    ) -> Decimal {
        // Get base ATR multiplier from config
        let atr_multiplier = self.config.atr_multiplier;

        // Get regime multiplier
        let regime_multiplier = regime.atr_multiplier();

        // Calculate ATR-based stop distance
        let atr_distance = atr_value * atr_multiplier * regime_multiplier;

        // Calculate stop-loss price (for long positions)
        let stop_loss_price = entry_price - (entry_price * atr_distance / dec!(100.0));

        tracing::debug!(
            entry_price = %entry_price,
            atr_value = %atr_value,
            atr_multiplier = %atr_multiplier,
            regime_multiplier = %regime_multiplier,
            stop_loss_price = %stop_loss_price,
            "ATR-based stop-loss calculated"
        );

        stop_loss_price
    }

    /// Check stop-loss for a position
    ///
    /// # Arguments
    /// * `trade_uuid` - Trade UUID
    /// * `wallet_address` - Wallet address (for WQS-based dynamic stops)
    /// * `entry_price` - Entry price (using Decimal for precision)
    /// * `token_address` - Token address
    ///
    /// # Returns
    /// Stop-loss action
    pub async fn check_stop_loss(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        entry_price: Decimal,
        token_address: &str,
        entry_time: chrono::DateTime<chrono::Utc>,
    ) -> StopLossAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => {
                // Check staleness even when price is cached — a cached price can be
                // hours old if the feed is stale. Without this check, stop-loss
                // protection silently degrades for tokens with stale feeds.
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        current_price = %price,
                        "STALE_PRICE: cached price is stale (>90s old) — forcing exit (risk management blind)"
                    );
                    return StopLossAction::Exit;
                }
                price
            }
            None => {
                // No cached price at all — check if this is a tracked token with a stale feed.
                // is_tracked_price_stale only reports staleness for tokens that are actually
                // being tracked, so this branch fires exactly when a tracked token's feed
                // has gone silent (>90s) — the force-exit safety net.
                if self.price_cache.is_tracked_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: no price update for >90s on tracked token — forcing exit (risk management blind)"
                    );
                    return StopLossAction::Exit;
                }
                return StopLossAction::None;
            }
        };

        // Calculate loss percentage using Decimal for precision
        // Negative when price has fallen (e.g. -15.0 for 15% drop), matching negative thresholds.
        // The engine now rejects BUY signals with zero entry_price before opening the position.
        // This guard is a last-resort safety net for positions that predate that check or were
        // inserted directly into the DB — force-exit to recover capital rather than holding
        // a position with no cost basis indefinitely.
        if entry_price.is_zero() {
            tracing::error!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                entry_price = %entry_price,
                "CORRUPT_POSITION: entry_price is zero — forcing immediate exit to recover capital"
            );
            return StopLossAction::Exit;
        }

        let loss_percent = {
            let diff = current_price - entry_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        };

        let elapsed_secs = chrono::Utc::now()
            .signed_duration_since(entry_time)
            .num_seconds()
            .max(0); // Clock-skew guard: a future-dated entry must not suppress stops indefinitely
        let is_hard_stop = loss_percent <= dec!(-25);

        // Recovery Gate: after wick protection + buffer (~90s), if the position
        // hasn't recovered above the threshold, cut. Data shows winners recover
        // above -1% within 48s; losers stay below -2.5%. This cuts losers 60%
        // faster than waiting for the -5% to -20% stop-loss.
        let recovery_gate_secs = self.config.recovery_gate_secs as i64;
        let recovery_gate_threshold = self.config.recovery_gate_threshold;
        let recovery_gate_max_secs = self.config.recovery_gate_max_secs as i64;
        if elapsed_secs > recovery_gate_secs && loss_percent < recovery_gate_threshold {
            // Phase 2 (selective recovery gate): the gate's blanket below-threshold
            // cut was the single biggest bleed in shadow (−1.70 SOL, 30 losses at
            // −5.7% avg) because it realized temporary dips that would have
            // recovered. So we only CUT a below-threshold position when it is
            // ALSO at/beyond the hard floor (a genuine dump) OR it has stayed
            // below threshold past the longer re-evaluation window
            // (`recovery_gate_max_secs`). A soft-band dip inside that window is
            // held for recovery instead of being realized.
            let hard_floor_reached = loss_percent <= self.config.recovery_gate_hard_threshold;
            let longer_window_elapsed = elapsed_secs >= recovery_gate_max_secs;
            if !hard_floor_reached && !longer_window_elapsed {
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token = token_address,
                    loss_pct = %loss_percent,
                    recovery_gate_threshold = %recovery_gate_threshold,
                    recovery_gate_hard_threshold = %self.config.recovery_gate_hard_threshold,
                    recovery_gate_max_secs,
                    "RECOVERY_GATE: soft-band dip within re-evaluation window — holding for recovery (selective gate)"
                );
            } else {
                // A genuine dump (hard floor) or a position that refused to
                // recover past the longer window: cut it. Smart-exit (Phase 1)
                // still applies — before realizing this exit on the cache price,
                // check whether the LIVE sell fill is materially worse and defer
                // for up to `defer_max_ticks`. The defer budget + the −25% hard
                // stop floor prevent bag-holding, and catastrophic losses never
                // defer.
                if self
                    .protective_stop_should_defer(
                        trade_uuid,
                        token_address,
                        entry_price,
                        loss_percent,
                    )
                    .await
                {
                    tracing::debug!(
                        trade_uuid = %trade_uuid,
                        token = token_address,
                        loss_pct = %loss_percent,
                        cache_loss_pct = %loss_percent,
                        hard_floor_reached,
                        longer_window_elapsed,
                        "RECOVERY_GATE: live sell fill materially worse than cache — deferring exit (bounded budget)"
                    );
                } else {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        wallet_address = %wallet_address,
                        token_address = token_address,
                        loss_pct = %loss_percent,
                        elapsed_secs,
                        recovery_gate_secs,
                        recovery_gate_threshold = %recovery_gate_threshold,
                        recovery_gate_hard_threshold = %self.config.recovery_gate_hard_threshold,
                        recovery_gate_max_secs,
                        "RECOVERY_GATE: Position not recovered above threshold at gate time — exiting early"
                    );
                    return StopLossAction::Exit;
                }
            }
        }

        // Get wallet WQS for dynamic stop calculation.
        // A DB failure is distinguished from a missing score: both fall back
        // to the neutral 50.0, but the error is surfaced so an operator
        // watching stop behavior can tell the dynamic stop is on defaults.
        let wallet_opt = self.db.get_wallet(wallet_address).await;
        let wqs: f64 = match wallet_opt {
            Ok(Some(w)) => w
                .wqs_score
                .map(|s| s.to_f64().unwrap_or(50.0))
                .unwrap_or(50.0),
            Ok(None) => 50.0,
            Err(e) => {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    wallet_address = %wallet_address,
                    error = %e,
                    "Failed to fetch wallet for dynamic stop — using default WQS 50"
                );
                50.0
            }
        };

        // Check if this is a consensus signal — read from SignalAggregator in-memory cache
        // (O(1), no DB query per position per 5-second tick).
        let is_consensus = {
            // Clone the Arc under a short-lived guard and drop the guard before
            // awaiting: the in-memory check (and the DB fallback) must not hold
            // the signal_aggregator write lock open across an await.
            let agg = { self.signal_aggregator.read().await.clone() };
            if let Some(ref agg) = agg {
                agg.is_consensus_token(token_address).await
            } else {
                // Fallback: DB query when in-memory aggregator is not wired
                let count = match self.db.pool() {
                    crate::db_abstraction::DbPool::PostgreSQL(ref pool) => {
                        let c: i64 = match sqlx::query_scalar(
                            "SELECT COUNT(DISTINCT wallet_address) FROM signal_aggregation WHERE token_address = $1 AND created_at >= NOW() - INTERVAL '5 minutes'",
                        )
                        .bind(token_address)
                        .fetch_one(pool)
                        .await
                        {
                            Ok(c) => c,
                            Err(e) => {
                                // A DB failure must not silently read as "no
                                // consensus" — that would tighten stops during
                                // exactly the outage when widening matters.
                                tracing::error!(
                                    trade_uuid = %trade_uuid,
                                    token_address = %token_address,
                                    error = %e,
                                    "Failed to query consensus for stop-loss; treating as no consensus"
                                );
                                0
                            }
                        };
                        c >= 2
                    }
                };
                count
            }
        };

        // Calculate base dynamic stop-loss threshold using compile-time Decimal constants.
        // High-WQS wallets get wider stops to let proven signals breathe; low-WQS gets tighter.
        let mut stop_loss_threshold = if wqs >= 70.0 {
            dec!(-20) // High WQS: wider stop
        } else if wqs >= 40.0 {
            dec!(-15) // Medium WQS: standard stop
        } else {
            dec!(-10) // Low WQS: tighter stop
        };

        // ATR-based stop-loss override (if enabled and ATR calculation is available)
        if self.config.atr_stop_loss_enabled {
            if let Some(volatility_f64) = self.price_cache.calculate_volatility(token_address) {
                let market_regime = self.get_market_regime();
                let atr_value = Decimal::from_f64(volatility_f64).unwrap_or(Decimal::ZERO);
                let atr_stop_price =
                    self.calculate_atr_stop_loss(entry_price, atr_value, market_regime);

                // Convert ATR stop price to percentage threshold
                let atr_loss_percent = ((atr_stop_price - entry_price) / entry_price) * dec!(100.0);

                // Use ATR-based threshold if it's more conservative than the WQS-based threshold
                if atr_loss_percent > stop_loss_threshold {
                    stop_loss_threshold = atr_loss_percent;
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        atr_loss_percent = %atr_loss_percent,
                        market_regime = ?market_regime,
                        "ATR-based stop-loss applied (overrides WQS-based threshold)"
                    );
                }
            }
        }

        // Adaptive stop-loss: adjust based on token volatility (ATR-like calculation).
        // If token is highly volatile, widen stops to avoid getting wicked out.
        // Skipped when atr_stop_loss_enabled: the ATR override above already
        // applies this same volatility reading — applying it a second time
        // would compound one signal into two adjustments.
        if !self.config.atr_stop_loss_enabled {
            if let Some(volatility) = self.price_cache.calculate_volatility(token_address) {
                // Volatility is returned as percentage (e.g., 15.0 = 15%)
                // If volatility > 20%, widen stop by 1.5x
                // If volatility > 30%, widen stop by 2x
                // If volatility < 10%, tighten stop by 0.9x
                let volatility_multiplier = if volatility > 30.0 {
                    dec!(2.0)
                } else if volatility > 20.0 {
                    dec!(1.5)
                } else if volatility < 10.0 {
                    dec!(0.9)
                } else {
                    Decimal::ONE
                };

                stop_loss_threshold *= volatility_multiplier;

                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    volatility_percent = volatility,
                    adjusted_threshold = %stop_loss_threshold,
                    "Adaptive stop-loss adjusted based on volatility"
                );
            }
        }

        // Widen stop-loss for consensus signals (applied after volatility).
        // Use a proportional 25% widening instead of a flat -5% so that tight stops
        // receive smaller absolute widening than wide stops — a flat -5% on a -10% base
        // would be a 50% widening, disproportionate relative to a -20% base.
        // A second clamp is applied immediately after so the combined result respects the envelope.
        if is_consensus {
            stop_loss_threshold *= dec!(1.25); // widen by 25% of current threshold
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                consensus_threshold = %stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 25%"
            );
        }

        let adaptive_threshold = stop_loss_threshold;

        // Final clamp: never tighter than -5% or wider than the operator-configured maximum.
        let widest_stop = self.config.max_stop_loss_distance;
        let tightest_stop = dec!(-5);
        stop_loss_threshold = stop_loss_threshold.max(widest_stop).min(tightest_stop);
        // Absolute floor: never wider than -25% regardless of config.
        // Changed from -35% to -25% to allow hard stop to trigger for catastrophic drops.
        // At 20% portfolio heat cap a single -25% stop wipes 5% of total capital.
        stop_loss_threshold = stop_loss_threshold.max(dec!(-25));

        // Warn when max_stop_loss_distance overrides adaptive widening so the operator can
        // see in logs that volatile/consensus tokens are being stopped tighter than intended.
        // To allow adaptive stops to breathe, set max_stop_loss_distance to a larger negative
        // value (e.g. -50) in config.yaml.
        if widest_stop > adaptive_threshold {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                adaptive_threshold = %adaptive_threshold,
                max_stop_loss_distance = %self.config.max_stop_loss_distance,
                effective_threshold = %stop_loss_threshold,
                "Adaptive stop-loss widening overridden by max_stop_loss_distance; \
                 set max_stop_loss_distance to a larger negative (e.g. -50) to let adaptive stops breathe"
            );
        }

        if loss_percent <= stop_loss_threshold {
            // Mark validation (2026-08-08): a single bad price observation
            // must not stop a position. Before honoring any stop breach,
            // force-refresh the price once; if the fresh mark no longer
            // breaches, hold. Verified: a -13.8% cache mark that was actually
            // -2.05% (Jupiter quote) stopped a position 2s after entry.
            if let Some(fresh_usd) = self.price_cache.refresh_price_usd(token_address).await {
                let fresh_loss = if !entry_price.is_zero() {
                    ((fresh_usd - entry_price) / entry_price) * dec!(100.0)
                } else {
                    loss_percent
                };
                if fresh_loss > stop_loss_threshold {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        cached_loss_pct = %loss_percent,
                        fresh_loss_pct = %fresh_loss,
                        stop_loss_threshold = %stop_loss_threshold,
                        "STOP_MARK_REJECTED: fresh quote no longer breaches stop — holding position"
                    );
                    return StopLossAction::None;
                }
                if fresh_loss > loss_percent {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        cached_loss_pct = %loss_percent,
                        fresh_loss_pct = %fresh_loss,
                        "STOP_MARK_DIVERGENT: cached mark worse than fresh quote — exiting on fresh mark"
                    );
                }
            }

            if elapsed_secs < self.config.wick_protection_secs as i64 {
                // Hard stop at -25% always bypasses wick protection — a 25%+ crash
                // in the first seconds is never "normal entry slippage."
                if is_hard_stop {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        current_price = %current_price,
                        entry_price = %entry_price,
                        loss_percent = %loss_percent,
                        stop_loss_threshold = %stop_loss_threshold,
                        wqs,
                        is_consensus,
                        wick_elapsed_secs = elapsed_secs,
                        is_hard_stop = true,
                        exit_signal = ?StopLossAction::Exit,
                        "Hard stop at -25% triggered during wick protection window — catastrophic drop bypasses grace period"
                    );
                    return StopLossAction::Exit;
                }

                // Large-loss override: a sustained loss beyond
                // wick_protection_max_loss_percent is a genuine dump, not an
                // entry wick — exit even within the grace period. Without this,
                // fast pump.fun dumps ride unprotected through the first 60s
                // (only the -25% hard stop applied), producing -10%..-14% losses.
                if loss_percent <= self.config.wick_protection_max_loss_percent {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        current_price = %current_price,
                        entry_price = %entry_price,
                        loss_percent = %loss_percent,
                        wick_protection_max_loss_percent = %self.config.wick_protection_max_loss_percent,
                        wick_elapsed_secs = elapsed_secs,
                        exit_signal = ?StopLossAction::Exit,
                        "Large loss during wick-protection window — overriding grace period (genuine dump, not entry wick)"
                    );
                    return StopLossAction::Exit;
                }

                tracing::info!(
                    trade_uuid = %trade_uuid,
                    elapsed_secs,
                    wick_protection_secs = self.config.wick_protection_secs,
                    loss_percent = %loss_percent,
                    "Stop-loss triggered but ignored due to entry grace period (wick protection)"
                );
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    current_price = %current_price,
                    entry_price = %entry_price,
                    loss_percent = %loss_percent,
                    stop_loss_threshold = %stop_loss_threshold,
                    wqs,
                    is_consensus,
                    wick_elapsed_secs = elapsed_secs,
                    is_hard_stop,
                    "Stop-loss within wick protection window — holding position"
                );
                return StopLossAction::None;
            }

            if self
                .protective_stop_should_defer(
                    trade_uuid,
                    token_address,
                    entry_price,
                    loss_percent,
                )
                .await
            {
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token = token_address,
                    loss_pct = %loss_percent,
                    cache_loss_pct = %loss_percent,
                    wqs,
                    "Adaptive stop: live sell fill materially worse than cache — deferring exit (bounded budget)"
                );
            } else {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    current_price = %current_price,
                    entry_price = %entry_price,
                    loss_percent = %loss_percent,
                    stop_loss_threshold = %stop_loss_threshold,
                    wqs,
                    is_consensus,
                    wick_elapsed_secs = elapsed_secs,
                    is_hard_stop,
                    exit_signal = ?StopLossAction::Exit,
                    "STOP-LOSS TRIGGERED — exiting position"
                );
                return StopLossAction::Exit;
            }
        }

        tracing::debug!(
            trade_uuid = %trade_uuid,
            token_address = token_address,
            current_price = %current_price,
            entry_price = %entry_price,
            loss_percent = %loss_percent,
            stop_loss_threshold = %stop_loss_threshold,
            wqs,
            is_consensus,
            wick_elapsed_secs = elapsed_secs,
            is_hard_stop,
            "Stop-loss NOT triggered — holding position"
        );

        StopLossAction::None
    }

    /// Pre-graduation exit rail (2026-08-07, Phase 5): exit a pump.fun curve
    /// token when its bonding curve enters the late-curve dump zone — above
    /// `pre_graduation_exit_threshold` completion but not yet complete — so
    /// the position is closed BEFORE the depth discontinuity at graduation.
    /// Research: late-curve is the dump zone (arxiv 2602.14860); 86% of pump
    /// tokens dump within 5 min of peak.
    ///
    /// Fail-open: RPC errors, non-curve tokens, and missing parser all return
    /// `None` — the stop-loss/recovery rails remain the safety net. The check
    /// is skipped entirely when `pre_graduation_exit_enabled` is false.
    pub async fn check_pre_graduation(
        &self,
        trade_uuid: &str,
        token_address: &str,
    ) -> StopLossAction {
        if !self.config.pre_graduation_exit_enabled {
            return StopLossAction::None;
        }
        let parser = {
            let guard = self.token_parser.read().await;
            guard.clone()
        };
        let Some(parser) = parser else {
            return StopLossAction::None;
        };
        match parser.get_bonding_curve_state(token_address).await {
            Ok(Some(curve)) => {
                if curve.complete {
                    return StopLossAction::None; // already graduated — no curve dump zone left
                }
                let completion = curve.completion_pct();
                let threshold = self
                    .config
                    .pre_graduation_exit_threshold
                    .to_f64()
                    .unwrap_or(0.85);
                if completion > threshold {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        completion_pct = completion,
                        threshold,
                        "PRE_GRADUATION_DUMP_ZONE: curve above threshold — exiting before graduation depth discontinuity"
                    );
                    return StopLossAction::Exit;
                }
            }
            Ok(None) => {
                tracing::debug!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    "Pre-graduation check: not a pump.fun curve token — skipping (fail-open)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    token_address = token_address,
                    error = %e,
                    "Pre-graduation check: curve fetch failed — skipping (fail-open)"
                );
            }
        }
        StopLossAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price_cache::PriceSource;
    use rust_decimal_macros::dec;

    #[test]
    fn test_calculate_atr_paths() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());
        let mgr = StopLossManager::new(db, Arc::new(ProfitManagementConfig::default()), cache);

        // Zero period → None.
        assert!(mgr.calculate_atr(&[dec!(1), dec!(2)], 0).is_none());
        // Not enough samples for the period → None.
        assert!(mgr.calculate_atr(&[dec!(1), dec!(2)], 2).is_none());
        // Empty history → None.
        assert!(mgr.calculate_atr(&[], 1).is_none());

        // Period 2 over [1, 3, 5]: ranges |3-1|=2, |5-3|=2 → ATR 2.
        let atr = mgr.calculate_atr(&[dec!(1), dec!(3), dec!(5)], 2).unwrap();
        assert_eq!(atr, dec!(2));

        // Period 2 over [1, 3, 5, 4]: only the last 3 closes matter →
        // ranges |5-3|=2, |4-5|=1 → ATR 1.5.
        let atr = mgr
            .calculate_atr(&[dec!(1), dec!(3), dec!(5), dec!(4)], 2)
            .unwrap();
        assert_eq!(atr, dec!(1.5));
    }

    #[test]
    fn test_market_regime_parsing_and_multipliers() {
        assert_eq!(MarketRegime::parse_regime("BULL"), MarketRegime::Bull);
        assert_eq!(MarketRegime::parse_regime("bear"), MarketRegime::Bear);
        assert_eq!(
            MarketRegime::parse_regime("Volatile"),
            MarketRegime::Volatile
        );
        assert_eq!(MarketRegime::parse_regime("NEUTRAL"), MarketRegime::Neutral);
        // Unknown → Neutral (with a warn log).
        assert_eq!(MarketRegime::parse_regime("typo"), MarketRegime::Neutral);

        assert_eq!(MarketRegime::Bull.atr_multiplier(), dec!(1.5));
        assert_eq!(MarketRegime::Bear.atr_multiplier(), dec!(1.0));
        assert_eq!(MarketRegime::Volatile.atr_multiplier(), dec!(2.0));
        assert_eq!(MarketRegime::Neutral.atr_multiplier(), dec!(1.25));
    }

    #[test]
    fn test_calculate_atr_stop_loss_and_regime_getter() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());
        let config = Arc::new(ProfitManagementConfig {
            atr_multiplier: dec!(2.0),
            market_regime: "BULL".to_string(),
            ..ProfitManagementConfig::default()
        });
        let mgr = StopLossManager::new(db, config.clone(), cache);

        assert_eq!(mgr.get_market_regime(), MarketRegime::Bull);

        // entry 100, ATR 5, atr_multiplier 2, regime Bull 1.5 →
        // distance = 5*2*1.5 = 15% → stop = 85.
        let stop = mgr.calculate_atr_stop_loss(dec!(100), dec!(5), MarketRegime::Bull);
        assert_eq!(stop, dec!(85));

        // Volatile regime: distance 5*2*2 = 20% → stop 80.
        let stop = mgr.calculate_atr_stop_loss(dec!(100), dec!(5), MarketRegime::Volatile);
        assert_eq!(stop, dec!(80));
    }

    #[test]
    fn test_set_signal_aggregator_and_token_parser() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());
        let mgr = StopLossManager::new(db, Arc::new(ProfitManagementConfig::default()), cache);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let agg = Arc::new(crate::monitoring::SignalAggregator::new(Arc::new(
                crate::monitoring::test_db::MockDb::new(),
            )));
            mgr.set_signal_aggregator(agg.clone()).await;
            assert!(mgr.signal_aggregator.read().await.is_some());

            let cache2 = Arc::new(crate::token::TokenCache::new(10, 10));
            let fetcher = Arc::new(
                crate::token::TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
                    "http://127.0.0.1:1",
                    None,
                    "http://127.0.0.1:1".to_string(),
                ),
            );
            let parser = Arc::new(crate::TokenParser::new(
                crate::token::TokenSafetyConfig {
                    freeze_authority_whitelist: std::collections::HashSet::new(),
                    mint_authority_whitelist: std::collections::HashSet::new(),
                    min_liquidity_shield_usd: dec!(0),
                    min_liquidity_spear_usd: dec!(0),
                    honeypot_detection_enabled: false,
                    holder_concentration_check_enabled: false,
                    max_holder_concentration_pct: 100.0,
                },
                cache2,
                fetcher,
            ));
            mgr.set_token_parser(parser).await;
            assert!(mgr.token_parser.read().await.is_some());
        });
    }

    #[tokio::test]
    async fn test_check_stop_loss_stale_and_missing_price() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());
        let mgr = StopLossManager::new(
            db,
            Arc::new(ProfitManagementConfig::default()),
            cache.clone(),
        );
        let entry = chrono::Utc::now() - chrono::TimeDelta::seconds(60);

        // No cached price, token not tracked → None (fail-open).
        assert_eq!(
            mgr.check_stop_loss("u1", "w", dec!(1), "tok-unknown", entry)
                .await,
            StopLossAction::None
        );

        // Tracked token whose price went stale → force Exit.
        cache.track_token("tok-stale");
        let old = chrono::Utc::now() - chrono::TimeDelta::seconds(200);
        cache.set_price_with_time("tok-stale", dec!(1), PriceSource::Jupiter, old, Some(9));
        assert_eq!(
            mgr.check_stop_loss("u2", "w", dec!(1), "tok-stale", entry)
                .await,
            StopLossAction::Exit
        );

        // Cached but stale (>90s) on a tracked token → force Exit.
        cache.track_token("tok-freshish");
        cache.set_price_with_time("tok-freshish", dec!(1), PriceSource::Jupiter, old, Some(9));
        assert_eq!(
            mgr.check_stop_loss("u3", "w", dec!(1), "tok-freshish", entry)
                .await,
            StopLossAction::Exit
        );
    }

    #[tokio::test]
    async fn test_check_stop_loss_zero_entry_forces_exit() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());
        cache.set_price("tok-z", dec!(1), PriceSource::Jupiter, Some(9));
        let mgr = StopLossManager::new(db, Arc::new(ProfitManagementConfig::default()), cache);
        let entry = chrono::Utc::now() - chrono::TimeDelta::seconds(60);
        assert_eq!(
            mgr.check_stop_loss("u", "w", dec!(0), "tok-z", entry).await,
            StopLossAction::Exit
        );
    }

    #[tokio::test]
    async fn test_check_pre_graduation_gates() {
        let db = Arc::new(crate::monitoring::test_db::MockDb::new());
        let cache = Arc::new(PriceCache::new().unwrap());

        // Disabled → None immediately.
        let mgr = StopLossManager::new(
            db.clone(),
            Arc::new(ProfitManagementConfig {
                pre_graduation_exit_enabled: false,
                ..ProfitManagementConfig::default()
            }),
            cache.clone(),
        );
        assert_eq!(
            mgr.check_pre_graduation("u", "tok").await,
            StopLossAction::None
        );

        // Enabled but no parser wired → None (fail-open).
        let mgr = StopLossManager::new(
            db,
            Arc::new(ProfitManagementConfig {
                pre_graduation_exit_enabled: true,
                ..ProfitManagementConfig::default()
            }),
            cache,
        );
        assert_eq!(
            mgr.check_pre_graduation("u", "tok").await,
            StopLossAction::None
        );
    }
}
