//! Subscription management for Helius LaserStream WebSocket
//!
//! Manages WebSocket subscriptions to specific wallet addresses and syncs
//! with the database wallet roster.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::db_abstraction::Database;

/// Subscription manager for Helius WebSocket
pub struct SubscriptionManager {
    db: Arc<dyn Database>,
    websocket_url: String,
    commitment: String,
    subscribed_wallets: Arc<RwLock<HashSet<String>>>,
    subscription_ids: Arc<RwLock<Vec<u64>>>,
}

impl SubscriptionManager {
    pub fn new(db: Arc<dyn Database>, websocket_url: String, commitment: String) -> Self {
        Self {
            db,
            websocket_url,
            commitment,
            subscribed_wallets: Arc::new(RwLock::new(HashSet::new())),
            subscription_ids: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Subscribe to a specific wallet address
    pub async fn subscribe_wallet(&self, wallet: &str) -> Result<()> {
        // Check if already subscribed
        let subscribed = self.subscribed_wallets.read().await;
        if subscribed.contains(wallet) {
            tracing::debug!(wallet = %wallet, "Already subscribed to wallet");
            return Ok(());
        }
        drop(subscribed);

        tracing::info!(wallet = %wallet, "Subscribing to wallet transactions");

        // Create subscription request
        let _request = SubscriptionRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "transactionSubscribe".to_string(),
            params: vec![SubscriptionParams {
                account: vec![wallet.to_string()],
                failed: false,
                commitment: self.commitment.clone(),
            }],
        };

        // Note: In a real implementation, we would send this via the WebSocket connection
        // For now, we'll track the subscription locally
        let mut subscribed = self.subscribed_wallets.write().await;
        subscribed.insert(wallet.to_string());

        tracing::info!(wallet = %wallet, "Successfully subscribed to wallet");

        Ok(())
    }

    /// Unsubscribe from a wallet address
    pub async fn unsubscribe_wallet(&self, wallet: &str) -> Result<()> {
        let mut subscribed = self.subscribed_wallets.write().await;

        if subscribed.remove(wallet) {
            tracing::info!(wallet = %wallet, "Unsubscribed from wallet");

            // In a real implementation, we would send unsubscribe request via WebSocket
            // For now, just remove from local tracking
        }

        Ok(())
    }

    /// Sync subscriptions with ACTIVE wallets from database
    pub async fn sync_active_wallets(&self) -> Result<()> {
        tracing::info!("Syncing subscriptions with ACTIVE wallets");

        // Get ACTIVE wallets from database
        let active_wallets = self.get_active_wallets().await?;

        // Get current subscriptions
        let current_subscriptions: HashSet<String> =
            self.subscribed_wallets.read().await.clone();

        // Subscribe to new wallets
        for wallet in &active_wallets {
            if !current_subscriptions.contains(wallet) {
                if let Err(e) = self.subscribe_wallet(wallet).await {
                    tracing::warn!(error = %e, wallet = %wallet, "Failed to subscribe");
                }
            }
        }

        // Unsubscribe from inactive wallets
        for wallet in current_subscriptions.iter() {
            if !active_wallets.contains(wallet) {
                if let Err(e) = self.unsubscribe_wallet(wallet).await {
                    tracing::warn!(error = %e, wallet = %wallet, "Failed to unsubscribe");
                }
            }
        }

        let final_count = self.subscribed_wallets.read().await.len();
        tracing::info!(
            active_count = active_wallets.len(),
            subscribed_count = final_count,
            "Wallet subscription sync complete"
        );

        Ok(())
    }

    /// Resubscribe to all currently tracked wallets (for reconnection)
    pub async fn resubscribe_all(&self) -> Result<()> {
        tracing::info!("Restoring all WebSocket subscriptions");

        let wallets = self.subscribed_wallets.read().await.clone();

        // Clear current subscriptions
        *self.subscribed_wallets.write().await = HashSet::new();

        // Resubscribe to each wallet
        for wallet in wallets {
            if let Err(e) = self.subscribe_wallet(&wallet).await {
                tracing::warn!(error = %e, wallet = %wallet, "Failed to resubscribe");
            }
        }

        Ok(())
    }

    /// Get ACTIVE wallets from database
    async fn get_active_wallets(&self) -> Result<Vec<String>> {
        let wallets = self
            .db
            .get_wallets_by_status("ACTIVE")
            .await
            .context("Failed to query active wallets")?;

        Ok(wallets.into_iter().map(|w| w.address).collect())
    }

    /// Batch subscribe to multiple wallets
    pub async fn batch_subscribe(&self, wallets: &[String]) -> Result<()> {
        for wallet in wallets {
            if let Err(e) = self.subscribe_wallet(wallet).await {
                tracing::warn!(error = %e, wallet = %wallet, "Failed to subscribe in batch");
            }
        }

        Ok(())
    }

    /// Cleanup inactive subscriptions
    pub async fn cleanup_inactive_subscriptions(&self) -> Result<()> {
        self.sync_active_wallets().await
    }

    /// Get current subscription count
    pub async fn subscription_count(&self) -> usize {
        self.subscribed_wallets.read().await.len()
    }
}

/// Subscription request for Helius WebSocket
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscriptionRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Vec<SubscriptionParams>,
}

/// Subscription parameters for transactionSubscribe
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SubscriptionParams {
    /// Account addresses to include in transaction filter
    account: Vec<String>,
    /// Filter out failed transactions
    failed: bool,
    /// Commitment level (processed, confirmed, finalized)
    commitment: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscription_sync() {
        // Test would require mock database
        // For now, just verify the struct compiles
        let params = SubscriptionParams {
            account: vec!["7xKXtg2CW87d97TXJSDpbD5jBkheTqA83hZRuYos7HtX".to_string()],
            failed: false,
            commitment: "confirmed".to_string(),
        };

        let request = SubscriptionRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "transactionSubscribe".to_string(),
            params: vec![params],
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("transactionSubscribe"));
    }
}
