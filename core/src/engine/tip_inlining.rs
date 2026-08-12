//! Inline a Jito tip into a swap transaction.
//!
//! The previous design sent a *separate* tip transaction as the first element
//! of a Jito bundle (`[tip_tx, swap_tx]`). Jito bundles are atomic, so that is
//! correct, but D3 prefers the tip to live *inside* the swap transaction itself:
//! one signature, all-or-nothing at the transaction level, and it removes the
//! two-tx bundle.
//!
//! Jupiter's self-sign Swap API (`/swap`, `/swap/v2/build`) has **no native
//! Jito-tip parameter** (its `feeAccount`/`platformFeeBps` is a percentage
//! integrator cut, not a fixed validator tip). Jupiter's own guidance for
//! self-sign swaps is to append a transfer instruction on top of the returned
//! transaction — the standard `web3.js .add(instruction)` pattern.
//!
//! ## Scope / safety
//! Inlining is implemented for **legacy** messages only. A legacy message's
//! accounts are all static (`account_keys`), so decompiling compiled
//! instructions back into `Instruction`s (resolving account indices and
//! deriving signer/writable flags from the message header) is **deterministic**
//! and round-trip exact. V0 messages use Address Lookup Tables; inlining a tip
//! there would require fetching and resolving ALTs (the fragile path removed in
//! P1-7), so V0 keeps the separate-tip-bundle (still atomic at the bundle level).
//!
//! 🛡️ safety: this reconstructs and re-signs the swap transaction. Must be
//! validated on devnet before mainnet.

use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    transaction::Transaction,
};
use thiserror::Error;

/// Errors that can occur while inlining a tip.
#[derive(Debug, Error)]
pub enum TipInlineError {
    #[error("invalid account index {index} (account_keys has {len})")]
    InvalidAccountIndex { index: usize, len: usize },
    #[error("empty message: no account keys")]
    EmptyAccountKeys,
    #[error("message header violates Solana account layout invariants")]
    InvalidHeader,
    #[error("swap transaction carries a pre-existing signature that would be dropped")]
    PreExistingSignature,
    #[error("recompiled message signer set differs from the original message")]
    SignerSetChanged,
    #[error("recompiled message changes an account's writable flag")]
    WritabilityChanged,
}

/// Decompile a legacy [`Message`] back into its constituent [`Instruction`]s.
///
/// Account indices are resolved against `account_keys`; signer/writable flags
/// are derived from the message header (the standard Solana account layout):
///   - indices `[0, num_required_signatures)` are signers;
///   - among signers, `[0, num_signers - num_readonly_signed)` are writable;
///   - among non-signers, `[num_signers, N - num_readonly_unsigned)` are writable.
///
/// This is deterministic for legacy messages (no ALTs) and round-trip exact for
/// the *instruction list*; header-only signers (not referenced by any
/// instruction) are NOT representable and are validated against in
/// [`inline_jito_tip`].
pub fn decompile_legacy_message(message: &Message) -> Result<Vec<Instruction>, TipInlineError> {
    let account_keys = &message.account_keys;
    if account_keys.is_empty() {
        return Err(TipInlineError::EmptyAccountKeys);
    }

    let header = &message.header;
    let num_signers = header.num_required_signatures as usize;
    let num_readonly_signed = header.num_readonly_signed_accounts as usize;
    let num_readonly_unsigned = header.num_readonly_unsigned_accounts as usize;
    let num_accounts = account_keys.len();

    // Validate the header against the actual account layout BEFORE deriving
    // metas: `saturating_sub` would silently clamp an inconsistent header
    // (e.g. more readonly-signers than signers) into wrong signer/writable
    // flags, and recompilation would happily ship a transaction with wrong
    // account semantics.
    if num_signers > num_accounts
        || num_readonly_signed > num_signers
        || num_readonly_unsigned > num_accounts.saturating_sub(num_signers)
    {
        return Err(TipInlineError::InvalidHeader);
    }

    let meta_for = |idx: u8| -> Result<AccountMeta, TipInlineError> {
        let i = idx as usize;
        let pubkey = *account_keys.get(i).ok_or(TipInlineError::InvalidAccountIndex {
            index: i,
            len: num_accounts,
        })?;
        let is_signer = i < num_signers;
        let is_writable = if i < num_signers {
            i < num_signers.saturating_sub(num_readonly_signed)
        } else {
            i < num_accounts.saturating_sub(num_readonly_unsigned)
        };
        Ok(AccountMeta {
            pubkey,
            is_signer,
            is_writable,
        })
    };

    let mut out = Vec::with_capacity(message.instructions.len());
    for compiled in &message.instructions {
        let program_id = *account_keys.get(compiled.program_id_index as usize).ok_or(
            TipInlineError::InvalidAccountIndex {
                index: compiled.program_id_index as usize,
                len: num_accounts,
            },
        )?;
        let mut accounts = Vec::with_capacity(compiled.accounts.len());
        for &idx in &compiled.accounts {
            accounts.push(meta_for(idx)?);
        }
        out.push(Instruction {
            program_id,
            accounts,
            data: compiled.data.clone(),
        });
    }
    Ok(out)
}

/// Inline a Jito tip (a System `transfer` to `tip_account`) as the **last**
/// instruction of a legacy swap transaction, returning a new unsigned
/// transaction ready to be signed by the caller.
///
/// The tip is appended last so the swap logic executes first; atomicity at the
/// transaction level guarantees the tip is only paid if the whole tx lands.
///
/// # Round-trip safety checks
/// Decompilation only emits accounts referenced by instructions, so header-only
/// signers (e.g. multisig cosigners) cannot be preserved by recompilation, and
/// a pre-existing signature on `swap_tx` would be silently dropped by the
/// caller's re-sign. Both are rejected with a typed [`TipInlineError`], as is
/// any change to a shared account's writable flag (programs may branch on
/// writability).
pub fn inline_jito_tip(
    swap_tx: &Transaction,
    payer: &Pubkey,
    tip_account: &Pubkey,
    tip_lamports: u64,
    blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction, TipInlineError> {
    // Reject transactions that already carry a signature: `inline_and_serialize_tip`
    // re-signs with the wallet keypair only, so any additional required signature
    // (e.g. a cosigner's) would be silently dropped — the swap could land without
    // the cosigner's approval.
    if swap_tx
        .signatures
        .iter()
        .any(|s| *s != solana_sdk::signature::Signature::default())
    {
        return Err(TipInlineError::PreExistingSignature);
    }

    let mut instructions = decompile_legacy_message(&swap_tx.message)?;

    // System program transfer: payer -> tip_account.
    let tip_ix = solana_system_interface::instruction::transfer(payer, tip_account, tip_lamports);
    instructions.push(tip_ix);

    // Recompile with the payer first (it is account_keys[0] / the fee payer).
    // `Transaction::new_with_payer` compiles the message (same path the builder
    // and tip-tx construction use); it leaves the tx unsigned for the caller.
    let mut tx = Transaction::new_with_payer(&instructions, Some(payer));
    // Stamp the real blockhash before signing (new_with_payer compiles against a
    // default blockhash — same pattern as the builder / tip-tx construction).
    tx.message.recent_blockhash = blockhash;

    // Round-trip safety: verify the signer set survived recompilation. Header-only
    // signers are dropped by decompilation, silently shrinking the authorization
    // set of the rebuilt transaction vs the signed swap.
    let original_signers: std::collections::HashSet<&Pubkey> = swap_tx
        .message
        .account_keys
        .iter()
        .take(swap_tx.message.header.num_required_signatures as usize)
        .collect();
    let rebuilt_signers: std::collections::HashSet<&Pubkey> = tx
        .message
        .account_keys
        .iter()
        .take(tx.message.header.num_required_signatures as usize)
        .collect();
    if original_signers != rebuilt_signers {
        return Err(TipInlineError::SignerSetChanged);
    }

    // Writability can change for accounts shared between the swap and the tip
    // transfer (e.g. a read-only payer becomes writable because the tip
    // transfer writes to it). Programs that branch on account writability
    // would execute the original swap instructions with a different view.
    let original_writable = message_writable_flags(&swap_tx.message);
    let rebuilt_writable = message_writable_flags(&tx.message);
    for (key, writable) in &original_writable {
        if let Some(rebuilt) = rebuilt_writable.get(key) {
            if rebuilt != writable {
                return Err(TipInlineError::WritabilityChanged);
            }
        }
    }

    Ok(tx)
}

/// Per-account writable flags derived from a legacy message's header layout.
fn message_writable_flags(message: &Message) -> std::collections::HashMap<Pubkey, bool> {
    let num_signers = message.header.num_required_signatures as usize;
    let num_readonly_signed = message.header.num_readonly_signed_accounts as usize;
    let num_readonly_unsigned = message.header.num_readonly_unsigned_accounts as usize;
    let num_accounts = message.account_keys.len();
    message
        .account_keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let writable = if i < num_signers {
                i < num_signers.saturating_sub(num_readonly_signed)
            } else {
                i < num_accounts.saturating_sub(num_readonly_unsigned)
            };
            (*key, writable)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::{hash, Hash},
        message::Message,
        pubkey::Pubkey,
    };
    use solana_system_interface::instruction as system_instruction;

    fn system_program_id() -> Pubkey {
        // Derive the System program id portably (crate path varies by version).
        system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1).program_id
    }

    fn build_legacy(payer: &Pubkey, recipient: &Pubkey) -> Transaction {
        // A swap-like legacy message: a memo + a SOL transfer, payer-signed.
        let memo_prog = Pubkey::new_unique();
        let memo_ix = Instruction {
            program_id: memo_prog,
            accounts: vec![AccountMeta {
                pubkey: *payer,
                is_signer: true,
                is_writable: true,
            }],
            data: vec![0xDE, 0xAD],
        };
        let transfer_ix = system_instruction::transfer(payer, recipient, 5_000);
        Transaction::new_with_payer(&[memo_ix, transfer_ix], Some(payer))
    }

    #[test]
    fn decompile_is_round_trip_exact() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tx = build_legacy(&payer, &recipient);

        let ixs = decompile_legacy_message(&tx.message).expect("decompile");

        // Same instruction count, same program ids, same data, same accounts.
        assert_eq!(ixs.len(), tx.message.instructions.len());
        for (ix, compiled) in ixs.iter().zip(tx.message.instructions.iter()) {
            assert_eq!(ix.program_id, tx.message.account_keys[compiled.program_id_index as usize]);
            assert_eq!(ix.data, compiled.data);
            assert_eq!(ix.accounts.len(), compiled.accounts.len());
            for (meta, &idx) in ix.accounts.iter().zip(compiled.accounts.iter()) {
                assert_eq!(meta.pubkey, tx.message.account_keys[idx as usize]);
            }
        }
    }

    #[test]
    fn inline_appends_tip_and_preserves_originals() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tip_account = Pubkey::new_unique();
        let blockhash = hash(&[7u8; 32]);

        let swap_tx = build_legacy(&payer, &recipient);
        let original_ixs = decompile_legacy_message(&swap_tx.message).unwrap();

        let inlined = inline_jito_tip(&swap_tx, &payer, &tip_account, 1_234_567, blockhash)
            .expect("inline");

        // The inlined message has one more instruction (the tip), appended last.
        let inlined_ixs = decompile_legacy_message(&inlined.message).unwrap();
        assert_eq!(inlined_ixs.len(), original_ixs.len() + 1);
        // The originals are preserved verbatim.
        for (a, b) in inlined_ixs.iter().take(original_ixs.len()).zip(original_ixs.iter()) {
            assert_eq!(a.program_id, b.program_id);
            assert_eq!(a.data, b.data);
            assert_eq!(a.accounts, b.accounts);
        }
        // The last instruction is a System transfer of exactly tip_lamports to the tip account.
        let tip = inlined_ixs.last().unwrap();
        assert_eq!(tip.program_id, system_program_id());
        assert!(tip.accounts.iter().any(|m| m.pubkey == tip_account && m.is_writable));
        assert!(tip.accounts.iter().any(|m| m.pubkey == payer && m.is_signer));
        // System Transfer instruction data: [2 (discriminant), u64 LE amount, u32 _maybe].
        assert_eq!(tip.data.first().copied(), Some(2)); // SystemInstruction::Transfer discriminant
        let amt = u64::from_le_bytes(tip.data[4..12].try_into().unwrap());
        assert_eq!(amt, 1_234_567);

        // Blockhash was stamped, payer is the fee payer.
        assert_eq!(inlined.message.recent_blockhash, blockhash);
        assert_eq!(inlined.message.account_keys[0], payer);
        // The inlined tx is unsigned until the caller signs it.
        assert!(inlined.signatures.iter().all(|s| s == &solana_sdk::signature::Signature::default()));
    }

    #[test]
    fn decompile_rejects_empty_message() {
        let empty = Message {
            header: solana_sdk::message::MessageHeader {
                num_required_signatures: 0,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: vec![],
            recent_blockhash: Hash::default(),
            instructions: vec![],
        };
        assert!(decompile_legacy_message(&empty).is_err());
    }

    #[test]
    fn decompile_rejects_inconsistent_header() {
        // num_required_signatures (2) exceeds the account count (1) → the
        // header violates the Solana account-layout invariants.
        let msg = Message {
            header: solana_sdk::message::MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: vec![Pubkey::new_unique()],
            recent_blockhash: Hash::default(),
            instructions: vec![],
        };
        assert!(matches!(
            decompile_legacy_message(&msg),
            Err(TipInlineError::InvalidHeader)
        ));
    }

    #[test]
    fn inline_rejects_preexisting_signature() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tip_account = Pubkey::new_unique();
        let mut tx = build_legacy(&payer, &recipient);
        // A pre-existing (non-default) signature must be rejected.
        tx.signatures = vec![solana_sdk::signature::Signature::new_unique()];
        assert!(matches!(
            inline_jito_tip(&tx, &payer, &tip_account, 1_000, hash(&[3u8; 32])),
            Err(TipInlineError::PreExistingSignature)
        ));
    }

    #[test]
    fn inline_rejects_header_only_signer_dropped() {
        let payer = Pubkey::new_unique();
        let extra = Pubkey::new_unique(); // header-only signer, no instruction refs it
        let recipient = Pubkey::new_unique();
        let system = system_program_id();
        let tip_account = Pubkey::new_unique();

        // Manual legacy message: payer + extra are both signers, but `extra` is
        // NOT referenced by any instruction, so decompilation drops it and the
        // rebuilt signer set shrinks → SignerSetChanged.
        let msg = Message {
            header: solana_sdk::message::MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 1, // `extra` is a read-only signer
                num_readonly_unsigned_accounts: 1, // system read-only
            },
            account_keys: vec![payer, extra, recipient, system],
            recent_blockhash: Hash::default(),
            instructions: vec![
                solana_sdk::message::compiled_instruction::CompiledInstruction {
                    program_id_index: 3,
                    accounts: vec![0, 2], // payer, recipient — NOT `extra`
                    data: vec![2, 0, 0, 0, 0, 232, 3, 0, 0, 0, 0, 0],
                },
            ],
        };
        let tx = Transaction {
            signatures: vec![],
            message: msg,
        };

        let err = inline_jito_tip(&tx, &payer, &tip_account, 1_000, hash(&[4u8; 32]));
        assert!(
            matches!(err, Err(TipInlineError::SignerSetChanged)),
            "expected SignerSetChanged, got {:?}",
            err
        );
    }

    #[test]
    fn inline_rejects_writability_change() {
        // Original message has the payer as a READ-ONLY signer; the appended tip
        // transfer writes to the payer, so the rebuilt message flips it writable.
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tip_account = Pubkey::new_unique();
        // Build a normal payer-signed transfer, then mark the payer as a
        // read-only signer via the header.
        let transfer = system_instruction::transfer(&payer, &recipient, 1_000);
        let mut tx = Transaction::new_with_payer(&[transfer], Some(&payer));
        tx.message.header.num_readonly_signed_accounts = 1;

        let err = inline_jito_tip(&tx, &payer, &tip_account, 1_000, hash(&[5u8; 32]));
        assert!(
            matches!(err, Err(TipInlineError::WritabilityChanged)),
            "expected WritabilityChanged, got {:?}",
            err
        );
    }
}
