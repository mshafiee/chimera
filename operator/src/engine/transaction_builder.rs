//! Transaction builder for Solana swaps
//!
//! Builds swap transactions using Jupiter Aggregator API
//! Supports both Jito bundles and standard TPU submission

use crate::config::AppConfig;
use crate::engine::dex_comparator::DexComparator;
use crate::error::AppResult;
use crate::models::{Action, Signal};
use crate::vault::VaultSecrets;
use rust_decimal::prelude::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::{Transaction, VersionedTransaction},
};
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
    /// DEX comparator for dynamic slippage estimation
    dex_comparator: DexComparator,
}

/// Built transaction ready for signing and submission
pub enum BuiltTransaction {
    /// Legacy transaction
    Legacy {
        transaction: Transaction,
        blockhash: solana_sdk::hash::Hash,
        /// Actual price impact from Jupiter quote (e.g. 1.5 = 1.5%).  `None` if unavailable.
        price_impact_pct: Option<Decimal>,
        /// Fill price from Jupiter quote: inAmount_lamports / outAmount_base_units.
        fill_price_lamports_per_base: Option<Decimal>,
    },
    /// Versioned transaction (v0/v1) - stored as raw bytes for RPC submission
    Versioned {
        transaction_bytes: Vec<u8>,
        blockhash: solana_sdk::hash::Hash,
        /// Actual price impact from Jupiter quote (e.g. 1.5 = 1.5%).  `None` if unavailable.
        price_impact_pct: Option<Decimal>,
        /// Fill price from Jupiter quote: inAmount_lamports / outAmount_base_units.
        fill_price_lamports_per_base: Option<Decimal>,
    },
}

impl BuiltTransaction {
    pub fn price_impact_pct(&self) -> Option<Decimal> {
        match self {
            BuiltTransaction::Legacy { price_impact_pct, .. } => *price_impact_pct,
            BuiltTransaction::Versioned { price_impact_pct, .. } => *price_impact_pct,
        }
    }

    pub fn fill_price_lamports_per_base(&self) -> Option<Decimal> {
        match self {
            BuiltTransaction::Legacy { fill_price_lamports_per_base, .. } => *fill_price_lamports_per_base,
            BuiltTransaction::Versioned { fill_price_lamports_per_base, .. } => *fill_price_lamports_per_base,
        }
    }
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

        let jupiter_url = config.jupiter.api_url.clone();
        Self {
            rpc_client,
            config,
            http_client,
            dex_comparator: DexComparator::with_jupiter_api_url(jupiter_url),
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
            return self
                .build_simulated_transaction(signal, wallet_keypair)
                .await;
        }

        // Determine input and output mints
        let (input_mint, output_mint, amount) = match signal.payload.action {
            Action::Buy => {
                // Buying token with SOL
                let sol_mint = Pubkey::from_str(crate::constants::mints::SOL).map_err(|e| {
                    crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e))
                })?;
                let token_mint = Pubkey::from_str(signal.token_address()).map_err(|e| {
                    crate::error::AppError::Validation(format!("Invalid token mint: {}", e))
                })?;

                // Convert SOL amount to lamports
                let amount_lamports = crate::utils::sol_to_lamports(signal.payload.amount_sol);
                (sol_mint, token_mint, amount_lamports)
            }
            Action::Sell => {
                // Selling token for SOL
                let token_mint = Pubkey::from_str(signal.token_address()).map_err(|e| {
                    crate::error::AppError::Validation(format!("Invalid token mint: {}", e))
                })?;
                let sol_mint = Pubkey::from_str(crate::constants::mints::SOL).map_err(|e| {
                    crate::error::AppError::Validation(format!("Invalid SOL mint: {}", e))
                })?;

                // Try to fetch the actual on-chain token balance; fall back to price estimate.
                let exit_fraction = signal.payload.exit_fraction.unwrap_or(Decimal::ONE);
                let amount_lamports = match self
                    .fetch_token_balance(&wallet_keypair.pubkey(), &token_mint)
                    .await
                {
                    Some(bal) if bal > 0 => {
                        let scaled_bal = (Decimal::from(bal) * exit_fraction)
                            .round()
                            .to_u64()
                            .unwrap_or(bal);
                        tracing::info!(
                            wallet = %wallet_keypair.pubkey(),
                            token = %signal.payload.token,
                            balance_lamports = bal,
                            exit_fraction = %exit_fraction,
                            scaled_balance_lamports = scaled_bal,
                            "SELL: using on-chain token balance scaled by exit_fraction"
                        );
                        scaled_bal
                    }
                    _ => {
                        tracing::error!(
                            wallet = %wallet_keypair.pubkey(),
                            token = %signal.payload.token,
                            sol_value = %signal.payload.amount_sol,
                            "SELL: on-chain token balance unavailable and price estimate fallback is disabled — refusing to guess sell amount"
                        );
                        return Err(crate::error::AppError::Rpc(
                            "Cannot determine token balance for SELL: on-chain fetch failed and no reliable price source available".to_string(),
                        ));
                    }
                };

                (token_mint, sol_mint, amount_lamports)
            }
        };

        // Get swap transaction from Jupiter Swap API
        let swap_response = self
            .get_jupiter_swap(input_mint, output_mint, amount, wallet_keypair.pubkey())
            .await?;

        // Extract quote-derived fields before consuming swap_response
        let price_impact_pct = swap_response.price_impact_pct;
        let fill_price_lamports_per_base = swap_response.fill_price_lamports_per_base;

        // Decode the base64 transaction
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        let tx_bytes = BASE64
            .decode(&swap_response.swap_transaction)
            .map_err(|e| {
                crate::error::AppError::Parse(format!("Failed to decode transaction: {}", e))
            })?;

        tracing::debug!(
            tx_bytes_len = tx_bytes.len(),
            first_byte = tx_bytes.first().copied(),
            "Decoded transaction from Jupiter"
        );

        // Get recent blockhash first (needed for both transaction types)
        let blockhash =
            self.rpc_client.get_latest_blockhash().await.map_err(|e| {
                crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e))
            })?;

        // Jupiter v1 API returns VersionedTransaction (starts with version byte 0x01)
        // Check the first byte to determine transaction type
        if !tx_bytes.is_empty() && tx_bytes[0] == 0x01 {
            // VersionedTransaction (version 1) - may be V0 or Legacy
            // Parse to sign
            // Use bincode 1.3 (bincode1) to match Solana wire format
            let mut versioned_tx: VersionedTransaction =
                bincode1::deserialize(&tx_bytes).map_err(|e| {
                    crate::error::AppError::Parse(format!("Failed to deserialize V0 tx: {}", e))
                })?;

            // Check if Jupiter ignored our asLegacyTransaction request
            let is_v0 = matches!(
                versioned_tx.message,
                solana_sdk::message::VersionedMessage::V0(_)
            );
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
            let signature = wallet_keypair
                .try_sign_message(&message_hash.to_bytes())
                .map_err(|e| {
                    crate::error::AppError::Validation(format!("Signing failed: {}", e))
                })?;

            // Replace signature (Jupiter sends placeholder or empty)
            if versioned_tx.signatures.is_empty() {
                versioned_tx.signatures.push(signature);
            } else {
                versioned_tx.signatures[0] = signature;
            }

            // Re-serialize signed transaction
            // Use bincode 1.3 (bincode1) to ensure correct wire format for RPC
            let signed_bytes = bincode1::serialize(&versioned_tx).map_err(|e| {
                crate::error::AppError::Parse(format!("Failed to re-serialize V0 tx: {}", e))
            })?;

            Ok(BuiltTransaction::Versioned {
                transaction_bytes: signed_bytes,
                blockhash,
                price_impact_pct,
                fill_price_lamports_per_base,
            })
        } else {
            // Legacy Transaction
            if tx_bytes.is_empty() {
                return Err(crate::error::AppError::Parse(
                    "Transaction bytes are empty".to_string(),
                ));
            }

            // Use bincode 1.3 (bincode1) for legacy transactions as well
            let mut tx: Transaction = bincode1::deserialize(&tx_bytes).map_err(|e| {
                crate::error::AppError::Parse(format!(
                    "Failed to deserialize legacy transaction: {}",
                    e
                ))
            })?;

            // Update blockhash and re-sign
            tx.message.recent_blockhash = blockhash;
            tx.sign(&[wallet_keypair], blockhash);

            Ok(BuiltTransaction::Legacy {
                transaction: tx,
                blockhash,
                price_impact_pct,
                fill_price_lamports_per_base,
            })
        }
    }

    /// Build a simulated transaction for devnet testing
    async fn build_simulated_transaction(
        &self,
        _signal: &Signal,
        wallet_keypair: &Keypair,
    ) -> AppResult<BuiltTransaction> {
        let blockhash =
            self.rpc_client.get_latest_blockhash().await.map_err(|e| {
                crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e))
            })?;

        let empty_tx = Transaction::new_with_payer(&[], Some(&wallet_keypair.pubkey()));

        Ok(BuiltTransaction::Legacy {
            transaction: empty_tx,
            blockhash,
            price_impact_pct: None,
            fill_price_lamports_per_base: None,
        })
    }

    /// Fetch the actual SPL token balance for `wallet_pubkey` / `token_mint` via RPC.
    ///
    /// Returns the largest balance found across all token accounts for that
    /// (owner, mint) pair, or `None` if the RPC call fails or no accounts exist.
    async fn fetch_token_balance(
        &self,
        wallet_pubkey: &Pubkey,
        token_mint: &Pubkey,
    ) -> Option<u64> {
        use solana_account_decoder::UiAccountData;
        use solana_client::rpc_request::TokenAccountsFilter;

        let accounts = self
            .rpc_client
            .get_token_accounts_by_owner(wallet_pubkey, TokenAccountsFilter::Mint(*token_mint))
            .await
            .ok()?;

        accounts
            .iter()
            .filter_map(|keyed| {
                if let UiAccountData::Json(parsed) = &keyed.account.data {
                    parsed
                        .parsed
                        .get("info")
                        .and_then(|i| i.get("tokenAmount"))
                        .and_then(|ta| ta.get("amount"))
                        .and_then(|a| a.as_str())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                }
            })
            .max()
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
        let quote = self
            .get_jupiter_quote(input_mint, output_mint, amount)
            .await?;

        // Extract priceImpactPct from the quote for cost tracking in the executor
        // Jupiter returns this as a percentage string, e.g. "1.234" = 1.234%
        let price_impact_pct: Option<Decimal> = quote
            .get("priceImpactPct")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok());

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

        let response = self
            .http_client
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                crate::error::AppError::Http(format!("Jupiter swap request failed: {}", e))
            })?;

        let mut swap_response: JupiterSwapResponse = response.json().await.map_err(|e| {
            crate::error::AppError::Parse(format!("Failed to parse Jupiter swap: {}", e))
        })?;

        swap_response.price_impact_pct = price_impact_pct;

        // Compute fill price from quote amounts: lamports_in / base_units_out
        // For BUY (SOL→TOKEN): in_amount = lamports spent, out_amount = token base units received
        swap_response.fill_price_lamports_per_base = {
            let in_amount = quote
                .get("inAmount")
                .and_then(|v| v.as_str().and_then(|s: &str| s.parse::<u64>().ok()))
                .or_else(|| quote.get("inAmount").and_then(|v| v.as_u64()));
            let out_amount = quote
                .get("outAmount")
                .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                .or_else(|| quote.get("outAmount").and_then(|v| v.as_u64()));
            match (in_amount, out_amount) {
                (Some(inn), Some(out)) if out > 0 && inn > 0 => {
                    let in_dec = Decimal::from(inn);
                    let out_dec = Decimal::from(out);
                    if output_mint.to_string() == crate::constants::mints::SOL {
                        // For SELL (TOKEN→SOL): out_amount is SOL lamports, in_amount is token base units
                        Some(out_dec / in_dec)
                    } else {
                        // For BUY (SOL→TOKEN): in_amount is SOL lamports, out_amount is token base units
                        Some(in_dec / out_dec)
                    }
                }
                _ => None,
            }
        };

        Ok(swap_response)
    }

    /// Get quote from Jupiter Quote API
    async fn get_jupiter_quote(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
    ) -> AppResult<JupiterQuote> {
        // Use DEX comparator to estimate true slippage and set a tighter tolerance.
        // Clamp to [30, 150] bps: 30 = tight floor, 150 = 1.5% absolute ceiling.
        let amount_sol = Decimal::from(amount) / Decimal::from(1_000_000_000u64);
        let slippage_bps: u64 = match self
            .dex_comparator
            .compare_and_select(
                &input_mint.to_string(),
                &output_mint.to_string(),
                amount_sol,
            )
            .await
        {
            Ok(result) if !amount_sol.is_zero() => {
                let bps = ((result.slippage_sol / amount_sol) * Decimal::from(10_000u64))
                    .to_u64()
                    .unwrap_or(50)
                    .clamp(30, 150);
                if result.selected_dex != "Jupiter" {
                    tracing::debug!(
                        selected_dex = %result.selected_dex,
                        slippage_bps = bps,
                        "DexComparator found cheaper route; slippage adjusted"
                    );
                }
                bps
            }
            _ => 50, // fallback to original hardcoded value on error
        };

        // Use the configured Jupiter API URL (defaults to lite-api.jup.ag)
        // Old quote-api.jup.ag/v6 is deprecated
        let url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            self.config.jupiter.api_url, input_mint, output_mint, amount, slippage_bps
        );

        tracing::debug!(url = %url, "Requesting Jupiter quote");
        let response = self.http_client.get(&url).send().await.map_err(|e| {
            tracing::error!(error = %e, url = %url, "Jupiter quote request failed");
            crate::error::AppError::Http(format!(
                "Jupiter quote request failed: {} (URL: {})",
                e, url
            ))
        })?;

        if !response.status().is_success() {
            return Err(crate::error::AppError::Http(format!(
                "Jupiter quote API returned error: {}",
                response.status()
            )));
        }

        let quote: JupiterQuote = response.json().await.map_err(|e| {
            crate::error::AppError::Parse(format!("Failed to parse Jupiter quote: {}", e))
        })?;

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
    /// Actual price impact from the Jupiter quote, as a percentage (e.g. 1.5 = 1.5%).
    /// Populated from the quote's `priceImpactPct` field; `None` if unavailable.
    #[serde(skip)]
    pub price_impact_pct: Option<Decimal>,
    /// Fill price computed from the Jupiter quote: inAmount_lamports / outAmount_base_units.
    /// For BUY (SOL→TOKEN) this is lamports-per-token-base-unit; divide by 1e9 to get SOL/token.
    /// `None` if inAmount/outAmount are unavailable or unparseable.
    #[serde(skip)]
    pub fill_price_lamports_per_base: Option<Decimal>,
}

/// Load wallet keypair from vault
pub fn load_wallet_keypair(secrets: &VaultSecrets) -> AppResult<Keypair> {
    use secrecy::ExposeSecret;

    let key_secret = secrets.wallet_private_key.as_ref().ok_or_else(|| {
        crate::error::AppError::Validation("Wallet private key not found in vault".to_string())
    })?;

    // Expose secret safely only for this operation
    let key_hex = key_secret.expose_secret();

    // Decode hex string to bytes
    let key_bytes = hex::decode(key_hex.trim()).map_err(|e| {
        crate::error::AppError::Validation(format!("Invalid private key hex: {}", e))
    })?;

    if key_bytes.len() != 64 {
        return Err(crate::error::AppError::Validation(format!(
            "Invalid keypair length (expected 64 bytes, got {})",
            key_bytes.len()
        )));
    }

    // Solana keypair format in vault: 64 bytes = 32 secret + 32 public
    // Solana SDK's Keypair::try_from expects 64 bytes (full keypair array)
    let keypair_bytes: [u8; 64] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| crate::error::AppError::Validation("Invalid keypair length".to_string()))?;

    // Use try_from with the full 64-byte array
    let keypair = Keypair::try_from(keypair_bytes.as_slice()).map_err(|e| {
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
