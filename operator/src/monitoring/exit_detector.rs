//! Exit signal detection for tracked wallet sells
//!
//! Detects when tracked wallets exit positions and generates EXIT signals.

use crate::monitoring::transaction_parser::{ParsedSwap, SwapDirection};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Exit detector state
pub struct ExitDetector {
    /// Pending exits (wallet -> token -> exit time)
    pending_exits: Arc<
        RwLock<
            std::collections::HashMap<
                String,
                std::collections::HashMap<String, std::time::SystemTime>,
            >,
        >,
    >,
    /// Database pool for position lookup (used to detect partial vs full exit)
    db: Option<crate::db::DbPool>,
}

/// Exit signal
#[derive(Debug, Clone)]
pub struct ExitSignal {
    pub wallet_address: String,
    pub token_address: String,
    pub exit_type: ExitType,
    pub delay_secs: u64,
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
            pending_exits: Arc::new(RwLock::new(std::collections::HashMap::new())),
            db: None,
        }
    }

    pub fn with_db(mut self, db: crate::db::DbPool) -> Self {
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

        // Store pending exit
        let mut pending = self.pending_exits.write().await;
        let wallet_exits = pending
            .entry(wallet_address.to_string())
            .or_insert_with(std::collections::HashMap::new);
        wallet_exits.insert(exited_token.clone(), std::time::SystemTime::now());

        Some(ExitSignal {
            wallet_address: wallet_address.to_string(),
            token_address: exited_token,
            exit_type,
            delay_secs: delay_secs.min(60), // Cap at 60 seconds
        })
    }

    /// Check if exit signal should be generated (after delay)
    pub async fn should_generate_exit(&self, signal: &ExitSignal) -> bool {
        let pending = self.pending_exits.read().await;

        if let Some(wallet_exits) = pending.get(&signal.wallet_address) {
            if let Some(&exit_time) = wallet_exits.get(&signal.token_address) {
                if let Ok(elapsed) = exit_time.elapsed() {
                    return elapsed.as_secs() >= signal.delay_secs;
                }
            }
        }

        false
    }

    /// Classify a sell as Full or Partial by comparing tokens sold to the tracked position.
    ///
    /// If the wallet's ACTIVE/EXITING position can be found, the sell is classified as Full
    /// when `amount_in >= 90%` of the estimated position token size, else Partial.
    /// Falls back to Full when no position data is available.
    async fn classify_exit_type(
        &self,
        wallet_address: &str,
        token_address: &str,
        amount_in: rust_decimal::Decimal,
    ) -> ExitType {
        let Some(ref pool) = self.db else {
            return ExitType::Full;
        };

        let row: Option<(f64, f64)> = sqlx::query_as(
            "SELECT entry_amount_sol, entry_price FROM positions \
             WHERE wallet_address = ? AND token_address = ? AND state IN ('ACTIVE', 'EXITING') \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(wallet_address)
        .bind(token_address)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);

        if let Some((entry_amount_sol, entry_price)) = row {
            if entry_price > 0.0 && entry_amount_sol > 0.0 {
                use rust_decimal::prelude::*;
                let est_tokens = Decimal::from_f64_retain(entry_amount_sol / entry_price)
                    .unwrap_or(Decimal::ZERO);
                let threshold = est_tokens * Decimal::from_str("0.9").unwrap_or(Decimal::ONE);
                if amount_in < threshold {
                    return ExitType::Partial;
                }
            }
        }

        ExitType::Full
    }

    /// Mark exit as processed
    pub async fn mark_exit_processed(&self, signal: &ExitSignal) {
        let mut pending = self.pending_exits.write().await;
        if let Some(wallet_exits) = pending.get_mut(&signal.wallet_address) {
            wallet_exits.remove(&signal.token_address);
            if wallet_exits.is_empty() {
                pending.remove(&signal.wallet_address);
            }
        }
    }
}

impl Default for ExitDetector {
    fn default() -> Self {
        Self::new()
    }
}
