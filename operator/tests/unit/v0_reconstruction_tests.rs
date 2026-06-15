use chimera_operator::engine::v0_reconstruction::{extract_v0_components, reconstruct_v0_message_with_blockhash};
use solana_sdk::{
    hash::Hash,
    instruction::CompiledInstruction,
    message::{
        v0::{self, MessageAddressTableLookup},
        MessageHeader, VersionedMessage,
    },
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use std::sync::Arc;

#[test]
fn test_extract_v0_components_success() {
    let payer = Pubkey::new_unique();
    let account2 = Pubkey::new_unique();
    let program_id = Pubkey::new_unique();
    let alt_key = Pubkey::new_unique();

    let header = MessageHeader {
        num_required_signatures: 1,
        num_readonly_signed_accounts: 0,
        num_readonly_unsigned_accounts: 1,
    };

    let static_keys = vec![payer, account2, program_id];

    let instructions = vec![CompiledInstruction {
        program_id_index: 2,
        accounts: vec![0, 1],
        data: vec![1, 2, 3],
    }];

    let address_table_lookups = vec![MessageAddressTableLookup {
        account_key: alt_key,
        writable_indexes: vec![0],
        readonly_indexes: vec![1],
    }];

    let msg = v0::Message {
        header,
        account_keys: static_keys.clone(),
        recent_blockhash: Hash::default(),
        instructions,
        address_table_lookups,
    };

    let components_res = extract_v0_components(&msg);
    assert!(components_res.is_ok());

    let components = components_res.unwrap();
    assert_eq!(components.payer, payer);
    assert_eq!(components.static_account_keys, static_keys);
    assert_eq!(components.instructions.len(), 1);
    assert_eq!(components.address_table_lookups.len(), 1);
    assert_eq!(components.address_table_lookups[0].account_key, alt_key);
}

#[tokio::test]
async fn test_reconstruct_legacy_fails() {
    let payer = Pubkey::new_unique();
    let message = VersionedMessage::Legacy(solana_sdk::message::Message::new(&[], Some(&payer)));
    let tx = VersionedTransaction {
        signatures: vec![],
        message,
    };
    let new_blockhash = Hash::new_unique();
    
    // We can pass a dummy RpcClient since it fails early on the Legacy check before doing RPC
    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new("http://localhost:8899".to_string()));

    let res = reconstruct_v0_message_with_blockhash(&tx, new_blockhash, &rpc_client).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "Cannot reconstruct legacy message as V0");
}
