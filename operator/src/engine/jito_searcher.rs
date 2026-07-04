//! Direct Jito Searcher integration for bundle submission
//!
//! This module provides direct integration with Jito Searcher API,
//! allowing bundle submission without requiring Helius Sender API.

use crate::engine::executor::ExecutorError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use solana_system_interface::instruction as system_instruction;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Official Jito tip accounts (mainnet), verbatim from the Jito docs
/// `getTipAccounts` response. Hardcoded as both the rotating set AND the
/// allowlist: a runtime `getTipAccounts` fetch would blindly trust whatever the
/// endpoint returns (fund-diversion risk if the endpoint is MITM'd or pointed at
/// a non-official relay), so the verified constants are authoritative.
const OFFICIAL_JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4",
    "HFqU5x63VTqvQss8hp11i4VV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

/// Round-robin cursor over the official tip accounts.
static TIP_ROTATION: AtomicU64 = AtomicU64::new(0);

/// Pick the next official Jito tip account (round-robin). Jito docs: "select one
/// of the [tip] accounts at random to reduce contention." Used by every tip path
/// (direct Jito + Helius), so they rotate identically.
pub fn next_tip_account() -> Pubkey {
    let idx = TIP_ROTATION.fetch_add(1, Ordering::Relaxed);
    let addr = OFFICIAL_JITO_TIP_ACCOUNTS[(idx as usize) % OFFICIAL_JITO_TIP_ACCOUNTS.len()];
    Pubkey::from_str(addr).expect("official Jito tip account (verified constant)")
}

/// Poll a `getBundleStatuses` endpoint for a bundle's landed transaction
/// signatures, returning the **last** one — the swap signature. Bundles execute
/// in submission order, so the swap is last for both single-tx inlined
/// (`[swap]`) and two-tx (`[tip, swap]`) bundles. Bounded backoff (~2.5s ceiling
/// on the live path). Shared by the Jito and Helius resolvers.
pub async fn resolve_bundle_status(
    http_client: &reqwest::Client,
    url: &str,
    bundle_id: &str,
) -> Option<String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBundleStatuses",
        "params": [[bundle_id]]
    });
    // Poll immediately, then back off (~2.5s total over 6 attempts).
    let backoff_ms: [u64; 6] = [0, 300, 400, 500, 600, 700];
    for (attempt, &delay) in backoff_ms.iter().enumerate() {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        if let Ok(resp) = http_client.post(url).json(&payload).send().await {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(sig) = extract_swap_signature_from_bundle(&val) {
                    return Some(sig);
                }
            }
        }
        tracing::debug!(attempt, bundle_id, "Bundle not yet resolved; retrying");
    }
    None
}

/// Extract the SWAP signature (the LAST in `result.value[].transactions`) from a
/// `getBundleStatuses` response. Bundles execute in submission order, so the
/// swap — submitted last in both `[swap]` and `[tip, swap]` bundles — is the
/// final entry. Returns `None` if the bundle hasn't landed or the shape is wrong.
pub fn extract_swap_signature_from_bundle(value: &serde_json::Value) -> Option<String> {
    value
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|entry| entry.get("transactions"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.last())
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}


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
    pub fn new(endpoint: String, rpc_client: Arc<RpcClient>) -> Result<Self, String> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            endpoint,
            http_client,
            rpc_client,
        })
    }

    /// Get the Jito endpoint URL
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Submit a **single-transaction** bundle (the swap tx already has the tip
    /// inlined — see `engine::tip_inlining`). D3: one signature, all-or-nothing
    /// at the transaction level, replacing the `[tip_tx, swap_tx]` two-tx bundle
    /// for legacy transactions. Returns a `bundle:<uuid>` ref the caller must
    /// resolve to a signature (F12) before polling.
    pub async fn submit_single_bundle(
        &self,
        tipped_tx_bytes: &[u8],
    ) -> Result<String, ExecutorError> {
        let tx_base64 = BASE64.encode(tipped_tx_bytes);
        self.send_bundle(vec![tx_base64]).await
    }

    /// Shared `sendBundle` JSON-RPC against `{endpoint}/api/v1/bundles`.
    /// Returns a `bundle:<uuid>` ref. Owns URL/payload/status/parse so the
    /// single-tx and two-tx paths cannot drift.
    async fn send_bundle(&self, txs_base64: Vec<String>) -> Result<String, ExecutorError> {
        let url = format!("{}/api/v1/bundles", self.endpoint);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [txs_base64]
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

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ExecutorError::Rpc(format!("Failed to parse Jito response: {}", e)))?;

        // sendBundle returns the bundle UUID as `result` (a string) — NOT a
        // signature. Tag it so the caller resolves it (F12).
        let bundle_id = result
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| ExecutorError::Rpc("No result/bundleId in Jito response".to_string()))?;

        Ok(format!("bundle:{}", bundle_id))
    }

    /// Resolve a bundle UUID to its real landed SWAP transaction signature.
    ///
    /// `sendBundle` returns only a UUID (F12: it was being polled as a
    /// signature). `getBundleStatuses` returns the bundle's `transactions`
    /// array (in bundle-submission order) once it lands. Because the swap is
    /// always the LAST element of the bundle — both single-tx inlined
    /// (`[swap]`) and two-tx (`[tip, swap]`) — we return the LAST signature,
    /// not the first (which would be the tip).
    ///
    /// Polls briefly (the bundle needs a slot to land). Returns `None` if it
    /// could not be resolved — callers must then treat the trade as
    /// *unconfirmed* and let recovery reconcile it, never poll the UUID itself.
    pub async fn resolve_bundle_signature(&self, bundle_id: &str) -> Option<String> {
        let url = format!("{}/api/v1/bundles", self.endpoint);
        resolve_bundle_status(&self.http_client, &url, bundle_id).await
    }

    /// Submit a bundle to Jito Searcher
    ///
    /// Creates a bundle with:
    /// 1. Tip transaction (to a rotated tip account)
    /// 2. Swap transaction (the actual trade)
    ///
    /// Returns the **bundle UUID** (Jito's `sendBundle` returns a UUID, not a
    /// transaction signature). Callers must resolve it to a real signature via
    /// [`Executor::resolve_bundle_signature`] before polling confirmation —
    /// never poll the UUID itself as a signature (F12).
    pub async fn submit_bundle(
        &self,
        swap_tx_bytes: &[u8],
        tip_lamports: u64,
        tip_keypair: &Keypair,
    ) -> Result<String, ExecutorError> {
        // Create tip transaction. The tip account rotates across the hardcoded
        // official 8 accounts (F18/P2-14) via `next_tip_account()`.
        let tip_transaction = self
            .create_tip_transaction(tip_lamports, tip_keypair)
            .await
            .map_err(|e| {
                ExecutorError::TransactionFailed(format!("Failed to create tip transaction: {}", e))
            })?;

        // Build bundle: tip transaction first, then swap transaction
        let tip_tx_bytes =
            bincode::serde::encode_to_vec(&tip_transaction, bincode::config::legacy()).map_err(
                |e| ExecutorError::TransactionFailed(format!("Failed to serialize tip tx: {}", e)),
            )?;
        let tip_tx_base64 = BASE64.encode(&tip_tx_bytes);

        // Encode the pre-serialized swap transaction
        let swap_tx_base64 = BASE64.encode(swap_tx_bytes);

        // Jito Searcher API expects bundle in specific format: [tip, swap]
        self.send_bundle(vec![tip_tx_base64, swap_tx_base64]).await
    }

    /// Create a tip transaction to a rotated Jito tip account
    async fn create_tip_transaction(
        &self,
        tip_lamports: u64,
        tip_keypair: &Keypair,
    ) -> Result<Transaction, ExecutorError> {
        // Rotate across the (runtime-fetched) tip accounts (F18/P2-14).
        let jito_tip_account = next_tip_account();

        // Get recent blockhash from RPC
        let recent_blockhash = crate::metrics::timed_rpc(
            "jito",
            "getLatestBlockhash",
            self.rpc_client.get_latest_blockhash(),
        )
        .await
        .map_err(|e| {
            ExecutorError::Rpc(format!("Failed to get recent blockhash: {}", e))
        })?;

        // Create tip instruction
        let tip_instruction =
            system_instruction::transfer(&tip_keypair.pubkey(), &jito_tip_account, tip_lamports);

        // Build transaction
        let mut transaction =
            Transaction::new_with_payer(&[tip_instruction], Some(&tip_keypair.pubkey()));

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
        let rpc_client = Arc::new(RpcClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
        ));
        let client = JitoSearcherClient::new(
            "https://mainnet.block-engine.jito.wtf".to_string(),
            rpc_client,
        )
        .expect("Failed to create JitoSearcherClient for test");
        assert_eq!(client.endpoint, "https://mainnet.block-engine.jito.wtf");
    }

    #[tokio::test]
    async fn test_tip_transaction_creation_no_network() {
        let keypair = Keypair::new();
        // Use a URL guaranteed unreachable so the RPC call always fails (true
        // in CI / without network — the old tautology `is_err() || is_ok()`
        // always passed, masking that nothing was tested).
        let rpc = RpcClient::new("http://127.0.0.1:1".to_string());
        let client = JitoSearcherClient::new(
            "https://mainnet.block-engine.jito.wtf".to_string(),
            Arc::new(rpc),
        )
        .expect("Failed to create JitoSearcherClient");

        let result = client.create_tip_transaction(1_000_000, &keypair).await;
        assert!(result.is_err(), "should error without network RPC");
    }

    /// 🛡️ safety: bundle→signature resolution must return the SWAP signature
    /// (LAST), not the tip (first), and must read `result.value` (not `result`
    /// as an array). Uses the official Jito `getBundleStatuses` example, where
    /// the two-tx bundle is `[tip, swap]` → `transactions = [tip_sig, swap_sig]`.
    #[test]
    fn extract_swap_signature_picks_last_and_reads_value() {
        // Verbatim shape from the Jito docs getBundleStatuses response example.
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "result": {
                "context": { "slot": 242806119 },
                "value": [{
                    "bundle_id": "892b79ed49138bfb3aa5441f0df6e06ef34f9ee8f3976c15b323605bae0cf51d",
                    "transactions": [
                        "TIP_SIGNATURE_FIRST",
                        "SWAP_SIGNATURE_LAST"
                    ],
                    "slot": 242804011,
                    "confirmation_status": "finalized",
                    "err": { "Ok": null }
                }]
            },
            "id": 1
        });
        assert_eq!(
            extract_swap_signature_from_bundle(&resp).as_deref(),
            Some("SWAP_SIGNATURE_LAST"),
            "must return the LAST (swap) signature, not the tip"
        );
    }

    /// Single-tx inlined bundle `[swap]` → only one signature, which is the swap.
    #[test]
    fn extract_swap_signature_single_tx_bundle() {
        let resp = serde_json::json!({
            "result": { "value": [{ "transactions": ["ONLY_SWAP_SIG"] }] }
        });
        assert_eq!(
            extract_swap_signature_from_bundle(&resp).as_deref(),
            Some("ONLY_SWAP_SIG")
        );
    }

    /// Bundle not landed / wrong shape → None (caller marks unconfirmed).
    #[test]
    fn extract_swap_signature_none_when_not_landed() {
        assert_eq!(extract_swap_signature_from_bundle(&serde_json::json!({ "result": null })), None);
        assert_eq!(
            extract_swap_signature_from_bundle(&serde_json::json!({ "result": { "value": [] } })),
            None
        );
    }

    /// Official 8 tip accounts parse and are distinct (allowlist integrity).
    #[test]
    fn official_tip_accounts_are_valid_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for addr in OFFICIAL_JITO_TIP_ACCOUNTS {
            let pk = Pubkey::from_str(addr).expect("official tip account must parse");
            assert!(seen.insert(pk), "duplicate official tip account: {}", addr);
        }
        assert_eq!(OFFICIAL_JITO_TIP_ACCOUNTS.len(), 8);
    }

    /// Rotation cycles through the official set.
    #[test]
    fn tip_rotation_covers_all_official_accounts() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..OFFICIAL_JITO_TIP_ACCOUNTS.len() {
            seen.insert(next_tip_account());
        }
        assert_eq!(seen.len(), OFFICIAL_JITO_TIP_ACCOUNTS.len());
    }
}
