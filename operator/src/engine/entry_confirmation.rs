//! Entry confirmation for single-wallet signals from unproven wallets.
//!
//! The consensus-OR-proven admission gate rejects single-wallet BUY signals
//! from wallets without a proven copy-trade record — the negative-EV class.
//! But a whale can still be RIGHT: the token may hold after their entry. Entry
//! confirmation gives these signals a second chance under a price-hold test:
//!
//! 1. At signal time the handler registers a pending entry with the whale's
//!    actual entry price (`amount_in / amount_out`, SOL per raw unit).
//! 2. After `wait_secs`, the background loop quotes the token's current price.
//! 3. If the price held (>= entry × (1 − max_drawdown_pct)), the signal is
//!    re-evaluated through the full selection pipeline with the consensus
//!    gate bypassed (the price-hold replaces the hard gate as the admission
//!    criterion). If it dumped, the entry is dropped.
//!
//! This replaces "buy the whale's fresh buy at the top" with "buy only if the
//! whale's entry is holding" — the pattern that produced all 134 closed trades
//! (-0.72 SOL) at -0.0055 SOL/trade average.

use crate::db_abstraction::{Database, InsertTrade, UpdateTradeStatus};
use crate::engine::selection::{BuyDecision, SelectionRequest, SelectionService};
use crate::engine::EngineHandle;
use crate::models::{Signal, SignalPayload, Strategy};
use crate::token::TokenParser;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Entry confirmation tuning knobs.
#[derive(Debug, Clone, Copy)]
pub struct EntryConfirmationConfig {
    /// Master switch (default true).
    pub enabled: bool,
    /// How long to wait after the whale's entry before checking the hold
    /// (default 300s).
    pub wait_secs: u64,
    /// Maximum allowed drawdown from the whale's entry price (percent,
    /// default 3.0). The token must hold within this tolerance to be admitted.
    pub max_drawdown_pct: Decimal,
}

impl Default for EntryConfirmationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            wait_secs: 300,
            max_drawdown_pct: dec!(3.0),
        }
    }
}

/// Whether `current_price` held within `max_drawdown_pct` of `ref_price`.
/// Both prices must be in the same unit space (SOL per raw base unit).
/// Fail-closed: zero/negative prices never count as "held".
pub fn price_held(ref_price: Decimal, current_price: Decimal, max_drawdown_pct: Decimal) -> bool {
    if ref_price <= Decimal::ZERO || current_price <= Decimal::ZERO {
        return false;
    }
    if max_drawdown_pct < Decimal::ZERO {
        return false;
    }
    let max_drop = max_drawdown_pct / Decimal::from(100);
    current_price >= ref_price * (Decimal::ONE - max_drop)
}

#[derive(Debug, Clone)]
struct PendingEntry {
    req: SelectionRequest,
    /// Whale's entry price: amount_in / amount_out (SOL per raw base unit).
    ref_price_sol_per_raw: Decimal,
    confirm_at: Instant,
}

/// Shared pending-entry store + background confirmation loop.
pub struct EntryConfirmationManager {
    config: EntryConfirmationConfig,
    pending: Mutex<HashMap<String, PendingEntry>>,
    db: Arc<dyn Database>,
    engine: EngineHandle,
    token_parser: Option<Arc<TokenParser>>,
    selection: Arc<SelectionService>,
}

impl EntryConfirmationManager {
    pub fn new(
        config: EntryConfirmationConfig,
        db: Arc<dyn Database>,
        engine: EngineHandle,
        token_parser: Option<Arc<TokenParser>>,
        selection: Arc<SelectionService>,
    ) -> Self {
        Self {
            config,
            pending: Mutex::new(HashMap::new()),
            db,
            engine,
            token_parser,
            selection,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Register a single-wallet unproven BUY signal for price-hold
    /// confirmation. Returns false when confirmation is disabled or the token
    /// already has a pending entry (keeps one confirmation per token).
    pub async fn register(&self, req: SelectionRequest, ref_price_sol_per_raw: Decimal) -> bool {
        if !self.config.enabled || ref_price_sol_per_raw <= Decimal::ZERO {
            return false;
        }
        let mut pending = self.pending.lock().await;
        if pending.contains_key(&req.token_address) {
            tracing::debug!(
                token = %req.token_address,
                "Entry confirmation: token already pending — skipping duplicate"
            );
            return false;
        }
        pending.insert(
            req.token_address.clone(),
            PendingEntry {
                req,
                ref_price_sol_per_raw,
                confirm_at: Instant::now() + Duration::from_secs(self.config.wait_secs),
            },
        );
        true
    }

    /// Spawn the background confirmation loop (10s tick).
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                self.check_due().await;
            }
        });
    }

    /// Evaluate all entries whose confirmation window has elapsed.
    async fn check_due(&self) {
        let due: Vec<(String, PendingEntry)> = {
            let mut pending = self.pending.lock().await;
            let now = Instant::now();
            let mut out = Vec::new();
            pending.retain(|token, entry| {
                if entry.confirm_at <= now {
                    out.push((token.clone(), entry.clone()));
                    false
                } else {
                    true
                }
            });
            out
        };
        for (token, entry) in due {
            self.evaluate(token, entry).await;
        }
    }

    /// Quote the token's current price (SOL per raw unit) and either admit
    /// (price held → full re-evaluation with the gate bypassed) or drop.
    async fn evaluate(&self, token: String, entry: PendingEntry) {
        let current_sol_per_raw = match self.token_parser.as_ref() {
            Some(parser) => match parser.get_token_decimals(&token).await {
                Some(decimals) => match 10u64.checked_pow(decimals as u32) {
                    Some(amount_raw) => match parser.sell_quote_out_sol(&token, amount_raw).await {
                        Ok(Some(out_sol)) => Some(out_sol / Decimal::from(amount_raw)),
                        Ok(None) => None,
                        Err(e) => {
                            tracing::debug!(
                                token = %token,
                                error = %e,
                                "Entry confirmation: quote failed — dropping pending entry (fail-closed)"
                            );
                            None
                        }
                    },
                    None => None,
                },
                None => None,
            },
            None => None,
        };

        let Some(current) = current_sol_per_raw else {
            tracing::warn!(
                token = %token,
                "ENTRY_CONFIRMATION: price unverifiable — dropping pending entry (fail-closed)"
            );
            return;
        };

        if !price_held(
            entry.ref_price_sol_per_raw,
            current,
            self.config.max_drawdown_pct,
        ) {
            let drop_pct = if entry.ref_price_sol_per_raw > Decimal::ZERO {
                ((entry.ref_price_sol_per_raw - current) / entry.ref_price_sol_per_raw)
                    * Decimal::from(100)
            } else {
                Decimal::ZERO
            };
            tracing::warn!(
                token = %token,
                ref_price_sol_per_raw = %entry.ref_price_sol_per_raw,
                current_price_sol_per_raw = %current,
                drop_pct = %drop_pct,
                max_drawdown_pct = %self.config.max_drawdown_pct,
                "ENTRY_CONFIRMATION_FAILED: token dropped below tolerance — not entering"
            );
            return;
        }

        // Price held → re-run the full pipeline with the consensus-OR-proven
        // gate bypassed (the price-hold is the admission criterion here).
        let decision = self
            .selection
            .decide_with_options(&entry.req, true)
            .await;
        if !decision.admitted {
            tracing::info!(
                token = %token,
                wallet = %entry.req.wallet_address,
                code = decision.rejection_code.unwrap_or("REJECTED"),
                reason = decision.rejection_reason.as_deref().unwrap_or("rejected"),
                "ENTRY_CONFIRMATION: price held but signal rejected by later gates"
            );
            return;
        }

        tracing::info!(
            token = %token,
            wallet = %entry.req.wallet_address,
            size_sol = ?decision.size_sol,
            "ENTRY_CONFIRMATION_PASSED: whale entry held — queuing trade"
        );
        let queued = queue_monitoring_signal(
            &self.db,
            &self.engine,
            self.token_parser.as_ref(),
            &self.selection,
            &decision,
            &entry.req,
        )
        .await;
        if !queued {
            tracing::error!(
                token = %token,
                "ENTRY_CONFIRMATION: signal admitted but failed to queue"
            );
        }
    }
}

/// Queue an admitted monitoring signal through the standard path: build the
/// time-bucketed trade UUID + payload, resolve decimals, insert the PENDING
/// trade row, link the decision record, and queue to the engine.
///
/// Shared between the Helius monitoring handler and the entry-confirmation
/// loop so both paths behave identically. Returns true when the signal was
/// queued (or deduped as an existing row).
pub(crate) async fn queue_monitoring_signal(
    db: &Arc<dyn Database>,
    engine: &EngineHandle,
    token_parser: Option<&Arc<TokenParser>>,
    selection: &SelectionService,
    decision: &BuyDecision,
    req: &SelectionRequest,
) -> bool {
    let trade_amount_sol = decision.size_sol.unwrap_or(req.source_amount_sol);
    let strategy = decision.strategy.unwrap_or(Strategy::Spear);

    // Time-bucketed UUID (5-minute buckets): duplicate webhooks within the
    // window dedup to the same UUID; after a position closes, a later window
    // mints a fresh UUID.
    let time_bucket = chrono::Utc::now().timestamp() / 300;
    let monitoring_uuid = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(req.wallet_address.as_bytes());
        hasher.update(b"|");
        hasher.update(req.token_address.as_bytes());
        hasher.update(b"|");
        hasher.update(req.action.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(trade_amount_sol.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(time_bucket.to_le_bytes());
        hex::encode(&hasher.finalize()[..16])
    };

    let signal_payload = SignalPayload {
        wallet_address: req.wallet_address.clone(),
        strategy,
        token: req.token_address.clone(),
        token_address: Some(req.token_address.clone()),
        action: req.action,
        amount_sol: trade_amount_sol,
        trade_uuid: Some(monitoring_uuid),
        exit_fraction: None,
        // Trial-lane marker (2026-09-01): thread the trial admission through
        // so the pipeline's off-hours minimum-size floor exempts it.
        trial_admission: decision.trial_admission,
    };

    let mut signal = Signal::new(signal_payload, chrono::Utc::now().timestamp(), None);

    if let Some(parser) = token_parser {
        if let Some(decimals) = parser
            .get_token_decimals(signal.token_address().unwrap_or(""))
            .await
        {
            signal.token_decimals = Some(decimals);
        }
    }

    match db
        .insert_trade(&InsertTrade {
            trade_uuid: signal.trade_uuid.clone(),
            wallet_address: signal.payload.wallet_address.clone(),
            token_address: signal.token_address().unwrap_or("").to_string(),
            token_symbol: Some(signal.payload.token.clone()),
            strategy: signal.payload.strategy.to_string(),
            side: signal.payload.action.to_string(),
            amount_sol: signal.payload.amount_sol,
            status: "PENDING".to_string(),
        })
        .await
    {
        Ok(_) => {
            if let Some(recorder) = selection.decision_recorder() {
                recorder.link_trade(decision.decision_id.clone(), signal.trade_uuid.clone());
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("duplicate key") {
                tracing::debug!(
                    trade_uuid = %signal.trade_uuid,
                    "Signal already in DB (duplicate webhook), skipping"
                );
            } else {
                tracing::error!(
                    error = %e,
                    trade_uuid = %signal.trade_uuid,
                    "Failed to insert trade from monitoring signal"
                );
            }
            return false;
        }
    }

    let wallet_wqs = decision.wqs;
    let signal_uuid = signal.trade_uuid.clone();
    if let Err(e) = engine.queue_signal(signal, wallet_wqs).await {
        tracing::error!(
            error = %e,
            trade_uuid = %signal_uuid,
            "Failed to queue signal"
        );
        let _ = db
            .update_trade_status(&UpdateTradeStatus {
                trade_uuid: signal_uuid,
                status: "FAILED".to_string(),
                tx_signature: None,
                error_message: Some(format!("Queue failed: {}", e)),
                network_fee_sol: None,
            })
            .await;
        return false;
    }

    // Update trade status to QUEUED after successful queue.
    if let Err(e) = db
        .update_trade_status(&UpdateTradeStatus {
            trade_uuid: signal_uuid.clone(),
            status: "QUEUED".to_string(),
            tx_signature: None,
            error_message: None,
            network_fee_sol: None,
        })
        .await
    {
        tracing::warn!(
            error = %e,
            trade_uuid = %signal_uuid,
            "Failed to update trade status to QUEUED"
        );
    }

    tracing::info!(
        wallet = %req.wallet_address,
        token = %req.token_address,
        trade_uuid = %signal_uuid,
        "Queued signal"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_held_accepts_flat_and_rise() {
        assert!(price_held(dec!(1.0), dec!(1.0), dec!(3.0)));
        assert!(price_held(dec!(1.0), dec!(1.05), dec!(3.0)));
        assert!(price_held(dec!(1.0), dec!(0.97), dec!(3.0)));
    }

    #[test]
    fn price_held_rejects_drop_beyond_tolerance() {
        assert!(!price_held(dec!(1.0), dec!(0.969), dec!(3.0)));
        assert!(!price_held(dec!(1.0), dec!(0.5), dec!(3.0)));
    }

    #[test]
    fn price_held_fails_closed_on_bad_input() {
        assert!(!price_held(dec!(0.0), dec!(1.0), dec!(3.0)));
        assert!(!price_held(dec!(1.0), dec!(0.0), dec!(3.0)));
        assert!(!price_held(dec!(-1.0), dec!(1.0), dec!(3.0)));
        assert!(!price_held(dec!(1.0), dec!(1.0), dec!(-1.0)));
    }
}
