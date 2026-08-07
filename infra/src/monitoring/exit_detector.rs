//! Exit signal detection for tracked wallet sells
//!
//! Detects when tracked wallets exit positions and generates EXIT signals.

use crate::db_abstraction::Database;
use crate::monitoring::transaction_parser::{ParsedSwap, SwapDirection};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// How long a pending exit may sit unprocessed before it is swept as stale
/// (covers consumer shutdown/restart without blocking new detections).
const PENDING_TTL: Duration = Duration::from_secs(300);

/// Per-token pending exit queue: wallet -> token -> queue of exit times.
type PendingExitMap = HashMap<String, HashMap<String, VecDeque<SystemTime>>>;

/// Exit detector state
pub struct ExitDetector {
    /// Pending exits (wallet -> token -> queue of exit times). A queue, not a
    /// single timestamp: a second sell of the same token before the first
    /// pending exit is processed must not overwrite/discard the earlier one.
    pending_exits: Arc<RwLock<PendingExitMap>>,
    /// Cumulative token amount already sold per (wallet, token) — used so a
    /// final sell after prior partial sells is still classified as a Full exit.
    cumulative_sold: Arc<RwLock<HashMap<(String, String), rust_decimal::Decimal>>>,
    /// Database pool for position lookup (used to detect partial vs full exit)
    db: Option<Arc<dyn Database>>,
}

/// Exit signal
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitSignal {
    pub wallet_address: String,
    pub token_address: String,
    pub exit_type: ExitType,
    pub delay_secs: u64,
    /// SOL received from the sell (token amount for non-SOL quote legs is 0)
    pub amount_sol: rust_decimal::Decimal,
}

/// Exit type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitType {
    /// Full exit (wallet sold all tokens)
    Full,
    /// Partial exit (wallet reduced position)
    Partial,
}

impl ExitDetector {
    pub fn new() -> Self {
        Self {
            pending_exits: Arc::new(RwLock::new(HashMap::new())),
            cumulative_sold: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }

    pub fn with_db(mut self, db: Arc<dyn Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Process swap and detect if it's an exit
    ///
    /// # Arguments
    /// * `wallet_address` - Wallet that made the swap
    /// * `swap` - Parsed swap information
    /// * `delay_secs` - Delay before generating exit signal (0-60)
    ///
    /// # Returns
    /// Exit signal if detected, None otherwise
    pub async fn detect_exit(
        &self,
        wallet_address: &str,
        swap: &ParsedSwap,
        delay_secs: u64,
    ) -> Option<ExitSignal> {
        // Only detect SELL swaps as exits
        if swap.direction != SwapDirection::Sell {
            return None;
        }

        // Determine if this is a full or partial exit by comparing tokens sold
        // against the tracked position size.
        let exit_type = self
            .classify_exit_type(wallet_address, &swap.token_in, swap.amount_in)
            .await;

        // For SELL swaps, the exited token is token_in (what we're selling), not token_out (SOL)
        let exited_token = swap.token_in.clone();

        let mut pending = self.pending_exits.write().await;
        // Opportunistic TTL sweep: entries that were never processed (consumer
        // dropped them, shutdown, clock skew) must not leak forever.
        {
            let now = SystemTime::now();
            for wallet_exits in pending.values_mut() {
                for times in wallet_exits.values_mut() {
                    times.retain(|t| {
                        now.duration_since(*t)
                            .map(|age| age < PENDING_TTL)
                            .unwrap_or(true)
                    });
                }
            }
        }
        pending.retain(|_, wallet_exits| {
            wallet_exits.retain(|_, times| !times.is_empty());
            !wallet_exits.is_empty()
        });

        let wallet_exits = pending
            .entry(wallet_address.to_string())
            .or_insert_with(HashMap::new);
        wallet_exits
            .entry(exited_token.clone())
            .or_insert_with(VecDeque::new)
            .push_back(SystemTime::now());

        Some(ExitSignal {
            wallet_address: wallet_address.to_string(),
            token_address: exited_token,
            exit_type,
            delay_secs: delay_secs.min(60), // Cap at 60 seconds
            amount_sol: swap.amount_out,
        })
    }

    /// Atomically claim the oldest pending exit entry for this signal if it is
    /// due (its delay has elapsed). Only one consumer wins per entry, so
    /// duplicate EXIT signals cannot be emitted by concurrent consumers.
    ///
    /// A clock read failure (`elapsed()` error, e.g. system time adjusted
    /// backwards) is treated as due so the entry cannot strand forever.
    pub async fn take_ready_exit(&self, signal: &ExitSignal) -> bool {
        let mut pending = self.pending_exits.write().await;
        if let Some(wallet_exits) = pending.get_mut(&signal.wallet_address) {
            if let Some(times) = wallet_exits.get_mut(&signal.token_address) {
                if let Some(&exit_time) = times.front() {
                    let due = match exit_time.elapsed() {
                        Ok(elapsed) => elapsed.as_secs() >= signal.delay_secs,
                        Err(_) => true,
                    };
                    if due {
                        times.pop_front();
                        if times.is_empty() {
                            wallet_exits.remove(&signal.token_address);
                        }
                        if wallet_exits.is_empty() {
                            pending.remove(&signal.wallet_address);
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Classify a sell as Full or Partial by comparing tokens sold (including
    /// any prior partial sells of the same position) to the tracked position.
    ///
    /// If the wallet's ACTIVE/EXITING position can be found, the sell is
    /// classified as Full when `amount_in + prior_sold >= 90%` of the
    /// estimated position token size, else Partial.
    ///
    /// Failures to verify (missing DB, DB query error, no matching position)
    /// resolve to `Partial` — an unverifiable classification must never emit a
    /// confident "full exit" signal.
    async fn classify_exit_type(
        &self,
        wallet_address: &str,
        token_address: &str,
        amount_in: rust_decimal::Decimal,
    ) -> ExitType {
        let Some(ref db) = self.db else {
            tracing::debug!(
                wallet = %wallet_address,
                token = %token_address,
                "Exit classification without DB — defaulting to Partial"
            );
            return ExitType::Partial;
        };

        let positions = match db.get_active_positions().await {
            Ok(positions) => positions,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query active positions for exit classification");
                return ExitType::Partial;
            }
        };

        let mut cumulative = self.cumulative_sold.write().await;
        let prior_sold = cumulative
            .get(&(wallet_address.to_string(), token_address.to_string()))
            .copied()
            .unwrap_or(rust_decimal::Decimal::ZERO);

        for pos in &positions {
            if pos.wallet_address == wallet_address && pos.token_address == token_address {
                use rust_decimal::prelude::*;
                if pos.entry_price > Decimal::ZERO && pos.entry_amount_sol > Decimal::ZERO {
                    let est_tokens = pos.entry_amount_sol / pos.entry_price;
                    let remaining = (est_tokens - prior_sold).max(Decimal::ZERO);
                    let threshold = remaining * Decimal::from_str("0.9").unwrap_or(Decimal::ONE);
                    let is_full = amount_in >= threshold;

                    *cumulative
                        .entry((wallet_address.to_string(), token_address.to_string()))
                        .or_insert(Decimal::ZERO) += amount_in;

                    return if is_full { ExitType::Full } else { ExitType::Partial };
                }
                // Unparseable position data — cannot classify reliably.
                tracing::debug!(
                    wallet = %wallet_address,
                    token = %token_address,
                    "Position entry data invalid — defaulting to Partial"
                );
                return ExitType::Partial;
            }
        }

        // No tracked position for this (wallet, token): tracking state is
        // irrelevant now — drop it and stay conservative.
        cumulative.remove(&(wallet_address.to_string(), token_address.to_string()));
        ExitType::Partial
    }

    /// Mark exit as processed (remove all pending entries for this signal).
    /// Called by the exit-signal processor after the exit was dispatched.
    pub async fn mark_exit_processed(&self, signal: &ExitSignal) {
        let mut pending = self.pending_exits.write().await;
        if let Some(wallet_exits) = pending.get_mut(&signal.wallet_address) {
            wallet_exits.remove(&signal.token_address);
            if wallet_exits.is_empty() {
                pending.remove(&signal.wallet_address);
            }
        }
        // A Full exit closes the position — cumulative sold tracking is stale.
        if signal.exit_type == ExitType::Full {
            let mut cumulative = self.cumulative_sold.write().await;
            cumulative.remove(&(signal.wallet_address.clone(), signal.token_address.clone()));
        }
    }
}

impl Default for ExitDetector {
    fn default() -> Self {
        Self::new()
    }
}
