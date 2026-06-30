//! Tests for the V0 blockhash refresh (F10).
//!
//! Verifies the refresh is a direct public-field swap on a clone — no ALT
//! fetch, no recompilation, no RPCs — preserving every field except the
//! blockhash.

use chimera_operator::engine::v0_reconstruction::refresh_v0_blockhash;
use solana_sdk::{
    hash::hash,
    message::{v0::Message as V0Message, VersionedMessage},
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use solana_system_interface::instruction as system_instruction;

#[test]
fn test_refresh_swaps_blockhash_only() {
    let payer = Pubkey::new_unique();
    let recipient = Pubkey::new_unique();
    let ix = system_instruction::transfer(&payer, &recipient, 1_000);

    let blockhash_a = hash(&[1u8; 32]);
    let v0 = V0Message::try_compile(&payer, &[ix], &[], blockhash_a).unwrap();
    let tx = VersionedTransaction {
        signatures: vec![],
        message: VersionedMessage::V0(v0),
    };

    let original_keys = match &tx.message {
        VersionedMessage::V0(m) => m.account_keys.clone(),
        _ => unreachable!(),
    };
    let original_header = *tx.message.header();

    let blockhash_b = hash(&[2u8; 32]);
    let refreshed = refresh_v0_blockhash(&tx, blockhash_b).expect("V0 refresh succeeds");

    match refreshed {
        VersionedMessage::V0(msg) => {
            assert_eq!(msg.recent_blockhash, blockhash_b);
            assert_ne!(msg.recent_blockhash, blockhash_a);
            // Everything else preserved.
            assert_eq!(msg.header, original_header);
            assert_eq!(msg.account_keys, original_keys);
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
    assert!(res.is_err());
}
