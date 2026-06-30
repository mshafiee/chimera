//! 🛡️ Safety-path validation harness.
//!
//! These tests exercise the signing-adjacent safety paths against **real**
//! Jupiter data. They are `#[ignore]`d (not run in CI) because they require a
//! Jupiter API key + network, and they parse rather than submit, so NO funds and
//! NO funded wallet are required.
//!
//! Run with:
//!   CHIMERA_JUPITER__API_KEY=... \
//!   cargo test --test safety_validation_tests -- --ignored --nocapture
//!
//! What this validates (the 🛡️ safety paths that need real-data confirmation):
//!   - `v0_refresh_preserves_real_jupiter_message`: a real Jupiter V0 swap tx is
//!     parsed, its blockhash is field-swapped via `refresh_v0_blockhash`, and we
//!     assert every field except the blockhash is byte-identical (no per-ALT RPC
//!     fetch / recompile). [P1-7]
//!   - `inline_tip_on_real_jupiter_legacy_tx`: a real Jupiter *legacy* swap tx
//!     has a Jito tip appended via `inline_jito_tip`; the originals are
//!     preserved verbatim and the System transfer lands last. [D3]
//!
//! What still requires a FUNDED wallet + landing (documented in the runbook, NOT
//! automated here — can't be done safely headless):
//!   - Real BUY→SELL round-trip (fill price non-zero, decimals-correct).
//!   - Jito bundle landing + getBundleStatuses resolution end-to-end.
//!   - Swap `simulateTransaction` against a mainnet RPC (needs the fee payer
//!     funded for the input asset).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chimera_operator::engine::{
    dex_comparator::DexComparator,
    tip_inlining::{decompile_legacy_message, inline_jito_tip},
    v0_reconstruction::refresh_v0_blockhash,
};
use chimera_operator::jupiter;
use solana_sdk::{
    hash::Hash,
    message::VersionedMessage,
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use std::str::FromStr;

const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// A throwaway valid pubkey used as the swap `userPublicKey`. We only parse the
/// returned (unsigned) transaction structure — we never sign or submit, so this
/// account never needs to exist or be funded.
const THROWAWAY_USER: &str = "11111111111111111111111111111111";
/// First official Jito tip account (verified constant) — used as the tip
/// destination in the inline-tip test. Mirrors jito_searcher::next_tip_account.
const TIP_ACCOUNT: &str = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4";

fn require_key() -> Option<String> {
    let key = std::env::var("CHIMERA_JUPITER__API_KEY").ok().filter(|v| !v.is_empty());
    if key.is_none() {
        eprintln!(
            "SKIP: set CHIMERA_JUPITER__API_KEY to run the safety validation harness (obtain from developers.jup.ag/portal)"
        );
    }
    key
}

/// Fetch a real (unsigned) swap transaction from Jupiter for SOL→USDC.
async fn fetch_swap_tx(use_legacy: bool) -> anyhow::Result<(VersionedTransaction, String)> {
    let key = std::env::var("CHIMERA_JUPITER__API_KEY")?;
    jupiter::set_api_key(Some(key.clone()));
    let comparator = DexComparator::new().map_err(|e| anyhow::anyhow!(e))?;
    // 0.01 SOL, generous slippage.
    let sel = comparator
        .select_route(SOL_MINT, USDC_MINT, 10_000_000, 1000)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let quote = sel.quote;

    let url = format!("{}/swap", "https://api.jup.ag/swap/v1");
    let payload = serde_json::json!({
        "quoteResponse": quote,
        "userPublicKey": THROWAWAY_USER,
        "wrapAndUnwrapSol": true,
        "asLegacyTransaction": use_legacy,
    });
    let resp = jupiter::with_api_key(reqwest::Client::new().post(&url).json(&payload))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Jupiter /swap returned {}: {}", resp.status(), resp.text().await?);
    }
    let val: serde_json::Value = resp.json().await?;
    let tx_b64 = val
        .get("swapTransaction")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no swapTransaction in response"))?;
    let tx_bytes = BASE64.decode(tx_b64)?;
    let tx: VersionedTransaction =
        bincode::serde::decode_from_slice(&tx_bytes, bincode::config::legacy())?.0;
    Ok((tx, tx_b64.to_string()))
}

async fn latest_blockhash() -> anyhow::Result<Hash> {
    // Use a public mainnet RPC just to obtain a current blockhash for the
    // field-swap assertion (no transaction is submitted).
    let rpc = solana_client::nonblocking::rpc_client::RpcClient::new(
        "https://api.mainnet-beta.solana.com".to_string(),
    );
    Ok(rpc.get_latest_blockhash().await?)
}

/// [P1-7] A real Jupiter V0 swap message survives a blockhash field-swap with
/// every other field byte-identical (no ALT fetch / recompile).
#[tokio::test]
#[ignore]
async fn v0_refresh_preserves_real_jupiter_message() {
    if require_key().is_none() {
        return;
    }
    let (tx, _b64) = match fetch_swap_tx(false).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("SKIP (could not fetch swap tx): {e}");
            return;
        }
    };
    let v0 = match &tx.message {
        VersionedMessage::V0(m) => m.clone(),
        VersionedMessage::Legacy(_) => {
            eprintln!("SKIP: Jupiter returned a legacy message, not V0 (asLegacyTransaction was ignored)");
            return;
        }
    };

    let fresh = match latest_blockhash().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("SKIP (no blockhash): {e}");
            return;
        }
    };

    let refreshed = refresh_v0_blockhash(&tx, fresh).expect("V0 refresh must succeed");
    match refreshed {
        VersionedMessage::V0(m) => {
            assert_eq!(m.recent_blockhash, fresh, "blockhash must be swapped");
            assert_ne!(m.recent_blockhash, v0.recent_blockhash);
            // Everything else preserved verbatim.
            assert_eq!(m.header, v0.header);
            assert_eq!(m.account_keys, v0.account_keys);
            assert_eq!(m.instructions, v0.instructions);
            assert_eq!(m.address_table_lookups, v0.address_table_lookups);
            println!(
                "OK: V0 message refreshed ({} account keys, {} instructions, {} ALTs) with no per-ALT RPC",
                m.account_keys.len(),
                m.instructions.len(),
                m.address_table_lookups.len()
            );
        }
        _ => panic!("expected V0"),
    }
}

/// [D3] A real Jupiter *legacy* swap tx gets the Jito tip inlined as the last
/// instruction; the originals are preserved verbatim and the System transfer is
/// present to an official tip account.
#[tokio::test]
#[ignore]
async fn inline_tip_on_real_jupiter_legacy_tx() {
    if require_key().is_none() {
        return;
    }
    let (tx, _b64) = match fetch_swap_tx(true).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("SKIP (could not fetch legacy swap tx — lite-api may ignore asLegacyTransaction): {e}");
            return;
        }
    };
    let legacy = match &tx.message {
        VersionedMessage::Legacy(m) => m.clone(),
        VersionedMessage::V0(_) => {
            eprintln!("SKIP: Jupiter returned V0 despite asLegacyTransaction=true (inline-tip is legacy-only; V0 keeps the separate-tip-bundle)");
            return;
        }
    };

    let original_ixs = decompile_legacy_message(&legacy).expect("decompile legacy");
    // Use the fee payer (account_keys[0]) as the tip source.
    let payer = legacy.account_keys[0];
    let tip_account = Pubkey::from_str(TIP_ACCOUNT).unwrap();
    let fresh = latest_blockhash().await.unwrap_or_default();

    let legacy_tx = solana_sdk::transaction::Transaction {
        signatures: vec![],
        message: legacy.clone(),
    };
    let tipped = inline_jito_tip(&legacy_tx, &payer, &tip_account, 1_000, fresh)
        .expect("inline tip must succeed");
    let tipped_ixs = decompile_legacy_message(&tipped.message).expect("decompile tipped");

    // One extra instruction (the tip), appended last.
    assert_eq!(tipped_ixs.len(), original_ixs.len() + 1);
    for (a, b) in tipped_ixs
        .iter()
        .take(original_ixs.len())
        .zip(original_ixs.iter())
    {
        assert_eq!(a.program_id, b.program_id);
        assert_eq!(a.data, b.data);
        assert_eq!(a.accounts, b.accounts, "original instructions must be preserved verbatim");
    }
    let tip = tipped_ixs.last().unwrap();
    assert_eq!(
        tip.program_id,
        Pubkey::from_str("11111111111111111111111111111111").unwrap()
    );
    assert!(tip.accounts.iter().any(|m| m.pubkey == tip_account && m.is_writable));
    println!(
        "OK: Jito tip inlined as last instruction on a real Jupiter legacy swap tx ({} originals preserved)",
        original_ixs.len()
    );
}
