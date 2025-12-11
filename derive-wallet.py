#!/usr/bin/env python3
"""
Derive Solana wallet address from mnemonic seed phrase.
"""

import sys

try:
    from mnemonic import Mnemonic
    from bip_utils import Bip39SeedGenerator, Bip44, Bip44Coins, Bip44Changes
    import base58
except ImportError:
    print("Installing required packages...")
    import subprocess
    subprocess.check_call([sys.executable, '-m', 'pip', 'install', 'mnemonic', 'bip-utils', 'base58', '--quiet'])
    from mnemonic import Mnemonic
    from bip_utils import Bip39SeedGenerator, Bip44, Bip44Coins, Bip44Changes
    import base58

def derive_wallet(mnemonic_phrase: str) -> tuple[str, str]:
    """
    Derive Solana wallet address and private key from mnemonic.
    
    Returns:
        (address, private_key_hex)
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
    private_key_hex = private_key.hex()
    
    return address, private_key_hex

if __name__ == "__main__":
    mnemonic = "tower squirrel silly adult derive case behave crisp ketchup other topic tray"
    address, private_key = derive_wallet(mnemonic)
    
    print(f"Wallet Address: {address}")
    print(f"Private Key (hex): {private_key}")
    print()
    print(f"Explorer: https://explorer.solana.com/address/{address}")
    print(f"Solscan: https://solscan.io/account/{address}")
