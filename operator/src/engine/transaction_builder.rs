//! Transaction builder for Solana swaps
//!
//! Builds swap transactions using Jupiter Aggregator API
//! Supports both Jito bundles and standard TPU submission

use crate::config::AppConfig;
use crate::engine::dex_comparator::DexComparator;
use crate::error::AppResult;
use crate::jupiter_error_handling::{execute_with_jupiter_error_handling, RetryConfig};
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
    /// Optional circuit breaker for Jupiter failure tracking
    circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
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
        /// Real per-route DEX fee in SOL (summed `routePlan[].swapInfo.feeAmount`).
        /// P2-17: replaces the flat `dex_fee_rate` estimate in cost tracking.
        route_fee_sol: Option<Decimal>,
    },
    /// Versioned transaction (v0/v1) - stored as raw bytes for RPC submission
    Versioned {
        transaction_bytes: Vec<u8>,
        blockhash: solana_sdk::hash::Hash,
        /// Actual price impact from Jupiter quote (e.g. 1.5 = 1.5%).  `None` if unavailable.
        price_impact_pct: Option<Decimal>,
        /// Fill price from Jupiter quote: inAmount_lamports / outAmount_base_units.
        fill_price_lamports_per_base: Option<Decimal>,
        /// Real per-route DEX fee in SOL. See [`BuiltTransaction::Legacy::route_fee_sol`].
        route_fee_sol: Option<Decimal>,
    },
}

impl BuiltTransaction {
    pub fn price_impact_pct(&self) -> Option<Decimal> {
        match self {
            BuiltTransaction::Legacy {
                price_impact_pct, ..
            } => *price_impact_pct,
            BuiltTransaction::Versioned {
                price_impact_pct, ..
            } => *price_impact_pct,
        }
    }

    pub fn fill_price_lamports_per_base(&self) -> Option<Decimal> {
        match self {
            BuiltTransaction::Legacy {
                fill_price_lamports_per_base,
                ..
            } => *fill_price_lamports_per_base,
            BuiltTransaction::Versioned {
                fill_price_lamports_per_base,
                ..
            } => *fill_price_lamports_per_base,
        }
    }

    /// The recent blockhash the transaction was built/signed with. Threaded
    /// through to submission paths so they can reuse it instead of issuing
    /// extra `getLatestBlockhash` RPCs (P1-11).
    pub fn blockhash(&self) -> solana_sdk::hash::Hash {
        match self {
            BuiltTransaction::Legacy { blockhash, .. } => *blockhash,
            BuiltTransaction::Versioned { blockhash, .. } => *blockhash,
        }
    }

    /// Real per-route DEX fee in SOL, when available (P2-17).
    pub fn route_fee_sol(&self) -> Option<Decimal> {
        match self {
            BuiltTransaction::Legacy { route_fee_sol, .. } => *route_fee_sol,
            BuiltTransaction::Versioned { route_fee_sol, .. } => *route_fee_sol,
        }
    }
}

pub struct QuoteResult {
    pub price_impact_pct: Option<Decimal>,
    pub fill_price_lamports_per_base: Option<Decimal>,
    pub in_amount: u64,
    pub out_amount: u64,
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new(rpc_client: Arc<RpcClient>, config: Arc<AppConfig>) -> AppResult<Self> {
        Self::with_circuit_breaker(rpc_client, config, None)
    }

    /// Create a new transaction builder with circuit breaker integration
    pub fn with_circuit_breaker(
        rpc_client: Arc<RpcClient>,
        config: Arc<AppConfig>,
        circuit_breaker: Option<Arc<crate::circuit_breaker::CircuitBreaker>>,
    ) -> AppResult<Self> {
        // Create HTTP client with proper TLS and timeout configuration
        let http_client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                // If builder fails, return a proper error instead of panicking
                return Err(crate::error::AppError::Signal(format!(
                    "Failed to create HTTP client for TransactionBuilder: {}",
                    e
                )));
            }
        };

        let jupiter_url = config.jupiter.api_url.clone();
        let mut dex_comparator = match DexComparator::with_jupiter_api_url(jupiter_url) {
            Ok(comparator) => comparator,
            Err(e) => {
                return Err(crate::error::AppError::Signal(format!(
                    "Failed to create DexComparator: {}",
                    e
                )));
            }
        };
        // Honor the config: disable the per-DEX fan-out when multi_dex_comparison
        // is off (single aggregate quote only).
        dex_comparator.set_multi_dex(config.jupiter.multi_dex_comparison);

        Ok(Self {
            rpc_client,
            config,
            http_client,
            dex_comparator,
            circuit_breaker,
        })
    }

    /// Build a swap transaction for a signal.
    ///
    /// `slippage_bps` is the unified on-chain tolerance (see `engine::slippage`),
    /// threaded in from the executor so the same estimate drives both the
    /// Jupiter request and cost bookkeeping.
    ///
    /// This uses Jupiter Swap API which returns a pre-built transaction
    /// that just needs to be signed.
    pub async fn build_swap_transaction(
        &self,
        signal: &Signal,
        wallet_keypair: &Keypair,
        slippage_bps: u16,
    ) -> AppResult<BuiltTransaction> {
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
                        // F15/P1-12: refuse an empty sell. Rounding a tiny
                        // balance or a small exit_fraction to 0 lamports would
                        // submit a no-op swap; abort with a clear error instead.
                        if scaled_bal == 0 {
                            tracing::error!(
                                wallet = %wallet_keypair.pubkey(),
                                token = %signal.payload.token,
                                balance_lamports = bal,
                                exit_fraction = %exit_fraction,
                                "SELL: scaled token amount rounds to 0 — refusing empty sell"
                            );
                            return Err(crate::error::AppError::Validation(format!(
                                "SELL amount rounds to 0 (balance {} lamports × exit_fraction {}). \
                                 Increase exit_fraction or skip this sell.",
                                bal, exit_fraction
                            )));
                        }
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

        // Get swap transaction from Jupiter Swap API with enhanced error handling.
        // Uses retry logic with exponential backoff for resilient Jupiter API calls.
        let retry_config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_multiplier: 2.0,
            jitter_factor: 0.1,
        };

        let swap_response = execute_with_jupiter_error_handling(
            || self.get_jupiter_swap(
                input_mint,
                output_mint,
                amount,
                wallet_keypair.pubkey(),
                slippage_bps,
            ),
            &retry_config,
            "Jupiter swap API call"
        ).await;

        // Track Jupiter API failures/success with circuit breaker
        match &swap_response {
            Ok(_) => {
                // Reset failure counter on success
                if let Some(cb) = &self.circuit_breaker {
                    cb.reset_jupiter_failures();
                }
            }
            Err(e) => {
                // Record failure with circuit breaker
                if let Some(cb) = &self.circuit_breaker {
                    let error_type = format!("swap_error: {}", e);
                    if cb.record_jupiter_failure(error_type).unwrap_or(false) {
                        // Circuit breaker was tripped, return with circuit breaker error
                        return Err(AppError::CircuitBreaker(
                            "Jupiter API failures exceeded threshold - trading halted".to_string()
                        ));
                    }
                }
            }
        };

        swap_response?

        // Extract quote-derived fields before consuming swap_response
        let price_impact_pct = swap_response.price_impact_pct;
        let fill_price_lamports_per_base = swap_response.fill_price_lamports_per_base;
        let route_fee_sol = swap_response.route_fee_sol;

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
        let blockhash = crate::metrics::timed_rpc(
            "primary",
            "getLatestBlockhash",
            self.rpc_client.get_latest_blockhash(),
        )
        .await
        .map_err(|e| {
            crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e))
        })?;

        // Jupiter v1 API returns VersionedTransaction (starts with version byte 0x01)
        // Check the first byte to determine transaction type
        if !tx_bytes.is_empty() && tx_bytes[0] == 0x01 {
            // VersionedTransaction (version 1) - may be V0 or Legacy
            // Parse to sign. Unified bincode 2.x serde API, legacy config =
            // identical wire format to bincode 1.3 (Solana-compatible).
            let mut versioned_tx: VersionedTransaction =
                bincode::serde::decode_from_slice(&tx_bytes, bincode::config::legacy())
                    .map_err(|e| {
                        crate::error::AppError::Parse(format!("Failed to deserialize V0 tx: {}", e))
                    })?
                    .0;

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
                    tracing::debug!(
                        "Jupiter returned V0 transaction despite asLegacyTransaction=true. \
                         Refreshing the blockhash field directly (no ALT recompile)."
                    );

                    // F10: V0 Message fields are public — refresh is a direct
                    // field swap on a clone, then re-sign. No per-ALT RPCs.
                    use crate::engine::v0_reconstruction;
                    match v0_reconstruction::refresh_v0_blockhash(&versioned_tx, blockhash) {
                        Ok(refreshed_message) => {
                            tracing::debug!(
                                "Refreshed V0 message blockhash in transaction builder"
                            );
                            versioned_tx.message = refreshed_message;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to refresh V0 blockhash in builder. \
                                 Using original message — transaction may fail if blockhash expires."
                            );
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

            // Re-serialize signed transaction via the unified bincode 2.x API.
            let signed_bytes =
                bincode::serde::encode_to_vec(&versioned_tx, bincode::config::legacy()).map_err(
                    |e| crate::error::AppError::Parse(format!("Failed to re-serialize V0 tx: {}", e)),
                )?;

            Ok(BuiltTransaction::Versioned {
                transaction_bytes: signed_bytes,
                blockhash,
                price_impact_pct,
                fill_price_lamports_per_base,
                route_fee_sol,
            })
        } else {
            // Legacy Transaction
            if tx_bytes.is_empty() {
                return Err(crate::error::AppError::Parse(
                    "Transaction bytes are empty".to_string(),
                ));
            }

            // Legacy Transaction — unified bincode 2.x API, legacy config.
            let mut tx: Transaction =
                bincode::serde::decode_from_slice(&tx_bytes, bincode::config::legacy())
                    .map_err(|e| {
                        crate::error::AppError::Parse(format!(
                            "Failed to deserialize legacy transaction: {}",
                            e
                        ))
                    })?
                    .0;

            // Update blockhash and re-sign
            tx.message.recent_blockhash = blockhash;
            tx.sign(&[wallet_keypair], blockhash);

            Ok(BuiltTransaction::Legacy {
                transaction: tx,
                blockhash,
                price_impact_pct,
                fill_price_lamports_per_base,
                route_fee_sol,
            })
        }
    }

    #[allow(dead_code)]
    /// Build a simulated transaction for devnet testing
    async fn build_simulated_transaction(
        &self,
        _signal: &Signal,
        wallet_keypair: &Keypair,
    ) -> AppResult<BuiltTransaction> {
        let blockhash = crate::metrics::timed_rpc(
            "primary",
            "getLatestBlockhash",
            self.rpc_client.get_latest_blockhash(),
        )
        .await
        .map_err(|e| {
            crate::error::AppError::Rpc(format!("Failed to get blockhash: {}", e))
        })?;

        let empty_tx = Transaction::new_with_payer(&[], Some(&wallet_keypair.pubkey()));

        Ok(BuiltTransaction::Legacy {
            transaction: empty_tx,
            blockhash,
            price_impact_pct: None,
            fill_price_lamports_per_base: None,
            route_fee_sol: None,
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

        let accounts = crate::metrics::timed_rpc(
            "primary",
            "getTokenAccountsByOwner",
            self.rpc_client.get_token_accounts_by_owner(
                wallet_pubkey,
                TokenAccountsFilter::Mint(*token_mint),
            ),
        )
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

    /// Get swap transaction from Jupiter Swap API.
    ///
    /// For v2: Uses Meta-Aggregator `/order` endpoint with all routers competing
    /// (Metis, JupiterZ RFQ, Dflow, OKX) for best price. Includes RTSE, Jupiter Beam,
    /// and gasless support.
    ///
    /// For v1 fallback: Performs multi-DEX route comparison and posts winning quote
    /// to `/swap` (v1 Metis).
    async fn get_jupiter_swap(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
        user_public_key: Pubkey,
        slippage_bps: u16,
    ) -> AppResult<JupiterSwapResponse> {
        if self.config.jupiter.use_swap_v2 {
            // v2 Meta-Aggregator: single `/order` call with all routers competing
            self.get_jupiter_v2_order(input_mint, output_mint, amount, user_public_key, slippage_bps).await
        } else {
            // v1 fallback: multi-DEX comparison + swap
            self.get_jupiter_v1_swap(input_mint, output_mint, amount, user_public_key, slippage_bps).await
        }
    }

    /// Get swap transaction using Jupiter v2 Meta-Aggregator `/order` endpoint.
    ///
    /// This provides the best price as all routers compete (Metis, JupiterZ RFQ, Dflow, OKX).
    /// Includes RTSE (Real-Time Slippage Estimation), Jupiter Beam for MEV protection,
    /// and automatic gasless support.
    async fn get_jupiter_v2_order(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
        user_public_key: Pubkey,
        slippage_bps: u16,
    ) -> AppResult<JupiterSwapResponse> {
        // Determine if we should use RTSE (Real-Time Slippage Estimation)
        // RTSE automatically prioritizes slippage-protected routes
        let slippage_param = if self.config.jupiter.enable_rtse {
            "rtse"  // Let Jupiter determine optimal slippage based on market conditions
        } else {
            &slippage_bps.to_string()
        };

        // Build v2 /order request
        let url = format!("{}/order", self.config.jupiter.api_url.trim_end_matches('/'));

        let mut request_params = vec![
            ("inputMint", input_mint.to_string()),
            ("outputMint", output_mint.to_string()),
            ("amount", amount.to_string()),
            ("taker", user_public_key.to_string()),
            ("slippageBps", slippage_param.to_string()),
            ("swapMode", "ExactIn".to_string()),
        ];

        // Add optional routing parameters
        if self.config.jupiter.exclude_routers.is_some() {
            request_params.push(("excludeRouters", self.config.jupiter.exclude_routers.clone().unwrap()));
        }
        if self.config.jupiter.exclude_dexes.is_some() {
            request_params.push(("excludeDexes", self.config.jupiter.exclude_dexes.clone().unwrap()));
        }

        tracing::debug!(
            url = %url,
            input_mint = %input_mint,
            output_mint = %output_mint,
            amount = amount,
            slippage_bps = %slippage_param,
            "Requesting Jupiter v2 /order"
        );

        // Build URL with parameters
        let url_with_params = format!("{}/?{}", url, request_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&"));

        let response = crate::jupiter::with_api_key(
            self.http_client.get(&url_with_params),
        )
        .send()
        .await
        .map_err(|e| {
            crate::error::AppError::Http(format!("Jupiter v2 /order request failed: {}", e))
        })?;

        let status = response.status();
        let raw: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::AppError::Parse(format!("Failed to parse Jupiter v2 /order response: {}", e))
        })?;

        if !status.is_success() {
            let error_msg = raw.get("error")
                .and_then(|v| v.as_str())
                .or_else(|| raw.get("errorMessage").and_then(|v| v.as_str()))
                .unwrap_or("Unknown Jupiter API error");

            tracing::error!(
                status = %status,
                error = %error_msg,
                raw_response = ?raw,
                "Jupiter v2 /order request failed"
            );

            return Err(crate::error::AppError::Http(format!(
                "Jupiter v2 /order failed: {} - {}", status, error_msg
            )));
        }

        // Parse v2 /order response
        // v2 returns `transaction` (base64) or `quote` fields directly
        let swap_transaction = raw.get("transaction")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::AppError::Parse(format!(
                    "Jupiter v2 /order response missing 'transaction' field: {}",
                    raw
                ))
            })?
            .to_string();

        // Extract price impact from v2 response (decimal, e.g., -0.001 = -0.1%)
        let price_impact_pct: Option<Decimal> = raw.get("priceImpact")
            .and_then(|v| v.as_f64())
            .map(|p| Decimal::from_f64((p * 100.0).abs()).unwrap_or(Decimal::ZERO));

        // Extract route information for logging
        let router = raw.get("router")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let mode = raw.get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        tracing::info!(
            router = %router,
            mode = %mode,
            price_impact_pct = ?price_impact_pct,
            "Jupiter v2 /order successful"
        );

        // Compute fill price from amounts
        let in_amount = raw.get("inAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| raw.get("inAmount").and_then(|v| v.as_u64()));

        let out_amount = raw.get("outAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| raw.get("outAmount").and_then(|v| v.as_u64()));

        let fill_price_lamports_per_base = match (in_amount, out_amount) {
            (Some(inn), Some(out)) if out > 0 && inn > 0 => {
                Some(Decimal::from(inn) / Decimal::from(out))
            },
            _ => None,
        };

        // Extract fee information from routePlan
        let route_fee_sol: Option<Decimal> = raw.get("routePlan")
            .and_then(|plan| plan.as_array())
            .and_then(|steps| {
                let mut total_fee = Decimal::ZERO;
                for step in steps.iter().filter_map(|s| s.as_object()) {
                    if let Some(swap_info) = step.get("swapInfo").and_then(|si| si.as_object()) {
                        if let Some(fee_amount) = swap_info.get("feeAmount")
                            .and_then(|f| f.as_str().and_then(|s| Decimal::from_str(s).ok())) {
                            total_fee += fee_amount;
                        }
                    }
                }
                if total_fee > Decimal::ZERO {
                    Some(total_fee / Decimal::from(LAMPORTS_PER_SOL))
                } else {
                    None
                }
            });

        Ok(JupiterSwapResponse {
            swap_transaction,
            price_impact_pct,
            fill_price_lamports_per_base,
            route_fee_sol,
        })
    }

    /// Get swap transaction using Jupiter v1 fallback (multi-DEX comparison + swap).
    async fn get_jupiter_v1_swap(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
        user_public_key: Pubkey,
        slippage_bps: u16,
    ) -> AppResult<JupiterSwapResponse> {
        // Single multi-DEX comparison; the winning quote is reused for the swap.
        let route = self
            .dex_comparator
            .select_route(
                &input_mint.to_string(),
                &output_mint.to_string(),
                amount,
                slippage_bps,
            )
            .await?;
        let quote = route.quote;

        // Extract priceImpactPct from the quote for cost tracking in the executor.
        // Jupiter returns this as a percentage string, e.g. "1.234" = 1.234%.
        // (P1-6: percent interpretation is consistent across builder/comparator.)
        let price_impact_pct: Option<Decimal> = quote
            .get("priceImpactPct")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok());

        let selected_dex = route.selected_dex.as_str();

        // Then get the swap transaction using v1 Metis endpoint
        let url = format!("{}/swap", self.config.jupiter.api_url);

        // v1 swap endpoint payload
        let payload = serde_json::json!({
            "quoteResponse": quote,
            "userPublicKey": user_public_key.to_string(),
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": "auto",
            "asLegacyTransaction": true,
        });

        tracing::debug!(
            url = %url,
            selected_dex = %selected_dex,
            "Requesting Jupiter v1 swap (fallback)"
        );

        let response = crate::jupiter::with_api_key(
            self.http_client.post(url).json(&payload),
        )
        .send()
        .await
        .map_err(|e| {
            crate::error::AppError::Http(format!("Jupiter v1 swap request failed: {}", e))
        })?;

        let raw: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::AppError::Parse(format!("Failed to parse Jupiter v1 swap response: {}", e))
        })?;

        // v1 returns `swapTransaction`
        let swap_transaction = raw
            .get("swapTransaction")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::AppError::Parse(format!(
                    "Jupiter v1 swap response missing 'swapTransaction': {}",
                    raw
                ))
            })?
            .to_string();

        // Compute fill price from quote amounts
        let in_amount = quote
            .get("inAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| quote.get("inAmount").and_then(|v| v.as_u64()));

        let out_amount = quote
            .get("outAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| quote.get("outAmount").and_then(|v| v.as_u64()));

        let fill_price_lamports_per_base = match (in_amount, out_amount) {
            (Some(inn), Some(out)) if out > 0 && inn > 0 => {
                Some(Decimal::from(inn) / Decimal::from(out))
            },
            _ => None,
        };

        Ok(JupiterSwapResponse {
            swap_transaction,
            price_impact_pct,
            fill_price_lamports_per_base,
            route_fee_sol: Some(route.fee_sol),
        })
    }

            crate::error::AppError::Http(format!("Jupiter swap request failed: {}", e))
        })?;

        let raw: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::AppError::Parse(format!("Failed to parse Jupiter swap: {}", e))
        })?;

        // v2 returns `transaction`, v1 returns `swapTransaction`.
        let swap_transaction = raw
            .get("swapTransaction")
            .or_else(|| raw.get("transaction"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::AppError::Parse(format!(
                    "Jupiter swap response missing swapTransaction/transaction: {}",
                    raw
                ))
            })?
            .to_string();

        let mut swap_response = JupiterSwapResponse {
            swap_transaction,
            price_impact_pct: None,
            fill_price_lamports_per_base: None,
            route_fee_sol: Some(route.fee_sol),
        };
        swap_response.price_impact_pct = price_impact_pct;

        // Compute fill price from quote amounts: lamports_in / base_units_out
        swap_response.fill_price_lamports_per_base = {
            let in_amount = quote
                .get("inAmount")
                .and_then(|v| v.as_str().and_then(|s: &str| s.parse::<u64>().ok()))
                .or_else(|| quote.get("inAmount").and_then(|v| v.as_u64()));
            let out_amount = quote
                .get("outAmount")
                .and_then(|v| v.as_str().and_then(|s: &str| s.parse::<u64>().ok()))
                .or_else(|| quote.get("outAmount").and_then(|v| v.as_u64()));
            match (in_amount, out_amount) {
                (Some(inn), Some(out)) if out > 0 && inn > 0 => {
                    let in_dec = Decimal::from(inn);
                    let out_dec = Decimal::from(out);
                    if output_mint.to_string() == crate::constants::mints::SOL {
                        // SELL (TOKEN→SOL): out_amount is SOL lamports, in_amount is token base units
                        Some(out_dec / in_dec)
                    } else {
                        // BUY (SOL→TOKEN): in_amount is SOL lamports, out_amount is token base units
                        Some(in_dec / out_dec)
                    }
                }
                _ => None,
            }
        };

        Ok(swap_response)
    }

    /// Get a single (unrestricted) Jupiter quote. Used for paper/devnet price
    /// discovery where multi-DEX comparison is unnecessary.
    ///
    /// `dexes`, when set, restricts routing (used by route comparison).
    async fn get_jupiter_quote(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount: u64,
        slippage_bps: u16,
    ) -> AppResult<JupiterQuote> {
        let url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            self.config.jupiter.api_url, input_mint, output_mint, amount, slippage_bps
        );

        tracing::debug!(url = %url, "Requesting Jupiter quote");
        let response = crate::jupiter::with_api_key(self.http_client.get(&url))
            .send()
            .await
            .map_err(|e| {
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

    pub async fn get_quote_prices(
        &self,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
    ) -> AppResult<QuoteResult> {
        let input_mint = Pubkey::from_str(input_mint).map_err(|e| {
            crate::error::AppError::Validation(format!("Invalid input mint: {}", e))
        })?;
        let output_mint = Pubkey::from_str(output_mint).map_err(|e| {
            crate::error::AppError::Validation(format!("Invalid output mint: {}", e))
        })?;
        // Paper/devnet price discovery uses a generous tolerance; the quote's
        // `outAmount` is tolerance-independent so this does not skew the price.
        let quote = self
            .get_jupiter_quote(input_mint, output_mint, amount, 1000)
            .await?;

        let price_impact_pct = quote
            .get("priceImpactPct")
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok());

        let in_amount = quote
            .get("inAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| quote.get("inAmount").and_then(|v| v.as_u64()))
            .unwrap_or(0);

        let out_amount = quote
            .get("outAmount")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .or_else(|| quote.get("outAmount").and_then(|v| v.as_u64()))
            .unwrap_or(0);

        let fill_price_lamports_per_base = match (in_amount, out_amount) {
            (inn, out) if out > 0 && inn > 0 => {
                if output_mint.to_string() == crate::constants::mints::SOL {
                    Some(Decimal::from(out) / Decimal::from(inn))
                } else {
                    Some(Decimal::from(inn) / Decimal::from(out))
                }
            }
            _ => None,
        };

        Ok(QuoteResult {
            price_impact_pct,
            fill_price_lamports_per_base,
            in_amount,
            out_amount,
        })
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
    /// Real per-route DEX fee in SOL (P2-17), summed from
    /// `routePlan[].swapInfo.feeAmount`. `None` if the quote lacked route info.
    #[serde(skip)]
    pub route_fee_sol: Option<Decimal>,
}

/// Load wallet keypair from vault
pub fn load_wallet_keypair(secrets: &VaultSecrets) -> AppResult<Keypair> {
    let key_hex = secrets.wallet_private_key.as_ref().ok_or_else(|| {
        crate::error::AppError::Validation("Wallet private key not found in vault".to_string())
    })?;

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
