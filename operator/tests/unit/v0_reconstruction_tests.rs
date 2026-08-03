//! Tests for the V0 blockhash refresh (F10).
//!
//! Verifies the refresh is a direct public-field swap on a clone — no ALT
//! fetch, no recompilation, no RPCs — preserving every field except the
//! blockhash.

use chimera_operator::engine::v0_reconstruction::refresh_v0_blockhash;
use solana_sdk::{
    hash::hash,
    message::{
        v0::Message as V0Message, AddressLookupTableAccount, VersionedMessage,
    },
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use solana_system_interface::instruction as system_instruction;

fn fixture_tx_with_lookup() -> VersionedTransaction {
    let payer = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();
    let ix = system_instruction::transfer(&payer, &recipient, 1_000);

    // A real address-table lookup so the fixture exercises the V0
    // address_table_lookups path (the old implementation had a 280-line
    // ALT-fetch/recompile path that must never be re-introduced).
    let lookup = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: vec![Pubkey::new_unique(), Pubkey::new_unique()],
    };
    let blockhash_a = hash(&[1u8; 32]);
    let v0 = V0Message::try_compile(&payer, &[ix], &[lookup], blockhash_a).unwrap();
    VersionedTransaction {
        signatures: vec![],
        message: VersionedMessage::V0(v0),
    }
}

#[test]
fn test_refresh_swaps_blockhash_only() {
    let tx = fixture_tx_with_lookup();

    let original_keys = match &tx.message {
        VersionedMessage::V0(m) => m.account_keys.clone(),
        _ => unreachable!(),
    };
    let original_header = *tx.message.header();
    let original_instructions = match &tx.message {
        VersionedMessage::V0(m) => m.instructions.clone(),
        _ => unreachable!(),
    };
    let original_lookups = match &tx.message {
        VersionedMessage::V0(m) => m.address_table_lookups.clone(),
        _ => unreachable!(),
    };
    let original_blockhash = match &tx.message {
        VersionedMessage::V0(m) => m.recent_blockhash,
        _ => unreachable!(),
    };

    let blockhash_b = hash(&[2u8; 32]);
    let refreshed = refresh_v0_blockhash(&tx, blockhash_b).expect("V0 refresh succeeds");

    match refreshed {
        VersionedMessage::V0(msg) => {
            assert_eq!(msg.recent_blockhash, blockhash_b);
            assert_ne!(msg.recent_blockhash, original_blockhash);
            // Everything else preserved byte-for-byte.
            assert_eq!(msg.header, original_header);
            assert_eq!(msg.account_keys, original_keys);
            assert_eq!(msg.instructions, original_instructions);
            assert_eq!(msg.address_table_lookups, original_lookups);
        }
        _ => panic!("expected V0"),
    }
}

#[test]
fn test_refresh_rejects_legacy_message() {
    let payer = Pubkey::new_unique();
    let legacy_msg = solana_sdk::message::Message::new(&[], Some(&payer));
    let tx = VersionedTransaction {
        signatures: vec![],
        message: VersionedMessage::Legacy(legacy_msg),
    };
    let res = refresh_v0_blockhash(&tx, hash(&[9u8; 32]));
    assert_eq!(
        res.unwrap_err().to_string(),
        "Cannot refresh a legacy message via the V0 path",
        "legacy messages must be rejected with the documented error"
    );
}

// NOTE: the in-module tests in v0_reconstruction.rs
// (refresh_swaps_only_the_blockhash / refresh_rejects_legacy_message) cover
// the same contract; keep both in sync when changing the preservation
// guarantees.
