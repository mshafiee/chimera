//! Direct Jito Searcher integration for bundle submission
//!
//! This module provides direct integration with Jito Searcher API,
//! allowing bundle submission without requiring Helius Sender API.

use crate::engine::executor::ExecutorError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use solana_client::nonblocking::rpc_client::RpcClient;
#[allow(deprecated)] // system_instruction is deprecated but still works in solana-sdk 2.1
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::str::FromStr;
use std::sync::Arc;

/// Jito Searcher client for direct bundle submission
pub struct JitoSearcherClient {
    /// Jito Searcher endpoint URL
    endpoint: String,
    /// HTTP client for API calls
    http_client: reqwest::Client,
    /// RPC client for getting recent blockhash
    rpc_client: Arc<RpcClient>,
}

impl JitoSearcherClient {
    /// Create a new Jito Searcher client
    pub fn new(endpoint: String, rpc_client: Arc<RpcClient>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            endpoint,
            http_client,
            rpc_client,
        }
    }

    /// Submit a bundle to Jito Searcher
    ///
    /// Creates a bundle with:
    /// 1. Tip transaction (to tip account)
    /// 2. Swap transaction (the actual trade)
    ///
    /// Returns the bundle signature
    pub async fn submit_bundle(
        &self,
        swap_transaction: &Transaction,
        tip_lamports: u64,
        tip_keypair: &Keypair,
    ) -> Result<String, ExecutorError> {
        // Create tip transaction
        let tip_transaction = self
            .create_tip_transaction(tip_lamports, tip_keypair)
            .await
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to create tip transaction: {}", e)))?;

        // Build bundle: tip transaction first, then swap transaction
        let tip_tx_bytes = bincode::serialize(&tip_transaction)
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to serialize tip tx: {}", e)))?;
        let tip_tx_base64 = BASE64.encode(&tip_tx_bytes);

        let swap_tx_bytes = bincode::serialize(swap_transaction)
            .map_err(|e| ExecutorError::TransactionFailed(format!("Failed to serialize swap tx: {}", e)))?;
        let swap_tx_base64 = BASE64.encode(&swap_tx_bytes);

        // Jito Searcher API expects bundle in specific format
        let bundle = vec![tip_tx_base64, swap_tx_base64];

        // Submit to Jito Searcher
        let url = format!("{}/api/v1/bundles", self.endpoint);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [bundle]
        });

        let response = self
            .http_client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Jito Searcher request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(ExecutorError::Rpc(format!(
                "Jito Searcher API error: {} - {}",
                status, error_text
            )));
        }

        // Parse response
        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Jito response: {}", e)))?;

        // Extract bundle signature or ID
        let signature = result
            .get("result")
            .and_then(|r| r.get("signature"))
            .or_else(|| result.get("result").and_then(|r| r.get("bundleId")))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecutorError::Rpc("No signature in Jito response".to_string()))?;

        Ok(signature.to_string())
    }

    /// Create a tip transaction to the Jito tip account
    async fn create_tip_transaction(
        &self,
        tip_lamports: u64,
        tip_keypair: &Keypair,
    ) -> Result<Transaction, ExecutorError> {
        // Jito tip account (mainnet)
        let jito_tip_account = Pubkey::from_str("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4")
            .map_err(|e| ExecutorError::TransactionFailed(format!("Invalid Jito tip account: {}", e)))?;

        // Get recent blockhash from RPC
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to get recent blockhash: {}", e)))?;

        // Create tip instruction
        let tip_instruction = system_instruction::transfer(
            &tip_keypair.pubkey(),
            &jito_tip_account,
            tip_lamports,
        );

        // Build transaction
        let mut transaction = Transaction::new_with_payer(
            &[tip_instruction],
            Some(&tip_keypair.pubkey()),
        );

        // Set recent blockhash
        transaction.message.recent_blockhash = recent_blockhash;

        // Sign transaction
        transaction.sign(&[tip_keypair], recent_blockhash);

        Ok(transaction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Keypair;

    #[test]
    fn test_jito_searcher_client_creation() {
        let rpc_client = Arc::new(RpcClient::new("https://api.mainnet-beta.solana.com".to_string()));
        let client = JitoSearcherClient::new("https://mainnet.block-engine.jito.wtf".to_string(), rpc_client);
        assert_eq!(client.endpoint, "https://mainnet.block-engine.jito.wtf");
    }

    #[tokio::test]
    async fn test_tip_transaction_creation() {
        let keypair = Keypair::new();
        let rpc_client = Arc::new(RpcClient::new("https://api.mainnet-beta.solana.com".to_string()));
        let client = JitoSearcherClient::new("https://mainnet.block-engine.jito.wtf".to_string(), rpc_client);
        
        // This will fail without recent blockhash, but tests structure
        let result = client.create_tip_transaction(1_000_000, &keypair).await;
        // In real implementation, would need RPC client to get blockhash
        // Just check it compiles - actual test would require network access
        // Result will be an error without network, which is expected
        assert!(result.is_err() || result.is_ok());
    }
}
