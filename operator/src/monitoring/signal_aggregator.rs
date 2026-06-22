//! Multi-wallet signal aggregation with consensus detection
//!
//! Tracks signals across all ACTIVE wallets and detects:
//! - Consensus: Multiple wallets buying same token
//! - Divergence: Some wallets exiting while others hold
//! - Clusters: Wallets that trade together

use crate::db_abstraction::Database;
use rust_decimal::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// Signal aggregator state
pub struct SignalAggregator {
    #[allow(dead_code)]
    db: Arc<dyn Database>,
    /// Recent signals by token (for consensus detection)
    recent_signals: Arc<RwLock<HashMap<String, Vec<TokenSignal>>>>,
    /// Wallet clusters (wallets that trade together)
    wallet_clusters: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

/// Token signal from a wallet
#[derive(Debug, Clone)]
struct TokenSignal {
    #[allow(dead_code)]
    wallet_address: String,
    #[allow(dead_code)]
    token_address: String,
    direction: String, // BUY or SELL
    amount_sol: Decimal,
    timestamp: Instant,
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
    pub fn new(db: Arc<dyn Database>) -> Self {
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
            timestamp: Instant::now(),
        };

        let mut signals = self.recent_signals.write().await;

        // Clean up old signals (older than 5 minutes)
        let five_min_ago = Instant::now() - Duration::from_secs(300);
        signals.retain(|_, token_signals| {
            token_signals.retain(|s| s.timestamp > five_min_ago);
            !token_signals.is_empty()
        });

        // Add new signal
        let token_signals = signals
            .entry(token_address.to_string())
            .or_insert_with(Vec::new);
        token_signals.push(signal);

        // Check for consensus (2+ DISTINCT wallets buying same token within 5 minutes).
        // Dedup by wallet address so a single wallet retrying cannot fake consensus.
        let mut seen = std::collections::HashSet::new();
        let unique_wallets: Vec<String> = token_signals
            .iter()
            .filter(|s| seen.insert(s.wallet_address.clone()))
            .map(|s| s.wallet_address.clone())
            .collect();

        if unique_wallets.len() >= 2 {
            let total_amount: Decimal = token_signals.iter().map(|s| s.amount_sol).sum();
            let confidence = (unique_wallets.len() as f64 / 5.0).min(1.0); // Max confidence at 5+ wallets

            // Update wallet clusters
            self.update_wallet_clusters(&unique_wallets).await;

            return Some(ConsensusSignal {
                token_address: token_address.to_string(),
                wallet_count: unique_wallets.len(),
                total_amount_sol: total_amount,
                wallets: unique_wallets,
                confidence,
            });
        }

        None
    }

    /// Return true if 2+ distinct wallets have BUY signals for this token in the last 5 minutes.
    /// Reads from the in-memory cache — no DB query needed.
    pub async fn is_consensus_token(&self, token_address: &str) -> bool {
        let signals = self.recent_signals.read().await;
        let five_min_ago = Instant::now() - Duration::from_secs(300);
        if let Some(token_signals) = signals.get(token_address) {
            let mut seen = std::collections::HashSet::new();
            for s in token_signals {
                if s.direction == "BUY" && s.timestamp > five_min_ago {
                    seen.insert(&s.wallet_address);
                }
            }
            return seen.len() >= 2;
        }
        false
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
        // Evict signals older than 1 hour before checking divergence.
        // add_signal() only cleans up on the 5-minute window; without this, stale
        // entries accumulate here indefinitely causing false divergence positives.
        {
            let cutoff = Instant::now() - Duration::from_secs(3600);
            let mut signals = self.recent_signals.write().await;
            signals.retain(|_, signals| {
                signals.retain(|s| s.timestamp > cutoff);
                !signals.is_empty()
            });
        }

        let signals = self.recent_signals.read().await;

        if let Some(token_signals) = signals.get(token_address) {
            // Check if there are other wallets that bought this token recently
            let other_buyers: Vec<&TokenSignal> = token_signals
                .iter()
                .filter(|s| {
                    s.direction == "BUY"
                        && s.wallet_address != exiting_wallet
                        && s.timestamp > Instant::now() - Duration::from_secs(3600)
                    // Within 1 hour
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
                let wallet_pair = vec![wallets[i].clone(), wallets[j].clone()];
                clusters.insert(cluster_key, wallet_pair);
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
