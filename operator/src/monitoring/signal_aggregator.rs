//! Multi-wallet signal aggregation with consensus detection
//!
//! Tracks signals across all ACTIVE wallets and detects:
//! - Consensus: Multiple wallets buying same token
//! - Divergence: Some wallets exiting while others hold
//! - Clusters: Wallets that trade together

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use rust_decimal::prelude::*;
use crate::db::DbPool;

/// Signal aggregator state
pub struct SignalAggregator {
    db: DbPool,
    /// Recent signals by token (for consensus detection)
    recent_signals: Arc<RwLock<HashMap<String, Vec<TokenSignal>>>>,
    /// Wallet clusters (wallets that trade together)
    wallet_clusters: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

/// Token signal from a wallet
#[derive(Debug, Clone)]

struct TokenSignal {
    wallet_address: String,
    token_address: String,
    direction: String, // BUY or SELL
    amount_sol: Decimal,
    timestamp: SystemTime,
}

/// Consensus signal (multiple wallets buying same token)
#[derive(Debug, Clone)]
pub struct ConsensusSignal {
    pub token_address: String,
    pub wallet_count: usize,
    pub total_amount_sol: Decimal,
    pub wallets: Vec<String>,
    pub confidence: f64, // 0.0 to 1.0
}

impl SignalAggregator {
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            recent_signals: Arc::new(RwLock::new(HashMap::new())),
            wallet_clusters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a signal and check for consensus
    ///
    /// # Arguments
    /// * `wallet_address` - Wallet that generated the signal
    /// * `token_address` - Token being traded
    /// * `direction` - BUY or SELL
    /// * `amount_sol` - Trade size in SOL
    ///
    /// # Returns
    /// Consensus signal if detected, None otherwise
    pub async fn add_signal(
        &self,
        wallet_address: &str,
        token_address: &str,
        direction: &str,
        amount_sol: Decimal,
    ) -> Option<ConsensusSignal> {
        // Only check consensus for BUY signals
        if direction != "BUY" {
            return None;
        }

        let signal = TokenSignal {
            wallet_address: wallet_address.to_string(),
            token_address: token_address.to_string(),
            direction: direction.to_string(),
            amount_sol,
            timestamp: SystemTime::now(),
        };

        let mut signals = self.recent_signals.write().await;

        // Clean up old signals (older than 5 minutes)
        let five_min_ago = SystemTime::now() - Duration::from_secs(300);
        signals.retain(|_, token_signals| {
            token_signals.retain(|s| s.timestamp > five_min_ago);
            !token_signals.is_empty()
        });

        // Add new signal
        let token_signals = signals.entry(token_address.to_string()).or_insert_with(Vec::new);
        token_signals.push(signal);

        // Check for consensus (2+ wallets buying same token within 5 minutes)
        if token_signals.len() >= 2 {
            let wallets: Vec<String> = token_signals.iter().map(|s| s.wallet_address.clone()).collect();
            let total_amount: Decimal = token_signals.iter().map(|s| s.amount_sol).sum();
            let confidence = (token_signals.len() as f64 / 5.0).min(1.0); // Max confidence at 5+ wallets

            // Update wallet clusters
            self.update_wallet_clusters(&wallets).await;

            return Some(ConsensusSignal {
                token_address: token_address.to_string(),
                wallet_count: token_signals.len(),
                total_amount_sol: total_amount,
                wallets,
                confidence,
            });
        }

        None
    }

    /// Check for divergence (some wallets exiting while others hold)
    ///
    /// # Arguments
    /// * `token_address` - Token to check
    /// * `exiting_wallet` - Wallet that is exiting
    ///
    /// # Returns
    /// True if divergence detected (others still hold)
    pub async fn check_divergence(&self, token_address: &str, exiting_wallet: &str) -> bool {
        let signals = self.recent_signals.read().await;
        
        if let Some(token_signals) = signals.get(token_address) {
            // Check if there are other wallets that bought this token recently
            let other_buyers: Vec<&TokenSignal> = token_signals
                .iter()
                .filter(|s| {
                    s.direction == "BUY" 
                        && s.wallet_address != exiting_wallet
                        && s.timestamp > SystemTime::now() - Duration::from_secs(3600) // Within 1 hour
                })
                .collect();

            return !other_buyers.is_empty();
        }

        false
    }

    /// Update wallet clusters (wallets that trade together)
    async fn update_wallet_clusters(&self, wallets: &[String]) {
        let mut clusters = self.wallet_clusters.write().await;

        // For each pair of wallets, record that they trade together
        for i in 0..wallets.len() {
            for j in (i + 1)..wallets.len() {
                let cluster_key = format!("{}:{}", wallets[i], wallets[j]);
                clusters.entry(cluster_key).or_insert_with(Vec::new);
            }
        }
    }

    /// Get wallet cluster (wallets that trade with this wallet)
    pub async fn get_wallet_cluster(&self, wallet_address: &str) -> Vec<String> {
        let clusters = self.wallet_clusters.read().await;
        let mut related_wallets = Vec::new();

        for (key, _) in clusters.iter() {
            if key.contains(wallet_address) {
                // Extract other wallet from cluster key
                let parts: Vec<&str> = key.split(':').collect();
                if parts.len() == 2 {
                    if parts[0] == wallet_address {
                        related_wallets.push(parts[1].to_string());
                    } else if parts[1] == wallet_address {
                        related_wallets.push(parts[0].to_string());
                    }
                }
            }
        }

        related_wallets
    }
}
