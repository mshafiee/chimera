//! Transaction builder for Solana swaps
//!
//! Builds swap transactions using Jupiter Aggregator API
//! Supports both Jito bundles and standard TPU submission

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::models::{Action, Signal};
use crate::vault::VaultSecrets;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use std::str::FromStr;
use std::sync::Arc;

/// Transaction builder for swap operations
pub struct TransactionBuilder {
    /// RPC client
    rpc_client: Arc<RpcClient>,
    /// Configuration
    config: Arc<AppConfig>,
}

/// Built transaction ready for signing and submission
pub struct BuiltTransaction {
    /// The transaction
    pub transaction: Transaction,
    /// Recent blockhash used
    pub blockhash: solana_sdk::hash::Hash,
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new(rpc_client: Arc<RpcClient>, config: Arc<AppConfig>) -> Self {
        Self { rpc_client, config }
    }

    /// Build a swap transaction for a signal
    ///
    /// This uses Jupiter Swap API which returns a pre-built transaction
    /// that just needs to be signed.
    pub async fn build_swap_transaction(
        &self,
        signal: &Signal,
        wallet_keypair: &Keypair,
    ) -> AppResult<BuiltTransaction> {
        // Determine input and output mints
        let (input_mint, output_mint, amount) = match signal.payload.action {
            Action::Buy => {
                // Buying token with SOL
                let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e)))?;
                let token_mint = Pubkey::from_str(signal.token_address())
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid token mint: {}", e)))?;
                
                // Convert SOL amount to lamports
                let amount_lamports = (signal.payload.amount_sol * 1_000_000_000.0) as u64;
                (sol_mint, token_mint, amount_lamports)
            }
            Action::Sell => {
                // Selling token for SOL
                let token_mint = Pubkey::from_str(signal.token_address())
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid token mint: {}", e)))?;
                let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e)))?;
                
                // For sell, we need to get token balance first
                // For now, use a placeholder - in production, fetch actual token balance
                let amount_lamports = (signal.payload.amount_sol * 1_000_000_000.0) as u64;
                (token_mint, sol_mint, amount_lamports)
            }
        };

        // Get swap transaction from Jupiter Swap API
        let swap_response = self
            .get_jupiter_swap(input_mint, output_mint, amount, wallet_keypair.pubkey())
            .await?;

        // Decode the base64 transaction
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        let tx_bytes = BASE64
            .decode(&swap_response.swap_transaction)
            .map_err(|e| crate::error::AppError::Parse(format!("Failed to decode transaction: {}", e)))?;

        // Deserialize transaction
        let mut transaction: Transaction = bincode::serde::decode_from_slice(&tx_bytes, bincode::config::standard())
            .map_err(|e| crate::error::AppError::Parse(format!("Failed to deserialize transaction: {}", e)))?
            .0;

        // Get recent blockhash (Jupiter transaction may have stale blockhash)
        let blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e)))?;

        // Update blockhash and re-sign
        transaction.message.recent_blockhash = blockhash;
        transaction.sign(&[wallet_keypair], blockhash);

        Ok(BuiltTransaction {
            transaction,
            blockhash,
        })
    }


    /// Get swap transaction from Jupiter Swap API
    async fn get_jupiter_swap(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
        user_public_key: Pubkey,
    ) -> AppResult<JupiterSwapResponse> {
        // First get a quote
        let quote = self.get_jupiter_quote(input_mint, output_mint, amount).await?;

        // Then get the swap transaction
        let url = "https://quote-api.jup.ag/v6/swap";
        let payload = serde_json::json!({
            "quoteResponse": quote,
            "userPublicKey": user_public_key.to_string(),
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": "auto"
        });

        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| crate::error::AppError::Http(format!("Jupiter swap request failed: {}", e)))?;

        let swap_response: JupiterSwapResponse = response
            .json()
            .await
            .map_err(|e| crate::error::AppError::Parse(format!("Failed to parse Jupiter swap: {}", e)))?;

        Ok(swap_response)
    }

    /// Get quote from Jupiter Quote API
    async fn get_jupiter_quote(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
    ) -> AppResult<JupiterQuote> {
        let url = format!(
            "https://quote-api.jup.ag/v6/quote?inputMint={}&outputMint={}&amount={}&slippageBps=50",
            input_mint, output_mint, amount
        );

        let response = reqwest::get(&url)
            .await
            .map_err(|e| crate::error::AppError::Http(format!("Jupiter quote request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(crate::error::AppError::Http(format!(
                "Jupiter quote API returned error: {}",
                response.status()
            )));
        }

        let quote: JupiterQuote = response
            .json()
            .await
            .map_err(|e| crate::error::AppError::Parse(format!("Failed to parse Jupiter quote: {}", e)))?;

        Ok(quote)
    }
}

/// Jupiter quote response
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct JupiterQuote {
    /// Input amount
    #[serde(rename = "inAmount")]
    pub in_amount: String,
    /// Output amount
    #[serde(rename = "outAmount")]
    pub out_amount: String,
    /// Price impact
    #[serde(rename = "priceImpactPct")]
    pub price_impact_pct: Option<f64>,
    /// Other quote fields
    #[serde(flatten)]
    pub other: serde_json::Value,
}

/// Jupiter swap response
#[derive(Debug, serde::Deserialize)]
pub struct JupiterSwapResponse {
    /// Swap transaction (base64 encoded)
    #[serde(rename = "swapTransaction")]
    pub swap_transaction: String,
}

/// Load wallet keypair from vault
pub fn load_wallet_keypair(secrets: &VaultSecrets) -> AppResult<Keypair> {
    let key_bytes = secrets
        .wallet_private_key
        .as_ref()
        .ok_or_else(|| crate::error::AppError::Validation("Wallet private key not found in vault".to_string()))?;

    if key_bytes.len() != 64 {
        return Err(crate::error::AppError::Validation(
            "Invalid keypair length (expected 64 bytes)".to_string(),
        ));
    }

    // First 32 bytes are the secret key, last 32 bytes are the public key
    let secret_key: [u8; 32] = key_bytes[0..32]
        .try_into()
        .map_err(|_| crate::error::AppError::Validation("Invalid secret key format".to_string()))?;

    let keypair = Keypair::try_from(&secret_key[..])
        .map_err(|e| crate::error::AppError::Validation(format!("Failed to create keypair: {}", e)))?;

    Ok(keypair)
}
