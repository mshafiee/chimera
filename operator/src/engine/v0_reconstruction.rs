//! V0 Transaction Message Reconstruction
//!
//! This module provides utilities to reconstruct V0 (Address Lookup Table) messages
//! with updated blockhashes, allowing client-side blockhash updates for V0 transactions.

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::address_lookup_table::state::AddressLookupTable;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    instruction::CompiledInstruction,
    message::{
        v0::{self, MessageAddressTableLookup},
        VersionedMessage,
    },
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use std::sync::Arc;

/// Extract components from a V0 message for reconstruction
pub struct V0Components {
    /// Payer public key (first account)
    pub payer: Pubkey,
    /// All instructions from the message
    pub instructions: Vec<CompiledInstruction>,
    /// Static account keys (non-ALT accounts)
    pub static_account_keys: Vec<Pubkey>,
    /// Address lookup table lookups
    pub address_table_lookups: Vec<MessageAddressTableLookup>,
}

/// Extract all necessary components from a V0 message
pub fn extract_v0_components(
    v0_message: &v0::Message,
) -> Result<V0Components, String> {
    // V0 message fields are accessed directly, not via methods
    // Get payer (first account in account_keys)
    let payer = v0_message
        .account_keys
        .first()
        .ok_or_else(|| "V0 message has no account keys".to_string())?
        .clone();

    // Extract instructions (field, not method)
    let instructions = v0_message.instructions.clone();

    // Extract static account keys - these are the account_keys field
    // (static accounts come before ALT-resolved accounts)
    let static_account_keys = v0_message.account_keys.clone();

    // Extract address table lookups (field, not method)
    let address_table_lookups = v0_message.address_table_lookups.clone();

    Ok(V0Components {
        payer,
        instructions,
        static_account_keys,
        address_table_lookups,
    })
}

/// Fetch Address Lookup Table accounts from RPC
pub async fn fetch_address_lookup_tables(
    rpc_client: &Arc<RpcClient>,
    alt_keys: &[Pubkey],
) -> Result<Vec<AddressLookupTableAccount>, String> {
    let mut alt_accounts = Vec::new();

    for alt_key in alt_keys {
        // Fetch the ALT account data
        let account_data = rpc_client
            .get_account_data(alt_key)
            .await
            .map_err(|e| format!("Failed to fetch ALT account {}: {}", alt_key, e))?;

        // Deserialize the ALT account
        let address_lookup_table = AddressLookupTable::deserialize(&account_data)
            .map_err(|e| format!("Failed to deserialize ALT account {}: {}", alt_key, e))?;

        // Create AddressLookupTableAccount
        let alt_account = AddressLookupTableAccount {
            key: *alt_key,
            addresses: address_lookup_table.addresses.to_vec(),
        };

        alt_accounts.push(alt_account);
    }

    Ok(alt_accounts)
}

/// Reconstruct a V0 message with a new blockhash
///
/// This function extracts all components from the existing V0 message,
/// fetches the required Address Lookup Table accounts from RPC,
/// and reconstructs the message with the new blockhash.
pub async fn reconstruct_v0_message_with_blockhash(
    versioned_tx: &VersionedTransaction,
    new_blockhash: solana_sdk::hash::Hash,
    rpc_client: &Arc<RpcClient>,
) -> Result<VersionedMessage, String> {
    // Extract V0 message
    let v0_message = match &versioned_tx.message {
        VersionedMessage::V0(msg) => msg,
        VersionedMessage::Legacy(_) => {
            return Err("Cannot reconstruct legacy message as V0".to_string());
        }
    };

    // Extract components
    let components = extract_v0_components(v0_message)?;

    // Collect all ALT keys from address table lookups
    let alt_keys: Vec<Pubkey> = components
        .address_table_lookups
        .iter()
        .map(|lookup| lookup.account_key)
        .collect();

    // Fetch Address Lookup Table accounts
    let alt_accounts = fetch_address_lookup_tables(rpc_client, &alt_keys).await?;

    // Reconstruct the V0 message with new blockhash
    // We need to convert CompiledInstructions back to Instructions
    // However, since we're working with CompiledInstructions, we need to
    // use the message's instruction account indices to properly reconstruct.
    //
    // The challenge is that CompiledInstructions use account indices, and we need
    // to maintain the same account ordering. We'll use try_compile which handles
    // this automatically by taking the full account set.

    // For V0 messages, we need to provide:
    // 1. Payer pubkey
    // 2. Instructions (as CompiledInstructions with account indices)
    // 3. Address lookup table accounts
    // 4. New blockhash

    // However, try_compile expects Instructions, not CompiledInstructions.
    // We need to convert CompiledInstructions back to Instructions by resolving
    // account indices to pubkeys.

    // Build the full account list (static + resolved from ALTs)
    let mut all_accounts = components.static_account_keys.clone();

    // For each address table lookup, we need to resolve the accounts
    // But since we're reconstructing, we'll let try_compile handle the account resolution
    // by providing the ALT accounts.

    // The issue is that CompiledInstructions reference accounts by index, and we need
    // to maintain that mapping. Let's use a different approach: we'll create a new
    // message by compiling from the original structure.

    // Actually, the Solana SDK's v0::Message::try_compile expects Instructions (not CompiledInstructions),
    // so we need to convert. However, this is complex because we need to resolve account indices.

    // Alternative approach: Use the message's serialize/deserialize or try to update
    // the blockhash directly in the message structure if possible.

    // Let's try a simpler approach: create a new message by cloning the structure
    // and updating only the recent_blockhash field. But V0 messages don't expose
    // this directly.

    // Best approach: Use the message's account keys and instructions to rebuild.
    // We'll need to convert CompiledInstructions to Instructions by resolving indices.

    // For now, let's use a workaround: we'll try to use the message's account keys
    // and reconstruct using try_compile with the instructions converted.

    // Use the message's account_keys field to get all accounts in order
    // This includes static accounts + resolved ALT accounts in the correct order
    let all_account_keys: Vec<Pubkey> = v0_message.account_keys.clone();
    
    // Get message header for signer/writable determination (field, not method)
    let header = &v0_message.header;
    let num_required_signatures = header.num_required_signatures as usize;
    let num_readonly_signed_accounts = header.num_readonly_signed_accounts as usize;
    let num_readonly_unsigned_accounts = header.num_readonly_unsigned_accounts as usize;
    let num_writable_signed = num_required_signatures - num_readonly_signed_accounts;
    let num_static_accounts = components.static_account_keys.len();

    // Convert CompiledInstructions to Instructions by resolving account indices
    use solana_sdk::instruction::{Instruction, AccountMeta};
    let mut resolved_instructions = Vec::new();

    for compiled_ix in &components.instructions {
        // Resolve program ID
        let program_id_idx = compiled_ix.program_id_index as usize;
        let program_id = all_account_keys
            .get(program_id_idx)
            .ok_or_else(|| format!("Program ID index {} out of range", program_id_idx))?
            .clone();

        // Resolve account keys and determine metadata (writable/signer)
        let account_metas: Vec<AccountMeta> = compiled_ix
            .accounts
            .iter()
            .map(|&idx| {
                let account_key = all_account_keys
                    .get(idx as usize)
                    .ok_or_else(|| format!("Account index {} out of range", idx))?
                    .clone();
                
                // Determine if account is a signer (must be in first num_required_signatures)
                let is_signer = (idx as usize) < num_required_signatures;
                
                // Determine if account is writable based on message structure
                let is_writable = if (idx as usize) < num_static_accounts {
                    // Static account: check header structure
                    (idx as usize) < num_writable_signed || 
                    ((idx as usize) >= num_required_signatures && 
                     (idx as usize) < num_static_accounts - num_readonly_unsigned_accounts)
                } else {
                    // Account from ALT: check if it's in writable_indexes of any ALT lookup
                    let mut current_alt_start = num_static_accounts;
                    for alt_lookup in &components.address_table_lookups {
                        let alt_account = alt_accounts
                            .iter()
                            .find(|a| a.key == alt_lookup.account_key)
                            .ok_or_else(|| {
                                format!("ALT account {} not found", alt_lookup.account_key)
                            })?;
                        
                        let alt_size = alt_account.addresses.len();
                        if (idx as usize) >= current_alt_start && 
                           (idx as usize) < current_alt_start + alt_size {
                            // This account is from this ALT
                            let alt_index = (idx as usize) - current_alt_start;
                            // Check if this index is in writable_indexes
                            let is_writable = alt_lookup.writable_indexes.contains(&(alt_index as u8));
                            return Ok::<AccountMeta, String>(AccountMeta {
                                pubkey: account_key,
                                is_signer,
                                is_writable,
                            });
                        }
                        current_alt_start += alt_size;
                    }
                    // Default to writable if we can't determine (conservative)
                    true
                };
                
                Ok(AccountMeta {
                    pubkey: account_key,
                    is_signer,
                    is_writable,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let instruction = Instruction {
            program_id,
            accounts: account_metas,
            data: compiled_ix.data.clone(),
        };

        resolved_instructions.push(instruction);
    }

    // Reconstruct the V0 message with new blockhash
    let reconstructed_message = v0::Message::try_compile(
        &components.payer,
        &resolved_instructions,
        alt_accounts.as_slice(),
        new_blockhash,
    )
    .map_err(|e| format!("Failed to compile V0 message: {}", e))?;

    Ok(VersionedMessage::V0(reconstructed_message))
}
