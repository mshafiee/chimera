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
    transaction::{Transaction, VersionedTransaction},
};
use rust_decimal::prelude::*;
use std::str::FromStr;
use std::sync::Arc;

/// Transaction builder for swap operations
pub struct TransactionBuilder {
    /// RPC client
    rpc_client: Arc<RpcClient>,
    /// Configuration
    config: Arc<AppConfig>,
    /// HTTP client for Jupiter API calls
    http_client: reqwest::Client,
}

/// Built transaction ready for signing and submission
pub enum BuiltTransaction {
    /// Legacy transaction
    Legacy {
        transaction: Transaction,
        blockhash: solana_sdk::hash::Hash,
    },
    /// Versioned transaction (v0/v1) - stored as raw bytes for RPC submission
    Versioned {
        transaction_bytes: Vec<u8>,
        blockhash: solana_sdk::hash::Hash,
    },
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new(rpc_client: Arc<RpcClient>, config: Arc<AppConfig>) -> Self {
        // Create HTTP client with proper TLS and timeout configuration
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new()); // Fallback to default if builder fails
        
        Self {
            rpc_client,
            config,
            http_client,
        }
    }

    /// Build a swap transaction for a signal
    ///
    /// This uses Jupiter Swap API which returns a pre-built transaction
    /// that just needs to be signed.
    /// In devnet simulation mode, returns a simulated transaction without calling Jupiter.
    pub async fn build_swap_transaction(
        &self,
        signal: &Signal,
        wallet_keypair: &Keypair,
    ) -> AppResult<BuiltTransaction> {
        // Check if devnet simulation mode is enabled
        if self.config.jupiter.devnet_simulation_mode {
            tracing::info!(
                token = %signal.token_address(),
                action = ?signal.payload.action,
                amount_sol = signal.payload.amount_sol.to_f64().unwrap_or(0.0),
                "Devnet simulation mode: skipping Jupiter API, creating simulated transaction"
            );
            return self.build_simulated_transaction(signal, wallet_keypair).await;
        }

        // Determine input and output mints
        let (input_mint, output_mint, amount) = match signal.payload.action {
            Action::Buy => {
                // Buying token with SOL
                let sol_mint = Pubkey::from_str(crate::constants::mints::SOL)
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e)))?;
                let token_mint = Pubkey::from_str(signal.token_address())
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid token mint: {}", e)))?;
                
                // Convert SOL amount to lamports
                let amount_lamports = crate::utils::sol_to_lamports(signal.payload.amount_sol);
                (sol_mint, token_mint, amount_lamports)
            }
            Action::Sell => {
                // Selling token for SOL
                let token_mint = Pubkey::from_str(signal.token_address())
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid token mint: {}", e)))?;
                let sol_mint = Pubkey::from_str(crate::constants::mints::SOL)
                    .map_err(|e| crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e)))?;
                
                // For sell, we need to get token balance first
                // For now, use a placeholder - in production, fetch actual token balance
                let amount_lamports = crate::utils::sol_to_lamports(signal.payload.amount_sol);
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

        tracing::debug!(
            tx_bytes_len = tx_bytes.len(),
            first_byte = tx_bytes.get(0).copied(),
            "Decoded transaction from Jupiter"
        );

        // Get recent blockhash first (needed for both transaction types)
        let blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e)))?;

        // Jupiter v1 API returns VersionedTransaction (starts with version byte 0x01)
        // Check the first byte to determine transaction type
        if tx_bytes.len() > 0 && tx_bytes[0] == 0x01 {
            // VersionedTransaction (version 1) - may be V0 or Legacy
            // Parse to sign
            // Use bincode 1.3 (bincode1) to match Solana wire format
            let mut versioned_tx: VersionedTransaction = bincode1::deserialize(&tx_bytes)
                .map_err(|e| crate::error::AppError::Parse(format!("Failed to deserialize V0 tx: {}", e)))?;
            
            // Check if Jupiter ignored our asLegacyTransaction request
            let is_v0 = matches!(versioned_tx.message, solana_sdk::message::VersionedMessage::V0(_));
            if is_v0 {
                // Check if V0 transactions should be rejected
                if self.config.jupiter.reject_v0_transactions {
                    return Err(crate::error::AppError::Validation(
                        "V0 transactions are disabled by configuration (reject_v0_transactions=true)".to_string()
                    ));
                }
                
                if self.config.jupiter.reconstruct_v0_on_blockhash_expiry {
                    tracing::warn!(
                        "Jupiter returned V0 transaction despite asLegacyTransaction=true. \
                        Attempting to reconstruct with fresh blockhash to reduce expiration risk."
                    );
                    
                    // Attempt to reconstruct V0 message with fresh blockhash
                    use crate::engine::v0_reconstruction;
                    match v0_reconstruction::reconstruct_v0_message_with_blockhash(
                        &versioned_tx,
                        blockhash,
                        &self.rpc_client,
                    )
                    .await
                    {
                        Ok(reconstructed_message) => {
                            tracing::info!("Successfully reconstructed V0 message with fresh blockhash in transaction builder");
                            // Update the transaction with reconstructed message
                            versioned_tx.message = reconstructed_message;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to reconstruct V0 message in transaction builder. \
                                Using original message - transaction may fail if blockhash expires."
                            );
                            // Continue with original message - executor will handle reconstruction if needed
                        }
                    }
                } else {
                    tracing::warn!(
                        "Jupiter returned V0 transaction despite asLegacyTransaction=true. \
                        V0 reconstruction is disabled. Transaction may fail if blockhash expires."
                    );
                }
            }

            // Sign with our keypair
            let message_hash = versioned_tx.message.hash();
            let signature = wallet_keypair.try_sign_message(&message_hash.to_bytes())
                .map_err(|e| crate::error::AppError::Validation(format!("Signing failed: {}", e)))?;

            // Replace signature (Jupiter sends placeholder or empty)
            if versioned_tx.signatures.is_empty() {
                versioned_tx.signatures.push(signature);
            } else {
                versioned_tx.signatures[0] = signature;
            }

            // Re-serialize signed transaction
            // Use bincode 1.3 (bincode1) to ensure correct wire format for RPC
            let signed_bytes = bincode1::serialize(&versioned_tx)
                .map_err(|e| crate::error::AppError::Parse(format!("Failed to re-serialize V0 tx: {}", e)))?;

            Ok(BuiltTransaction::Versioned {
                transaction_bytes: signed_bytes, // Return signed bytes
                blockhash,
            })
        } else {
            // Legacy Transaction
            if tx_bytes.is_empty() {
                return Err(crate::error::AppError::Parse("Transaction bytes are empty".to_string()));
            }
            
            // Use bincode 1.3 (bincode1) for legacy transactions as well
            let mut tx: Transaction = bincode1::deserialize(&tx_bytes)
                .map_err(|e| crate::error::AppError::Parse(format!("Failed to deserialize legacy transaction: {}", e)))?;

            // Update blockhash and re-sign
            tx.message.recent_blockhash = blockhash;
            tx.sign(&[wallet_keypair], blockhash);
            
            Ok(BuiltTransaction::Legacy {
                transaction: tx,
                blockhash,
            })
        }
    }

    /// Build a simulated transaction for devnet testing
    /// This creates a minimal transaction that won't be submitted to RPC
    async fn build_simulated_transaction(
        &self,
        _signal: &Signal,
        wallet_keypair: &Keypair,
    ) -> AppResult<BuiltTransaction> {
        // Get recent blockhash (still needed for transaction structure)
        let blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e)))?;

        // Create a minimal empty transaction for simulation
        // This transaction will be marked as simulated and won't be submitted to RPC
        let empty_tx = Transaction::new_with_payer(&[], Some(&wallet_keypair.pubkey()));
        
        // Return as Legacy transaction with the blockhash
        // The executor will detect this is a simulated transaction and skip RPC submission
        Ok(BuiltTransaction::Legacy {
            transaction: empty_tx,
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
        // Use the configured Jupiter API URL (defaults to lite-api.jup.ag)
        // Note: Jupiter lite API may ignore asLegacyTransaction and still return V0 transactions
        // V0 transactions use Address Lookup Tables (ALTs) which may not exist on devnet
        // If ALT errors occur, consider using mainnet RPC or a different Jupiter endpoint
        let url = format!("{}/swap", self.config.jupiter.api_url);
        let payload = serde_json::json!({
            "quoteResponse": quote,  // Pass the full quote response
            "userPublicKey": user_public_key.to_string(),
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": "auto",
            "asLegacyTransaction": true  // Request legacy format (may be ignored by lite API)
        });

        let response = self.http_client
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
        // Use the configured Jupiter API URL (defaults to lite-api.jup.ag)
        // Old quote-api.jup.ag/v6 is deprecated
        let url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps=50",
            self.config.jupiter.api_url, input_mint, output_mint, amount
        );

        tracing::debug!(url = %url, "Requesting Jupiter quote");
        let response = self.http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, url = %url, "Jupiter quote request failed");
                crate::error::AppError::Http(format!("Jupiter quote request failed: {} (URL: {})", e, url))
            })?;

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

/// Jupiter quote response (v1 API format)
/// We use serde_json::Value to handle the full response flexibly
pub type JupiterQuote = serde_json::Value;

/// Jupiter swap response
#[derive(Debug, serde::Deserialize)]
pub struct JupiterSwapResponse {
    /// Swap transaction (base64 encoded)
    #[serde(rename = "swapTransaction")]
    pub swap_transaction: String,
}

/// Load wallet keypair from vault
pub fn load_wallet_keypair(secrets: &VaultSecrets) -> AppResult<Keypair> {
    use secrecy::ExposeSecret; 

    let key_secret = secrets
        .wallet_private_key
        .as_ref()
        .ok_or_else(|| crate::error::AppError::Validation("Wallet private key not found in vault".to_string()))?;

    // Expose secret safely only for this operation
    let key_hex = key_secret.expose_secret();
    
    // Decode hex string to bytes
    let key_bytes = hex::decode(key_hex.trim())
        .map_err(|e| crate::error::AppError::Validation(format!("Invalid private key hex: {}", e)))?;

    if key_bytes.len() != 64 {
        return Err(crate::error::AppError::Validation(
            format!("Invalid keypair length (expected 64 bytes, got {})", key_bytes.len())
        ));
    }

    // Solana keypair format in vault: 64 bytes = 32 secret + 32 public
    // Solana SDK's Keypair::try_from expects 64 bytes (full keypair array)
    let keypair_bytes: [u8; 64] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| crate::error::AppError::Validation("Invalid keypair length".to_string()))?;
    
    // Use try_from with the full 64-byte array
    let keypair = Keypair::try_from(keypair_bytes.as_slice())
        .map_err(|e| {
            crate::error::AppError::Validation(format!(
                "Failed to create keypair from 64-byte array: {:?}. \
                Ensure the keypair bytes are in the correct format (32 secret + 32 public).",
                e
            ))
        })?;

    // The secrets struct will zeroize itself when dropped (out of scope), 
    // but the Keypair (Solana SDK) persists. This is unavoidable as we need it 
    // for signing, but at least the source buffer is cleaned.

    Ok(keypair)
}
