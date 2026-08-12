//! Rent Scavenger - Reclaims rent from empty token accounts
//!
//! Periodically scans for empty token accounts (zero balance, no delegate)
//! and closes them to reclaim the 0.00204 SOL rent exemption per account.
//! Supports both legacy SPL Token and Token-2022 programs.

use crate::error::{AppError, AppResult};
use crate::metrics::RentScavengerMetrics;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
    instruction::{AccountMeta, Instruction},
};
use solana_account_decoder::UiAccountData;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tokio::time::sleep;
use tracing::{info, error, debug, warn};
use bincode;

/// Retry configuration for RPC calls
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 1000; // 1 second

/// Rent scavenger configuration
#[derive(Clone)]
pub struct RentScavengerConfig {
    /// Whether rent scavenging is enabled
    pub enabled: bool,
    /// Scavenging interval in seconds (default: 6 hours)
    pub interval_secs: u64,
    /// Maximum number of accounts to close per batch
    pub max_batch_size: usize,
    /// Maximum rent to reclaim per run (in lamports) as safety limit
    pub max_rent_lamports: u64,
}

impl Default for RentScavengerConfig {
    fn default() -> Self {
        Self {
            enabled: std::env::var("RENT_SCAVENGER_ENABLED")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(false),
            interval_secs: 6 * 3600, // 6 hours
            max_batch_size: 10,
            max_rent_lamports: 1_000_000_000, // 1 SOL safety limit
        }
    }
}

impl RentScavengerConfig {
    pub fn validate(&mut self) {
        if self.interval_secs < 300 {
            warn!(value = self.interval_secs, "RENT_SCAVENGER_INTERVAL_SECS below 300s, clamping to default");
            self.interval_secs = 6 * 3600;
        }
        if self.max_batch_size < 1 || self.max_batch_size > 20 {
            warn!(value = self.max_batch_size, "RENT_SCAVENGER_BATCH_SIZE out of range [1,20], clamping to default");
            self.max_batch_size = 10;
        }
        if self.max_rent_lamports < 1_000_000 {
            warn!(value = self.max_rent_lamports, "RENT_SCAVENGER_MAX_RENT_LAMPORTS below 0.001 SOL, clamping to default");
            self.max_rent_lamports = 1_000_000_000;
        }
    }
}

/// Rent scavenger - reclaims rent from empty token accounts
pub struct RentScavenger {
    rpc_url: String,
    funding_keypair: Arc<Keypair>,
    config: RentScavengerConfig,
    metrics: Option<Arc<RentScavengerMetrics>>,
}

impl RentScavenger {
    /// Retry helper for transient RPC failures with exponential backoff
    async fn retry_rpc<F, T, E>(
        &self,
        operation_name: &str,
        operation: F,
    ) -> Result<T, E>
    where
        F: Fn() -> Result<T, E>,
        E: std::fmt::Display,
    {
        let mut attempt = 0;
        let mut delay = Duration::from_millis(INITIAL_RETRY_DELAY_MS);
        
        loop {
            attempt += 1;
            match operation() {
                Ok(result) => {
                    if attempt > 1 {
                        debug!(
                            operation = operation_name,
                            attempts = attempt,
                            "RPC operation succeeded after retries"
                        );
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    let is_transient = error_msg.contains("timeout") 
                        || error_msg.contains("network")
                        || error_msg.contains("connection")
                        || error_msg.contains("503")
                        || error_msg.contains("429");
                    
                    if attempt >= MAX_RETRIES || !is_transient {
                        error!(
                            operation = operation_name,
                            attempts = attempt,
                            error = %e,
                            "RPC operation failed permanently"
                        );
                        if let Some(ref metrics) = self.metrics {
                            metrics.increment_errors();
                        }
                        return Err(e);
                    }
                    
                    warn!(
                        operation = operation_name,
                        attempt = attempt,
                        error = %e,
                        retry_delay_ms = delay.as_millis(),
                        "RPC operation failed transiently, retrying with exponential backoff"
                    );
                    
                    sleep(delay).await;
                    delay *= 2; // Exponential backoff
                }
            }
        }
    }

    /// Create a new rent scavenger
    pub fn new(
        rpc_url: String,
        funding_keypair: Arc<Keypair>,
        config: RentScavengerConfig,
        metrics: Option<Arc<RentScavengerMetrics>>,
    ) -> Self {
        Self {
            rpc_url,
            funding_keypair,
            config,
            metrics,
        }
    }

    /// Start the rent scavenger background task
    pub async fn start(self: Arc<Self>) -> AppResult<()> {
        if !self.config.enabled {
            info!("Rent scavenger disabled via RENT_SCAVENGER_ENABLED");
            return Ok(());
        }

        info!(
            interval_secs = self.config.interval_secs,
            max_batch_size = self.config.max_batch_size,
            "Starting rent scavenger"
        );

        let scavenger = Arc::clone(&self);
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(scavenger.config.interval_secs));
            
            // Initial run
            if let Err(e) = scavenger.reclaim_empty_accounts().await {
                error!(error = %e, "Initial rent scavenger run failed");
            }

            loop {
                ticker.tick().await;
                debug!("Rent scavenger tick");
                
                if let Err(e) = scavenger.reclaim_empty_accounts().await {
                    error!(error = %e, "Rent scavenger run failed");
                }
            }
        });

        Ok(())
    }

    /// Reclaim rent from empty token accounts
    pub async fn reclaim_empty_accounts(&self) -> AppResult<()> {
        let start_time = std::time::Instant::now();
        let owner = self.funding_keypair.pubkey();
        
        // Process both legacy and Token-2022 programs
        let programs = vec![
            ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", "Token"),
            ("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", "Token-2022"),
        ];

        let mut total_closed = 0u64;
        let mut total_rent_reclaimed = 0u64;

        for (program_id, program_name) in programs {
            match self.reclaim_empty_accounts_for_program(&owner, program_id, program_name).await {
                Ok((closed, rent_reclaimed)) => {
                    total_closed += closed;
                    total_rent_reclaimed += rent_reclaimed;
                }
                Err(e) => {
                    warn!(
                        program = program_name,
                        error = %e,
                        "Failed to reclaim empty accounts for program"
                    );
                    if let Some(ref metrics) = self.metrics {
                        metrics.increment_errors();
                    }
                }
            }
        }

        if total_closed > 0 {
            info!(
                accounts_closed = total_closed,
                rent_reclaimed_lamports = total_rent_reclaimed,
                rent_reclaimed_sol = total_rent_reclaimed as f64 / 1_000_000_000.0,
                "Rent scavenger completed successfully"
            );
            
            if let Some(ref metrics) = self.metrics {
                metrics.increment_accounts_closed(total_closed);
                metrics.increment_rent_reclaimed(total_rent_reclaimed);
            }
        } else {
            debug!("No empty token accounts found to close");
        }
        
        if let Some(ref metrics) = self.metrics {
            metrics.record_run_duration(start_time.elapsed());
        }

        Ok(())
    }

    /// Reclaim empty token accounts for a specific token program
    async fn reclaim_empty_accounts_for_program(
        &self,
        owner: &Pubkey,
        program_id: &str,
        program_name: &str,
    ) -> AppResult<(u64, u64)> {
        let program_pubkey = program_id
            .parse::<Pubkey>()
            .map_err(|e| AppError::Internal(format!("Invalid program ID {}: {}", program_id, e)))?;

        // Create blocking RPC client for this operation
        let rpc_client = RpcClient::new(self.rpc_url.clone());

        // Get all token accounts for this program
        use solana_client::rpc_request::TokenAccountsFilter;
        
        let accounts = rpc_client
            .get_token_accounts_by_owner(
                owner,
                TokenAccountsFilter::ProgramId(program_pubkey),
            )
            .map_err(|e| {
                AppError::Rpc(format!(
                    "Failed to fetch token accounts for {}: {}",
                    program_name, e
                ))
            })?;

        // Find empty accounts (zero balance, no delegate, closable by this keypair)
        let mut empty_accounts = Vec::new();
        // Rent exemption is queried once per distinct account size (Token-2022
        // extension accounts can be larger than 165 bytes).
        let mut rent_cache: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();

        for keyed_account in accounts {
            if let UiAccountData::Json(parsed) = keyed_account.account.data {
                // jsonParsed token accounts nest the balance/authority fields
                // under `parsed.parsed.info`:
                //   info = { tokenAmount: { amount, ... }, delegate, state, closeAuthority }
                // (Account data length lives on the ParsedAccount struct as
                // `parsed.space`, not inside `info`.)
                if let Some(info) = parsed.parsed.get("info") {
                    if !Self::is_closable_empty_account(info, owner) {
                        continue;
                    }
                    if let Ok(account_pubkey) = keyed_account.pubkey.parse::<Pubkey>() {
                        // `space` is a field of ParsedAccount (a sibling of the
                        // inner `parsed` JSON), NOT a key inside `info`. It
                        // reports the on-chain account data length, so Token-2022
                        // extension accounts correctly report >165 bytes here.
                        let space: usize = if parsed.space > 0 {
                            parsed.space as usize
                        } else {
                            165
                        };
                        let rent = match rent_cache.get(&space) {
                            Some(r) => *r,
                            None => {
                                let r = rpc_client
                                    .get_minimum_balance_for_rent_exemption(space)
                                    .map_err(|e| {
                                        AppError::Rpc(format!(
                                            "Failed to get rent exemption for {} (space {}): {}",
                                            program_name, space, e
                                        ))
                                    })?;
                                rent_cache.insert(space, r);
                                r
                            }
                        };
                        empty_accounts.push((account_pubkey, rent));
                    }
                }
            }
        }

        if empty_accounts.is_empty() {
            return Ok((0, 0));
        }

        debug!(
            program = program_name,
            empty_accounts = empty_accounts.len(),
            "Found empty token accounts"
        );

        // Re-verification snapshot: fetch the current empty set ONCE per program
        // instead of once per batch (O(batches × total) round-trips otherwise).
        let current_empty = self.fetch_current_empty_accounts(&rpc_client, &program_pubkey, owner)?;

        // Close accounts in batches
        let mut total_closed = 0u64;
        let mut total_rent_reclaimed = 0u64;

        for batch in empty_accounts.chunks(self.config.max_batch_size) {
            // Re-verify accounts are still empty before closing (prevents race conditions)
            let verified_batch: Vec<(Pubkey, u64)> = batch
                .iter()
                .filter(|(account_pubkey, _)| current_empty.contains(account_pubkey))
                .cloned()
                .collect();
            
            if verified_batch.is_empty() {
                debug!("All accounts in batch failed re-verification, skipping");
                continue;
            }
            
            // Safety check: don't exceed max rent limit
            let batch_rent: u64 = verified_batch.iter().map(|(_, rent)| rent).sum();
            if total_rent_reclaimed + batch_rent > self.config.max_rent_lamports {
                warn!(
                    current_rent_lamports = total_rent_reclaimed,
                    batch_rent_lamports = batch_rent,
                    max_rent_lamports = self.config.max_rent_lamports,
                    "Rent scavenger safety limit reached, stopping"
                );
                break;
            }

            match self.close_token_accounts_batch(&verified_batch, &program_pubkey, &rpc_client).await {
                Ok(closed) => {
                    total_closed += closed;
                    total_rent_reclaimed += batch_rent;
                }
                Err(e) => {
                    warn!(
                        program = program_name,
                        batch_size = verified_batch.len(),
                        error = %e,
                        "Failed to close batch of token accounts"
                    );
                }
            }
        }

        Ok((total_closed, total_rent_reclaimed))
    }

    /// Whether a parsed token account is a safe close candidate: zero balance,
    /// no delegate, not frozen, and either no close authority or one this
    /// funding keypair can exercise.
    fn is_closable_empty_account(info: &serde_json::Value, owner: &Pubkey) -> bool {
        let amount_zero = info
            .get("tokenAmount")
            .and_then(|t| t.get("amount"))
            .and_then(|a| a.as_str())
            .map(|s| s == "0")
            .unwrap_or(false);
        if !amount_zero {
            return false;
        }
        // Accounts with a delegate cannot be closed.
        let has_delegate = info
            .get("delegate")
            .and_then(|d| d.as_str())
            .map(|s| !s.is_empty() && s != "11111111111111111111111111111111")
            .unwrap_or(false);
        if has_delegate {
            return false;
        }
        // Frozen accounts cannot be closed.
        if info
            .get("state")
            .and_then(|s| s.as_str())
            .map(|s| s == "frozen")
            .unwrap_or(false)
        {
            return false;
        }
        // closeAuthority must be absent, the system program (i.e. effectively
        // none), or this funding keypair — otherwise the batch transaction
        // would fail with the funding keypair's signature.
        let owner_str = owner.to_string();
        let close_authority_ok = info
            .get("closeAuthority")
            .and_then(|d| d.as_str())
            .map(|s| {
                s.is_empty()
                    || s == "11111111111111111111111111111111"
                    || s == owner_str
            })
            .unwrap_or(true);
        close_authority_ok
    }

    /// Fetch the set of currently-closable empty token accounts for a program.
    /// The empty set is re-fetched once per program run and shared by every
    /// batch re-verification.
    fn fetch_current_empty_accounts(
        &self,
        rpc_client: &RpcClient,
        program_pubkey: &Pubkey,
        owner: &Pubkey,
    ) -> AppResult<std::collections::HashSet<Pubkey>> {
        use solana_client::rpc_request::TokenAccountsFilter;

        let current_accounts = rpc_client
            .get_token_accounts_by_owner(owner, TokenAccountsFilter::ProgramId(*program_pubkey))
            .map_err(|e| {
                AppError::Rpc(format!("Failed to fetch current token accounts: {}", e))
            })?;

        let mut current_empty: std::collections::HashSet<Pubkey> = std::collections::HashSet::new();
        for keyed_account in current_accounts {
            if let UiAccountData::Json(parsed) = keyed_account.account.data {
                if let Some(info) = parsed.parsed.get("info") {
                    if Self::is_closable_empty_account(info, owner) {
                        if let Ok(account_pubkey) = keyed_account.pubkey.parse::<Pubkey>() {
                            current_empty.insert(account_pubkey);
                        }
                    }
                }
            }
        }
        Ok(current_empty)
    }

    /// Close a batch of token accounts.
    ///
    /// If the whole batch fails (e.g. one account was closed/frozen between
    /// verification and send), retry each account individually so a single bad
    /// account cannot block the others; per-account failures are non-fatal.
    async fn close_token_accounts_batch(
        &self,
        accounts: &[(Pubkey, u64)],
        program_id: &Pubkey,
        rpc_client: &RpcClient,
    ) -> AppResult<u64> {
        if accounts.is_empty() {
            return Ok(0);
        }

        match self.send_close_transaction(accounts, program_id, rpc_client).await {
            Ok(sig) => {
                info!(
                    accounts_closed = accounts.len(),
                    signature = %sig,
                    "Successfully closed token accounts batch"
                );
                Ok(accounts.len() as u64)
            }
            Err(e) => {
                warn!(
                    batch_size = accounts.len(),
                    error = %e,
                    "Batch close failed — retrying per-account (non-fatal)"
                );
                let mut closed = 0u64;
                for (account_pubkey, rent) in accounts {
                    let single = [(*account_pubkey, *rent)];
                    match self.send_close_transaction(&single, program_id, rpc_client).await {
                        Ok(sig) => {
                            debug!(account = %account_pubkey, signature = %sig, "Closed account individually");
                            closed += 1;
                        }
                        Err(per_account_err) => {
                            warn!(
                                account = %account_pubkey,
                                error = %per_account_err,
                                "Failed to close account individually; skipping"
                            );
                        }
                    }
                }
                Ok(closed)
            }
        }
    }

    /// Send one CloseAccount transaction for the given accounts.
    async fn send_close_transaction(
        &self,
        accounts: &[(Pubkey, u64)],
        program_id: &Pubkey,
        rpc_client: &RpcClient,
    ) -> AppResult<solana_sdk::signature::Signature> {
        let owner = self.funding_keypair.pubkey();
        let recent_blockhash = rpc_client
            .get_latest_blockhash()
            .map_err(|e| AppError::Rpc(format!("Failed to get blockhash: {}", e)))?;

        let mut instructions = Vec::new();

        for (account_pubkey, _) in accounts {
            // Build CloseAccount instruction manually
            // CloseAccount instruction layout:
            // - 0: [WRITE] Account to close
            // - 1: [WRITE] Destination account for rent (owner)
            // - 2: [] Authority (signer)
            
            let instruction = Instruction {
                program_id: *program_id,
                accounts: vec![
                    AccountMeta::new(*account_pubkey, false),
                    AccountMeta::new(owner, false),
                    AccountMeta::new_readonly(owner, true),
                ],
                data: vec![9], // CloseAccount instruction discriminator
            };

            instructions.push(instruction);
        }

        let mut transaction = Transaction::new_with_payer(
            &instructions,
            Some(&owner),
        );
        
        // Sign the transaction
        transaction.sign(&[self.funding_keypair.as_ref()], recent_blockhash);

        // Validate transaction size before submission (max 1232 bytes)
        let tx_bytes = bincode::serde::encode_to_vec(&transaction, bincode::config::legacy())
            .map_err(|e| AppError::Internal(format!("Failed to serialize transaction: {}", e)))?;
        
        let tx_size = tx_bytes.len();
        
        if tx_size > 1232 {
            return Err(AppError::Internal(format!(
                "Transaction too large: {} bytes (max 1232), consider reducing batch size",
                tx_size
            )));
        }

        debug!(
            tx_size = tx_size,
            accounts_count = accounts.len(),
            "Transaction size validated"
        );

        // Send transaction with retry logic for transient failures
        let signature = self.retry_rpc("send_close_account_transaction", || {
            rpc_client
                .send_and_confirm_transaction(&transaction)
                .map_err(|e| {
                    AppError::Rpc(format!("Failed to send close account transaction: {}", e))
                })
        }).await?;

        Ok(signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validate_clamps_low_interval() {
        let mut config = RentScavengerConfig {
            enabled: true,
            interval_secs: 10, // below 300s minimum
            max_batch_size: 10,
            max_rent_lamports: 1_000_000_000,
        };
        config.validate();
        assert_eq!(config.interval_secs, 6 * 3600);
    }

    #[test]
    fn test_config_validate_clamps_batch_size() {
        let mut config = RentScavengerConfig {
            enabled: true,
            interval_secs: 3600,
            max_batch_size: 0,   // below minimum
            max_rent_lamports: 1_000_000_000,
        };
        config.validate();
        assert_eq!(config.max_batch_size, 10);

        let mut config2 = config.clone();
        config2.max_batch_size = 100; // above maximum
        config2.validate();
        assert_eq!(config2.max_batch_size, 10);
    }

    #[test]
    fn test_config_validate_clamps_max_rent() {
        let mut config = RentScavengerConfig {
            enabled: true,
            interval_secs: 3600,
            max_batch_size: 10,
            max_rent_lamports: 100, // below 0.001 SOL
        };
        config.validate();
        assert_eq!(config.max_rent_lamports, 1_000_000_000);
    }

    #[test]
    fn test_config_validate_passes_valid_values() {
        let mut config = RentScavengerConfig {
            enabled: true,
            interval_secs: 7200,
            max_batch_size: 15,
            max_rent_lamports: 500_000_000,
        };
        config.validate();
        assert_eq!(config.interval_secs, 7200);
        assert_eq!(config.max_batch_size, 15);
        assert_eq!(config.max_rent_lamports, 500_000_000);
    }

    // ==========================================================================
    // is_closable_empty_account
    // ==========================================================================

    fn owner() -> Pubkey {
        Pubkey::new_unique()
    }

    fn info_json(fields: &[(&str, &str)]) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for (k, v) in fields {
            obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
        serde_json::Value::Object(obj)
    }

    fn amount_info(amount: &str) -> serde_json::Value {
        let mut amount_obj = serde_json::Map::new();
        amount_obj.insert(
            "amount".to_string(),
            serde_json::Value::String(amount.to_string()),
        );
        let mut info = serde_json::Map::new();
        info.insert("tokenAmount".to_string(), serde_json::Value::Object(amount_obj));
        serde_json::Value::Object(info)
    }

    /// Build a full parsed account `info` object: `tokenAmount` is the nested
    /// object the real Helius/jsonParsed payload uses, plus any extra fields.
    fn full_info(amount: &str, extra: &[(&str, &str)]) -> serde_json::Value {
        let mut amount_obj = serde_json::Map::new();
        amount_obj.insert(
            "amount".to_string(),
            serde_json::Value::String(amount.to_string()),
        );
        let mut info = serde_json::Map::new();
        info.insert(
            "tokenAmount".to_string(),
            serde_json::Value::Object(amount_obj),
        );
        for (k, v) in extra {
            info.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
        serde_json::Value::Object(info)
    }

    #[test]
    fn test_closable_empty_account_with_no_delegate() {
        let owner = owner();
        let info = full_info("0", &[("delegate", ""), ("state", "initialized")]);
        assert!(
            RentScavenger::is_closable_empty_account(&info, &owner),
            "empty account with no delegate should be closable"
        );
    }

    #[test]
    fn test_non_zero_balance_not_closable() {
        let owner = owner();
        let info = amount_info("12345");
        assert!(!RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_missing_token_amount_not_closable() {
        let owner = owner();
        let info = info_json(&[]);
        assert!(!RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_delegate_present_not_closable() {
        let owner = owner();
        let info = full_info("0", &[("delegate", "SomeDelegate")]);
        assert!(!RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_system_program_delegate_ignored() {
        let owner = owner();
        // 11111111111111111111111111111111 is treated as "no delegate".
        let info = full_info("0", &[("delegate", "11111111111111111111111111111111")]);
        assert!(RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_frozen_account_not_closable() {
        let owner = owner();
        let info = full_info("0", &[("delegate", ""), ("state", "frozen")]);
        assert!(!RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_close_authority_owner_allowed() {
        let owner = owner();
        let info = full_info(
            "0",
            &[("delegate", ""), ("closeAuthority", &owner.to_string())],
        );
        assert!(RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_close_authority_system_program_allowed() {
        let owner = owner();
        let info = full_info(
            "0",
            &[
                ("delegate", ""),
                ("closeAuthority", "11111111111111111111111111111111"),
            ],
        );
        assert!(RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_foreign_close_authority_not_closable() {
        let owner = owner();
        let info = full_info(
            "0",
            &[
                ("delegate", ""),
                ("closeAuthority", "SomeOtherAuthority"),
            ],
        );
        assert!(!RentScavenger::is_closable_empty_account(&info, &owner));
    }

    #[test]
    fn test_default_retry_and_initial_delay_constants() {
        // Smoke-check the module-level constants referenced by retry_rpc.
        assert_eq!(MAX_RETRIES, 3);
        assert_eq!(INITIAL_RETRY_DELAY_MS, 1000);
    }
}