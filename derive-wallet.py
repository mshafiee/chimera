#!/usr/bin/env python3
"""
Derive Solana wallet address from mnemonic seed phrase.
Reads the mnemonic from the MNEMONIC environment variable or prompts
interactively. Never prints the private key.
"""

import os
import sys

try:
    from bip_utils import Bip39SeedGenerator, Bip44, Bip44Coins, Bip44Changes
    import base58
except ImportError:
    print("Missing dependencies. Install with: pip install bip-utils base58", file=sys.stderr)
    sys.exit(1)


def derive_wallet(mnemonic_phrase: str) -> tuple[str, str]:
    """
    Derive Solana wallet address and keypair from mnemonic.

    Returns:
        (address, keypair_hex) where keypair_hex is the full 64-byte
        keypair (32-byte seed || 32-byte public key) usable by Solana tooling.
    """
    # Generate seed from mnemonic
    seed = Bip39SeedGenerator(mnemonic_phrase).Generate()

    # Derive Solana keypair (BIP44 path: m/44'/501'/0'/0')
    bip44_mst = Bip44.FromSeed(seed, Bip44Coins.SOLANA)
    bip44_acc = bip44_mst.Purpose().Coin().Account(0)
    bip44_chg = bip44_acc.Change(Bip44Changes.CHAIN_EXT)
    bip44_addr = bip44_chg.AddressIndex(0)

    # Get keys
    private_key = bip44_addr.PrivateKey().Raw().ToBytes()
    public_key = bip44_addr.PublicKey().RawUncompressed().ToBytes()

    # Solana uses first 32 bytes of public key
    solana_pubkey = public_key[:32]

    # Convert to base58 address
    address = base58.b58encode(solana_pubkey).decode('utf-8')
    # Solana tooling expects the 64-byte keypair: seed || pubkey
    keypair_hex = (private_key + solana_pubkey).hex()

    return address, keypair_hex


if __name__ == "__main__":
    mnemonic = os.environ.get("MNEMONIC", "").strip()
    if not mnemonic:
        try:
            import getpass
            mnemonic = getpass.getpass("Enter mnemonic seed phrase: ").strip()
        except Exception:
            print("Set the MNEMONIC environment variable or provide input interactively", file=sys.stderr)
            sys.exit(1)
    if not mnemonic:
        print("Empty mnemonic", file=sys.stderr)
        sys.exit(1)

    address, _keypair = derive_wallet(mnemonic)

    print(f"Wallet Address: {address}")
    print()
    print(f"Explorer: https://explorer.solana.com/address/{address}")
    print(f"Solscan: https://solscan.io/account/{address}")
