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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn sell_swap(token: &str, amount_in: Decimal, amount_out: Decimal) -> ParsedSwap {
        ParsedSwap {
            token_in: token.to_string(),
            token_out: "So11111111111111111111111111111111111111112".to_string(),
            amount_in,
            amount_out,
            direction: SwapDirection::Sell,
            dex: "Jupiter".to_string(),
            slippage: None,
        }
    }

    fn buy_swap(token: &str) -> ParsedSwap {
        ParsedSwap {
            token_in: "So11111111111111111111111111111111111111112".to_string(),
            token_out: token.to_string(),
            amount_in: Decimal::new(1, 0),
            amount_out: Decimal::new(100, 0),
            direction: SwapDirection::Buy,
            dex: "Jupiter".to_string(),
            slippage: None,
        }
    }

    fn signal(wallet: &str, token: &str, delay: u64) -> ExitSignal {
        ExitSignal {
            wallet_address: wallet.to_string(),
            token_address: token.to_string(),
            exit_type: ExitType::Partial,
            delay_secs: delay,
            amount_sol: Decimal::new(1, 0),
        }
    }

    #[tokio::test]
    async fn buy_swap_does_not_detect_exit() {
        let detector = ExitDetector::new();
        let result = detector
            .detect_exit("wallet-1", &buy_swap("token-1"), 5)
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn sell_swap_detects_exit_with_capped_delay() {
        let detector = ExitDetector::new();
        let swap = sell_swap("token-1", Decimal::new(50, 0), Decimal::new(1, 0));
        let result = detector
            .detect_exit("wallet-1", &swap, 120) // delay capped at 60
            .await
            .unwrap();
        assert_eq!(result.wallet_address, "wallet-1");
        assert_eq!(result.token_address, "token-1");
        assert_eq!(result.exit_type, ExitType::Partial); // no DB -> Partial
        assert_eq!(result.delay_secs, 60);
        assert_eq!(result.amount_sol, Decimal::new(1, 0));
    }

    #[tokio::test]
    async fn take_ready_exit_not_due() {
        let detector = ExitDetector::new();
        let swap = sell_swap("token-1", Decimal::new(50, 0), Decimal::new(1, 0));
        detector.detect_exit("wallet-1", &swap, 60).await;

        // Not due yet (delay 60s, just inserted)
        let sig = signal("wallet-1", "token-1", 60);
        assert!(!detector.take_ready_exit(&sig).await);

        // Missing wallet/token
        assert!(!detector
            .take_ready_exit(&signal("nobody", "token-1", 0))
            .await);
        assert!(!detector
            .take_ready_exit(&signal("wallet-1", "nothing", 0))
            .await);
    }

    #[tokio::test]
    async fn take_ready_exit_due_with_zero_delay() {
        let detector = ExitDetector::new();
        let swap = sell_swap("token-1", Decimal::new(50, 0), Decimal::new(1, 0));
        detector.detect_exit("wallet-1", &swap, 0).await;

        let sig = signal("wallet-1", "token-1", 0);
        assert!(detector.take_ready_exit(&sig).await);
        // Second take fails — entry consumed
        assert!(!detector.take_ready_exit(&sig).await);
    }

    #[tokio::test]
    async fn take_ready_exit_with_clock_error_treated_as_due() {
        let detector = ExitDetector::new();
        // Insert a FUTURE exit time -> elapsed() errors -> treated as due
        {
            let mut pending = detector.pending_exits.write().await;
            let mut inner = HashMap::new();
            let mut queue = VecDeque::new();
            queue.push_back(SystemTime::now() + Duration::from_secs(3600));
            inner.insert("token-1".to_string(), queue);
            pending.insert("wallet-1".to_string(), inner);
        }
        let sig = signal("wallet-1", "token-1", 0);
        assert!(detector.take_ready_exit(&sig).await);
    }

    #[tokio::test]
    async fn ttl_sweep_removes_stale_pending_entries() {
        let detector = ExitDetector::new();
        // Insert an entry older than PENDING_TTL
        {
            let mut pending = detector.pending_exits.write().await;
            let mut inner = HashMap::new();
            let mut queue = VecDeque::new();
            queue.push_back(SystemTime::now() - Duration::from_secs(PENDING_TTL.as_secs() + 60));
            inner.insert("token-1".to_string(), queue);
            pending.insert("wallet-1".to_string(), inner);
        }

        // A new detect_exit triggers the opportunistic sweep
        let swap = sell_swap("token-1", Decimal::new(50, 0), Decimal::new(1, 0));
        detector.detect_exit("wallet-2", &swap, 0).await;

        let pending = detector.pending_exits.read().await;
        assert!(!pending.contains_key("wallet-1"), "stale entries must be swept");
        assert!(pending.contains_key("wallet-2"));
    }

    #[tokio::test]
    async fn mark_exit_processed_removes_entries() {
        let detector = ExitDetector::new();
        let swap = sell_swap("token-1", Decimal::new(50, 0), Decimal::new(1, 0));
        detector.detect_exit("wallet-1", &swap, 0).await;
        detector.detect_exit("wallet-1", &swap, 0).await; // second pending entry

        let mut pending = detector.pending_exits.write().await;
        pending
            .entry("wallet-2".to_string())
            .or_insert_with(HashMap::new)
            .insert("token-2".to_string(), VecDeque::new());
        drop(pending);

        detector
            .mark_exit_processed(&signal("wallet-1", "token-1", 0))
            .await;
        let pending = detector.pending_exits.read().await;
        assert!(!pending.contains_key("wallet-1"), "wallet entry fully removed");
        assert!(pending.contains_key("wallet-2"), "other wallets untouched");
    }

    #[tokio::test]
    async fn mark_exit_processed_full_exit_clears_cumulative() {
        let detector = ExitDetector::new();
        detector
            .cumulative_sold
            .write()
            .await
            .insert(("wallet-1".to_string(), "token-1".to_string()), Decimal::new(9, 0));

        let mut full = signal("wallet-1", "token-1", 0);
        full.exit_type = ExitType::Full;
        detector.mark_exit_processed(&full).await;
        assert!(detector
            .cumulative_sold
            .read()
            .await
            .get(&("wallet-1".to_string(), "token-1".to_string()))
            .is_none());

        // Partial exit keeps cumulative
        let detector2 = ExitDetector::new();
        detector2
            .cumulative_sold
            .write()
            .await
            .insert(("wallet-1".to_string(), "token-1".to_string()), Decimal::new(9, 0));
        detector2
            .mark_exit_processed(&signal("wallet-1", "token-1", 0))
            .await;
        assert!(detector2
            .cumulative_sold
            .read()
            .await
            .get(&("wallet-1".to_string(), "token-1".to_string()))
            .is_some());
    }

    #[test]
    fn exit_types_are_partial_eq() {
        assert_eq!(ExitType::Full, ExitType::Full);
        assert_ne!(ExitType::Full, ExitType::Partial);
    }
}
