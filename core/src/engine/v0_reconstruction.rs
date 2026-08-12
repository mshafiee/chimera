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

/// Error refreshing a V0 message's blockhash.
///
/// Typed so callers can branch on the variant instead of string-matching:
/// passing a legacy message here is an API-contract violation, not a runtime
/// condition the caller can recover from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshBlockhashError {
    /// A legacy message was passed to the V0-only refresh path.
    LegacyMessageNotSupported,
}

impl std::fmt::Display for RefreshBlockhashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LegacyMessageNotSupported => {
                write!(f, "Cannot refresh a legacy message via the V0 path")
            }
        }
    }
}

impl std::error::Error for RefreshBlockhashError {}

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
///
/// NOTE: this is O(message size) per refresh (the message, including
/// `account_keys`, `instructions` and `address_table_lookups`, is cloned).
pub fn refresh_v0_blockhash(
    versioned_tx: &VersionedTransaction,
    new_blockhash: Hash,
) -> Result<VersionedMessage, RefreshBlockhashError> {
    match &versioned_tx.message {
        VersionedMessage::V0(v0_msg) => {
            let mut refreshed = v0_msg.clone();
            refreshed.recent_blockhash = new_blockhash;
            Ok(VersionedMessage::V0(refreshed))
        }
        VersionedMessage::Legacy(_) => Err(RefreshBlockhashError::LegacyMessageNotSupported),
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
                // Everything else preserved — including the compiled
                // instructions and ALT lookups, so a field-swap bug anywhere
                // in the refresh is caught, not just header/account_keys.
                assert_eq!(msg.header, tx.message.header().clone());
                assert_eq!(
                    msg.account_keys,
                    match &tx.message {
                        VersionedMessage::V0(m) => m.account_keys.clone(),
                        _ => unreachable!(),
                    }
                );
                assert_eq!(
                    msg.instructions,
                    match &tx.message {
                        VersionedMessage::V0(m) => m.instructions.clone(),
                        _ => unreachable!(),
                    }
                );
                assert_eq!(
                    msg.address_table_lookups,
                    match &tx.message {
                        VersionedMessage::V0(m) => m.address_table_lookups.clone(),
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

    #[test]
    fn refresh_blockhash_error_display_and_eq() {
        assert_eq!(
            RefreshBlockhashError::LegacyMessageNotSupported.to_string(),
            "Cannot refresh a legacy message via the V0 path"
        );
        // Derives PartialEq/Eq/Clone/Copy.
        assert_eq!(
            RefreshBlockhashError::LegacyMessageNotSupported,
            RefreshBlockhashError::LegacyMessageNotSupported
        );
    }
}
