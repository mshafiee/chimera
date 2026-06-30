//! V0 transaction blockhash refresh.
//!
//! Previously this module recompiled a V0 message from `CompiledInstruction`s
//! with *heuristic* signer/writable derivation plus per-ALT `getAccountData`
//! RPCs (~280 lines, fragile). That is unnecessary: every field of a V0
//! [`Message`] is public, so refreshing a stale blockhash is a direct field
//! swap on a clone, followed by re-signing at the call site.
//!
//! The executor also re-requests Jupiter on `BlockhashExpired`, so this refresh
//! is only needed to extend a still-valid-but-aging blockhash before submission
//! — not to recover from a hard expiry.

use solana_sdk::{
    hash::Hash,
    message::VersionedMessage,
    transaction::VersionedTransaction,
};

/// Refresh a V0 message's `recent_blockhash` to `new_blockhash` by cloning the
/// message and swapping the single public field.
///
/// Returns the updated [`VersionedMessage::V0`]; the caller re-signs the
/// message hash and replaces the transaction's signatures. No RPC calls, no
/// ALT fetch, no recompilation — the on-chain structure is byte-for-byte
/// preserved apart from the blockhash.
///
/// Returns an error for legacy messages (legacy messages are refreshed inline
/// at their call sites by setting `recent_blockhash` directly).
pub fn refresh_v0_blockhash(
    versioned_tx: &VersionedTransaction,
    new_blockhash: Hash,
) -> Result<VersionedMessage, String> {
    match &versioned_tx.message {
        VersionedMessage::V0(v0_msg) => {
            let mut refreshed = v0_msg.clone();
            refreshed.recent_blockhash = new_blockhash;
            Ok(VersionedMessage::V0(refreshed))
        }
        VersionedMessage::Legacy(_) => {
            Err("Cannot refresh a legacy message via the V0 path".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::hash,
        message::v0::Message as V0Message,
        pubkey::Pubkey,
    };
    use solana_system_interface::instruction as system_instruction;
    use std::str::FromStr;

    #[test]
    fn refresh_swaps_only_the_blockhash() {
        let payer = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer, &recipient, 1_000);
        let blockhash_a = hash(&[1u8; 32]);
        let v0 = V0Message::try_compile(&payer, &[ix], &[], blockhash_a).unwrap();
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::V0(v0),
        };

        let blockhash_b = hash(&[2u8; 32]);
        let refreshed = refresh_v0_blockhash(&tx, blockhash_b).unwrap();

        match refreshed {
            VersionedMessage::V0(msg) => {
                assert_eq!(msg.recent_blockhash, blockhash_b);
                // Everything else preserved.
                assert_eq!(msg.header, tx.message.header().clone());
                assert_eq!(
                    msg.account_keys,
                    match &tx.message {
                        VersionedMessage::V0(m) => m.account_keys.clone(),
                        _ => unreachable!(),
                    }
                );
            }
            _ => panic!("expected V0"),
        }
    }

    #[test]
    fn refresh_rejects_legacy_message() {
        // A VersionedTransaction wrapping a legacy message cannot be refreshed
        // through the V0 path.
        let payer = Pubkey::new_unique();
        let legacy_msg = solana_sdk::message::Message::new(&[], Some(&payer));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(legacy_msg),
        };
        assert!(refresh_v0_blockhash(&tx, hash(&[9u8; 32])).is_err());
    }
}
