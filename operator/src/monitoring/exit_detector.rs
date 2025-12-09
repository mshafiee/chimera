//! Exit signal detection for tracked wallet sells
//!
//! Detects when tracked wallets exit positions and generates EXIT signals.

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::monitoring::transaction_parser::{ParsedSwap, SwapDirection};

/// Exit detector state
pub struct ExitDetector {
    /// Pending exits (wallet -> token -> exit time)
    pending_exits: Arc<RwLock<std::collections::HashMap<String, std::collections::HashMap<String, std::time::SystemTime>>>>,
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
        }
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

        // Check if this is a full or partial exit
        // For now, assume full exit (would need position tracking to determine partial)
        let exit_type = ExitType::Full;

        // Store pending exit
        let mut pending = self.pending_exits.write().await;
        let wallet_exits = pending.entry(wallet_address.to_string()).or_insert_with(std::collections::HashMap::new);
        wallet_exits.insert(swap.token_out.clone(), std::time::SystemTime::now());

        Some(ExitSignal {
            wallet_address: wallet_address.to_string(),
            token_address: swap.token_out.clone(),
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
