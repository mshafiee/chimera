//! Shared keypair normalization helpers.
//!
//! `normalize_to_64_bytes` accepts the three formats produced by common Solana
//! tooling and returns a 64-byte (32 secret + 32 public) keypair buffer wrapped
//! in `Zeroizing` so the secret bytes are wiped when the handle is dropped:
//!
//! - **Solana CLI JSON byte-array** — e.g. the contents of a `~/.config/solana/id.json`
//!   file. Detected by a leading `[`.
//! - **Base58** — what `solana-keygen pubkey` and most browser wallets expose.
//!   A 64-byte Ed25519 keypair encodes to 86-88 base58 characters (the length
//!   varies because base58 has no fixed width; a leading zero byte in the
//!   keypair produces a shorter encoding).
//! - **Hex** — the canonical storage format inside the encrypted vault
//!   (`VaultSecrets::wallet_private_key`). Always 128 hex chars for 64 bytes.
//!
//! Both the `import_keypair` tool and `load_wallet_keypair` route through this
//! helper so the on-disk format stays unambiguous (hex) regardless of input.
//!
//! All intermediate buffers (the decoded `Vec<u8>` and the returned array) are
//! wrapped in `Zeroizing` so the raw Ed25519 secret is wiped from memory on
//! drop, not just the textual input.

use crate::error::{AppError, AppResult};
use zeroize::Zeroizing;

/// Bitcoin/Solana base58 alphabet (excludes `0`, `O`, `I`, `l`).
const BASE58_ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Maximum base58 length for a 64-byte buffer. Empirically 64 random bytes
/// encode to 86-88 chars (the high end, 88, is reached when no leading zero
/// byte is present). We accept anything up to 88 and let the post-decode
/// `bytes.len() != 64` check reject wrong-length inputs.
const BASE58_MAX_LEN: usize = 88;

/// Returns true if every byte of `s` belongs to the base58 alphabet.
fn is_base58(s: &str) -> bool {
    s.bytes().all(|b| BASE58_ALPHABET.contains(&b))
}

/// Returns true if every byte of `s` is a hex digit (`0-9`, `a-f`, `A-F`).
fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Normalize a keypair provided in any supported format into a 64-byte array.
///
/// The input is trimmed of surrounding whitespace before format detection.
/// The detection order is intentional:
/// 1. JSON (`[` prefix) wins outright — it cannot collide with the other two.
/// 2. Hex (exactly 128 chars) is checked before base58 so that a 128-char hex
///    string whose characters happen to all fall inside the base58 alphabet
///    (e.g. containing no `0`) is still decoded as hex, not misclassified.
/// 3. Base58 (≤ 88 chars, all in alphabet). A 64-byte Ed25519 keypair encodes
///    to 86-88 base58 chars; the post-decode length check below rejects
///    anything that doesn't decode to exactly 64 bytes.
///
/// Anything else is rejected with a message listing accepted formats.
///
/// The returned array is wrapped in [`Zeroizing`] so callers don't have to
/// remember to wipe the secret bytes manually — they are cleared on drop.
pub fn normalize_to_64_bytes(input: &str) -> AppResult<Zeroizing<[u8; 64]>> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(AppError::Validation("Keypair input is empty".to_string()));
    }

    // Intermediate buffer is Zeroizing so the decoded secret is wiped even if
    // the length check below fails and we return early.
    let bytes: Zeroizing<Vec<u8>> = if trimmed.starts_with('[') {
        // 1. JSON byte-array (Solana CLI id.json format).
        Zeroizing::new(serde_json::from_str::<Vec<u8>>(trimmed).map_err(|e| {
            AppError::Validation(format!(
                "Keypair looks like a JSON array but failed to parse: {}. \
                     Expected a Solana CLI id.json-style array of 64 unsigned bytes.",
                e
            ))
        })?)
    } else if trimmed.len() == 128 && is_hex(trimmed) {
        // 2. Hex (128 chars, all hex chars). Checked before base58 so a hex
        //    string that also happens to be valid base58 alphabet is decoded
        //    as hex.
        Zeroizing::new(
            hex::decode(trimmed)
                .map_err(|e| AppError::Validation(format!("Invalid hex keypair: {}", e)))?,
        )
    } else if trimmed.len() <= BASE58_MAX_LEN && is_base58(trimmed) {
        // 3. Base58 (≤ 88 chars, all in alphabet). A 64-byte Ed25519 keypair
        //    encodes to 86-88 chars depending on leading zero bytes.
        Zeroizing::new(
            bs58::decode(trimmed)
                .into_vec()
                .map_err(|e| AppError::Validation(format!("Invalid base58 keypair: {}", e)))?,
        )
    } else {
        return Err(AppError::Validation(format!(
            "Unrecognized keypair format (len={}). \
             Accepted formats: Solana CLI JSON byte-array (starts with '['), \
             base58 (86-88 chars), or hex (128 chars).",
            trimmed.len()
        )));
    };

    if bytes.len() != 64 {
        return Err(AppError::Validation(format!(
            "Decoded keypair is {} bytes — expected exactly 64 \
             (32-byte Ed25519 secret + 32-byte public key).",
            bytes.len()
        )));
    }

    let mut arr = Zeroizing::new([0u8; 64]);
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::{Keypair, Signer};

    /// A valid hex keypair must round-trip back to the same pubkey.
    #[test]
    fn round_trip_hex() {
        let kp = Keypair::new();
        let hex_str = hex::encode(kp.to_bytes());
        let bytes = normalize_to_64_bytes(&hex_str).expect("hex decode");
        let kp2 = Keypair::try_from(bytes.as_slice()).expect("keypair");
        assert_eq!(kp.pubkey(), kp2.pubkey());
    }

    #[test]
    fn round_trip_base58() {
        let kp = Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        // base58 of 64 bytes is 86-88 chars depending on leading zero bytes.
        // Do NOT assert a fixed length — it flakes (~1 in 24k keypairs is 86).
        let bytes = normalize_to_64_bytes(&b58).expect("base58 decode");
        let kp2 = Keypair::try_from(bytes.as_slice()).expect("keypair");
        assert_eq!(kp.pubkey(), kp2.pubkey());
    }

    /// Regression test for the base58 length edge case: a 64-byte buffer with
    /// a leading zero byte can encode to exactly 86 base58 chars (when the
    /// remaining 63 bytes represent a number < 58^85). The old `len == 87 ||
    /// len == 88` guard rejected this valid encoding.
    #[test]
    fn base58_86_chars_accepted() {
        // Pre-computed: this hex decodes to 64 bytes whose base58 encoding is
        // exactly 86 chars. NOT a valid Ed25519 keypair (pubkey won't match
        // secret), but the format detector should still accept it and produce
        // the correct 64 bytes. Keypair::try_from is expected to fail.
        let hex_86 = "0002b78138113eb6c8d94d5b857732f332ff14329145008de5b57189646d782a1a9213eab6fbacf612bf408ed656c159ee26cd7a795f3001a5666956a00f2cb4";
        let b58_86 = "1iQ8AyJpfeCPBcZui5mcfbGbcvpZyxhNUBdtCkiKNRTjvH5WE4VHab1e8XwMMPJKZRJFKKy225yCXPpDKN64Lb";
        assert_eq!(b58_86.len(), 86, "precomputed value must be 86 chars");

        // Both the hex and base58 forms must decode to the same 64 bytes.
        let from_hex = normalize_to_64_bytes(hex_86).expect("hex form should decode");
        let from_b58 = normalize_to_64_bytes(b58_86).expect("86-char base58 must be accepted");
        assert_eq!(*from_hex, *from_b58);
        assert_eq!(from_hex.len(), 64);
    }

    #[test]
    fn round_trip_json() {
        let kp = Keypair::new();
        let json = format!("{:?}", kp.to_bytes().to_vec());
        let bytes = normalize_to_64_bytes(&json).expect("json decode");
        let kp2 = Keypair::try_from(bytes.as_slice()).expect("keypair");
        assert_eq!(kp.pubkey(), kp2.pubkey());
    }

    #[test]
    fn json_with_whitespace() {
        let kp = Keypair::new();
        let bytes = kp.to_bytes();
        // Mirror the real Solana CLI id.json formatting.
        let json = format!(
            "[\n  {}\n]\n",
            bytes
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(",\n  ")
        );
        let decoded = normalize_to_64_bytes(&json).expect("json with newlines");
        assert_eq!(*decoded, bytes);
    }

    #[test]
    fn rejects_empty() {
        assert!(normalize_to_64_bytes("").is_err());
        assert!(normalize_to_64_bytes("   \n\t").is_err());
    }

    #[test]
    fn rejects_short_json() {
        let kp = Keypair::new();
        let mut short = kp.to_bytes().to_vec();
        short.pop(); // 63 bytes
        let json = format!("{:?}", short);
        let err = normalize_to_64_bytes(&json).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[test]
    fn rejects_negative_json() {
        // serde_json(u8) will reject negative numbers outright.
        let json = "[-1, 2, 3]";
        assert!(normalize_to_64_bytes(json).is_err());
    }

    #[test]
    fn rejects_invalid_base58_char() {
        // '0' and 'l' and 'I' and 'O' are not in the base58 alphabet.
        let kp = Keypair::new();
        let mut b58 = bs58::encode(kp.to_bytes()).into_string();
        // Inject an invalid char while preserving length.
        b58.replace_range(0..1, "0");
        assert!(normalize_to_64_bytes(&b58).is_err());
    }

    #[test]
    fn rejects_garbage() {
        let err = normalize_to_64_bytes("not a keypair at all").unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }
}
