//! Multi-wallet signal aggregation with consensus detection
//!
//! Tracks signals across all ACTIVE wallets and detects:
//! - Consensus: Multiple wallets buying same token
//! - Divergence: Some wallets exiting while others hold
//! - Clusters: Wallets that trade together
//! - Smart-money cluster: N statistically-profitable wallets converging on
//!   the same token within a window (research-backed signal — Nansen:
//!   "10+ smart money wallets buying the same token within 48 hours =
//!   coordinated conviction"; arxiv 2601.08641: wallet selection is the
//!   dominant copier-profitability factor)

use crate::db_abstraction::Database;
use rust_decimal::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// T-statistic configuration for wallet profitability classification.
#[derive(Debug, Clone, Copy)]
pub struct WalletTstatConfig {
    pub threshold: f64,
    pub min_samples: i32,
    pub window_days: i32,
}

impl Default for WalletTstatConfig {
    fn default() -> Self {
        Self {
            threshold: 1.645,
            min_samples: 10,
            window_days: 30,
        }
    }
}

/// Smart-money cluster window: how long a wallet's BUY signal stays relevant
/// for cluster detection. Longer than the 5-minute consensus window so that
/// coordinated accumulation over hours is captured (Nansen: 48h window).
pub const CLUSTER_WINDOW_SECS: u64 = 12 * 3600;

/// Signal aggregator state
pub struct SignalAggregator {
    #[allow(dead_code)]
    db: Arc<dyn Database>,
    /// Recent signals by token (for consensus detection)
    recent_signals: Arc<RwLock<HashMap<String, Vec<TokenSignal>>>>,
    /// Wallet clusters (wallets that trade together)
    wallet_clusters: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Cached wallet profitability (t-stat pass/fail) with TTL.
    profitable_cache: Arc<RwLock<HashMap<String, (bool, Instant)>>>,
    /// T-stat config for the smart-money cluster signal.
    tstat_config: WalletTstatConfig,
}

/// Token signal from a wallet
#[derive(Debug, Clone)]
pub struct TokenSignal {
    #[allow(dead_code)]
    pub wallet_address: String,
    #[allow(dead_code)]
    pub token_address: String,
    pub direction: String, // BUY or SELL
    pub amount_sol: Decimal,
    pub timestamp: Instant,
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
        Self::with_tstat_config(db, WalletTstatConfig::default())
    }

    pub fn with_tstat_config(db: Arc<dyn Database>, tstat_config: WalletTstatConfig) -> Self {
        Self {
            db,
            recent_signals: Arc::new(RwLock::new(HashMap::new())),
            wallet_clusters: Arc::new(RwLock::new(HashMap::new())),
            profitable_cache: Arc::new(RwLock::new(HashMap::new())),
            tstat_config,
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

        // Clean up old signals (older than the cluster window; per-reader
        // timestamps (5-min consensus, 1h divergence) filter their own
        // sub-windows, so retaining the longer horizon is safe)
        let cutoff = Instant::now() - Duration::from_secs(CLUSTER_WINDOW_SECS);
        signals.retain(|_, token_signals| {
            token_signals.retain(|s| s.timestamp > cutoff);
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
        self.peek_consensus_wallet_count(token_address).await >= 2
    }

    /// Number of statistically-profitable (t-stat) wallets with BUY signals on
    /// this token within the cluster window. Results are cached per wallet
    /// (1h TTL) so repeated calls don't hammer the DB.
    pub async fn peek_profitable_cluster_count(&self, token_address: &str) -> usize {
        let cutoff = Instant::now() - Duration::from_secs(CLUSTER_WINDOW_SECS);
        let signals = self.recent_signals.read().await;
        let Some(token_signals) = signals.get(token_address) else {
            return 0;
        };
        let mut seen = std::collections::HashSet::new();
        for s in token_signals {
            if s.direction == "BUY"
                && s.timestamp > cutoff
                && !seen.contains(&s.wallet_address)
                && self.wallet_is_profitable_cached(&s.wallet_address).await
            {
                seen.insert(s.wallet_address.clone());
            }
        }
        seen.len()
    }

    /// Cached t-stat profitability classification for a wallet.
    async fn wallet_is_profitable_cached(&self, wallet_address: &str) -> bool {
        let one_hour_ago = Instant::now() - Duration::from_secs(3600);
        {
            let cache = self.profitable_cache.read().await;
            if let Some((is_profitable, cached_at)) = cache.get(wallet_address) {
                if *cached_at > one_hour_ago {
                    return *is_profitable;
                }
            }
        }
        let cfg = self.tstat_config;
        let is_profitable = match self
            .db
            .get_wallet_pnl_statistics(wallet_address, cfg.window_days)
            .await
        {
            Ok(Some((sample_count, mean, stderr))) => {
                sample_count >= cfg.min_samples as i64
                    && stderr > rust_decimal::Decimal::ZERO
                    && (mean / stderr)
                        .to_f64()
                        .map(|t| t > cfg.threshold)
                        .unwrap_or(false)
            }
            _ => false,
        };
        let mut cache = self.profitable_cache.write().await;
        cache.insert(wallet_address.to_string(), (is_profitable, Instant::now()));
        is_profitable
    }

    /// Read-only consensus estimate: the number of distinct wallets with BUY
    /// signals for this token in the last 5 minutes, WITHOUT recording a new
    /// signal. Lets callers (e.g. the selection pipeline) score a candidate
    /// signal against the current window and record it only once the decision
    /// is admitted, so rejected noise never pollutes the consensus window.
    pub async fn peek_consensus_wallet_count(&self, token_address: &str) -> usize {
        let signals = self.recent_signals.read().await;
        let five_min_ago = Instant::now() - Duration::from_secs(300);
        if let Some(token_signals) = signals.get(token_address) {
            let mut seen = std::collections::HashSet::new();
            for s in token_signals {
                if s.direction == "BUY" && s.timestamp > five_min_ago {
                    seen.insert(&s.wallet_address);
                }
            }
            seen.len()
        } else {
            0
        }
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

    /// Get all recent signals for divergence analysis
    ///
    /// Returns a snapshot of all recent signals across all tokens.
    /// This is used for divergence detection in the consensus API.
    pub async fn get_all_recent_signals(&self) -> Vec<TokenSignal> {
        let signals = self.recent_signals.read().await;
        let mut all_signals = Vec::new();

        // Collect all signals from all tokens
        for token_signals in signals.values() {
            for signal in token_signals {
                all_signals.push(signal.clone());
            }
        }

        // Sort by timestamp (most recent first)
        all_signals.sort_by_key(|s| std::cmp::Reverse(s.timestamp));

        all_signals
    }
}
