#!/usr/bin/env python3
"""
Helius Webhook Registration Tool for Chimera Trading System

This script registers wallets with Helius webhook monitoring for real-time
transaction detection and trading signal generation.
"""

import os
import sys
import json
import argparse
import requests
from typing import List, Dict, Optional
from datetime import datetime

# Configuration
HELIUS_API_KEY = os.getenv("HELIUS_API_KEY", "609cb910-17a5-4a76-9d1b-2ca9c42f759e")
HELIUS_BASE_URL = "https://api.helius.xyz"
WEBHOOK_URL = os.getenv("CHIMERA_MONITORING__HELIUS_WEBHOOK_URL",
                      "https://chimera.example.com/api/v1/monitoring/helius-webhook")


def register_webhook(wallet_addresses: List[str]) -> Dict:
    """
    Register webhooks for multiple wallets with Helius

    Args:
        wallet_addresses: List of Solana wallet addresses to monitor

    Returns:
        Registration response from Helius
    """
    url = f"{HELIUS_BASE_URL}/v0/webhooks?api-key={HELIUS_API_KEY}"

    webhook_config = {
        "webhookURL": WEBHOOK_URL,
        "transactionTypes": ["SWAP", "TRANSFER"],
        "accountKeys": wallet_addresses,
        "webhookType": "enhanced"
    }

    try:
        response = requests.post(url, json=webhook_config)
        response.raise_for_status()
        return response.json()
    except requests.exceptions.RequestException as e:
        print(f"Error registering webhooks: {e}")
        return {"error": str(e)}


def get_existing_webhooks() -> List[Dict]:
    """
    Get all existing webhooks from Helius

    Returns:
        List of existing webhook configurations
    """
    url = f"{HELIUS_BASE_URL}/v0/webhooks?api-key={HELIUS_API_KEY}"

    try:
        response = requests.get(url)
        response.raise_for_status()
        return response.json()
    except requests.exceptions.RequestException as e:
        print(f"Error fetching webhooks: {e}")
        return []


def delete_webhook(webhook_id: str) -> bool:
    """
    Delete a specific webhook from Helius

    Args:
        webhook_id: Helius webhook ID to delete

    Returns:
        True if successful, False otherwise
    """
    url = f"{HELIUS_BASE_URL}/v0/webhooks/{webhook_id}?api-key={HELIUS_API_KEY}"

    try:
        response = requests.delete(url)
        response.raise_for_status()
        return True
    except requests.exceptions.RequestException as e:
        print(f"Error deleting webhook {webhook_id}: {e}")
        return False


def test_webhook_url(url: str) -> bool:
    """
    Test if webhook URL is accessible

    Args:
        url: Webhook URL to test

    Returns:
        True if accessible, False otherwise
    """
    try:
        # Test with a simple health check
        health_url = url.replace("/api/v1/monitoring/helius-webhook", "/api/v1/health")
        response = requests.get(health_url, timeout=5)
        return response.status_code == 200
    except requests.exceptions.RequestException:
        return False


def list_wallets_from_db(db_path: str = "data/chimera.db") -> List[str]:
    """
    List ACTIVE wallets from Chimera database

    Args:
        db_path: Path to Chimera database

    Returns:
        List of ACTIVE wallet addresses
    """
    try:
        import sqlite3
        conn = sqlite3.connect(db_path)
        cursor = conn.cursor()

        cursor.execute("""
            SELECT address FROM wallets
            WHERE status = 'ACTIVE'
            ORDER BY wqs_score DESC
        """)

        wallets = [row[0] for row in cursor.fetchall()]
        conn.close()

        return wallets
    except Exception as e:
        print(f"Error reading database: {e}")
        return []


def validate_wallet_address(address: str) -> bool:
    """
    Validate Solana wallet address format

    Args:
        address: Solana wallet address to validate

    Returns:
        True if valid format, False otherwise
    """
    # Basic validation: Solana addresses are base58 encoded, typically 32-44 characters
    return isinstance(address, str) and 32 <= len(address) <= 44 and address.isalnum()


def main():
    parser = argparse.ArgumentParser(
        description="Register Helius webhooks for Chimera trading system"
    )
    parser.add_argument(
        "wallets",
        nargs="*",
        help="Wallet addresses to register (if not provided, uses ACTIVE wallets from DB)"
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List existing webhooks"
    )
    parser.add_argument(
        "--test",
        action="store_true",
        help="Test webhook URL accessibility"
    )
    parser.add_argument(
        "--delete",
        type=str,
        metavar="WEBHOOK_ID",
        help="Delete specific webhook by ID"
    )
    parser.add_argument(
        "--delete-all",
        action="store_true",
        help="Delete all existing webhooks"
    )
    parser.add_argument(
        "--url",
        type=str,
        help="Override webhook URL"
    )
    parser.add_argument(
        "--db",
        type=str,
        default="data/chimera.db",
        help="Path to Chimera database"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be done without making changes"
    )

    args = parser.parse_args()

    # Override webhook URL if provided
    global WEBHOOK_URL
    if args.url:
        WEBHOOK_URL = args.url

    print("=" * 70)
    print("Chimera Trading System - Helius Webhook Registration")
    print("=" * 70)
    print(f"Webhook URL: {WEBHOOK_URL}")
    print(f"Helius API Key: {HELIUS_API_KEY[:10]}...")
    print("=" * 70)

    # List existing webhooks
    if args.list:
        print("\n📋 Existing Webhooks:")
        webhooks = get_existing_webhooks()

        if not webhooks:
            print("No existing webhooks found")
        else:
            for webhook in webhooks:
                print(f"  ID: {webhook.get('webhookID', 'N/A')}")
                print(f"  URL: {webhook.get('webhookURL', 'N/A')}")
                print(f"  Type: {webhook.get('webhookType', 'N/A')}")
                print(f"  Accounts: {', '.join(webhook.get('accountKeys', []))}")
                print()

    # Test webhook URL
    if args.test:
        print("\n🧪 Testing Webhook URL:")
        if test_webhook_url(WEBHOOK_URL):
            print(f"✅ Webhook URL is accessible: {WEBHOOK_URL}")
        else:
            print(f"❌ Webhook URL is not accessible: {WEBHOOK_URL}")
            print("Please check:")
            print("1. Server is running and accessible")
            print("2. Firewall allows inbound connections")
            print("3. DNS is correctly configured")
        return

    # Delete specific webhook
    if args.delete:
        print(f"\n🗑️  Deleting Webhook: {args.delete}")
        if args.dry_run:
            print("[DRY RUN] Would delete webhook")
        else:
            if delete_webhook(args.delete):
                print("✅ Webhook deleted successfully")
            else:
                print("❌ Failed to delete webhook")
        return

    # Delete all webhooks
    if args.delete_all:
        print("\n🗑️  Deleting All Webhooks:")
        webhooks = get_existing_webhooks()

        if not webhooks:
            print("No existing webhooks to delete")
            return

        for webhook in webhooks:
            webhook_id = webhook.get('webhookID')
            print(f"Deleting: {webhook_id}")

            if args.dry_run:
                print("[DRY RUN] Would delete webhook")
            else:
                if delete_webhook(webhook_id):
                    print(f"✅ Deleted {webhook_id}")
                else:
                    print(f"❌ Failed to delete {webhook_id}")

        if not args.dry_run:
            print("✅ All webhooks deleted")
        return

    # Register webhooks
    print("\n🔗 Registering Webhooks:")

    # Get wallet addresses
    if args.wallets:
        wallet_addresses = args.wallets
        print(f"Using {len(wallet_addresses)} wallet addresses from command line")
    else:
        wallet_addresses = list_wallets_from_db(args.db)
        print(f"Using {len(wallet_addresses)} ACTIVE wallets from database")

    if not wallet_addresses:
        print("❌ No wallet addresses provided")
        print("Either:")
        print("1. Provide wallet addresses as arguments")
        print("2. Ensure database has ACTIVE wallets")
        return

    # Validate wallet addresses
    valid_wallets = []
    invalid_wallets = []

    for wallet in wallet_addresses:
        if validate_wallet_address(wallet):
            valid_wallets.append(wallet)
        else:
            invalid_wallets.append(wallet)

    if invalid_wallets:
        print(f"⚠️  Skipping {len(invalid_wallets)} invalid wallet addresses:")
        for wallet in invalid_wallets:
            print(f"  - {wallet}")

    if not valid_wallets:
        print("❌ No valid wallet addresses to register")
        return

    # Register webhooks
    if args.dry_run:
        print(f"[DRY RUN] Would register {len(valid_wallets)} wallets:")
        for wallet in valid_wallets:
            print(f"  - {wallet}")
        return

    print(f"Registering {len(valid_wallets)} wallets...")

    result = register_webhook(valid_wallets)

    if "error" in result:
        print(f"❌ Registration failed: {result['error']}")
        print("\nPlease check:")
        print("1. Helius API key is valid")
        print("2. Webhook URL is accessible")
        print("3. Network connectivity is working")
        return

    print("✅ Webhook registration successful!")
    print(f"\nRegistration Details:")
    print(f"Webhook ID: {result.get('webhookID', 'N/A')}")
    print(f"Wallets Registered: {', '.join(valid_wallets)}")
    print(f"Webhook URL: {WEBHOOK_URL}")
    print(f"Timestamp: {datetime.now().isoformat()}")

    print("\n🎯 Next Steps:")
    print("1. Monitor webhook endpoint: /api/v1/monitoring/helius-webhook")
    print("2. Check operator logs for webhook activity")
    print("3. Verify trading signals are generated")
    print("=" * 70)


if __name__ == "__main__":
    main()