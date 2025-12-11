"""
Helius API client for wallet discovery and transaction fetching.
"""

import os
import time
from datetime import datetime, timedelta
from typing import List, Optional, Dict, Any
import requests


class HeliusClient:
    """Client for Helius API to discover wallets and fetch transactions."""

    def __init__(self, api_key: Optional[str] = None):
        """
        Initialize Helius client.

        Args:
            api_key: Helius API key (from HELIUS_API_KEY env var if not provided)
        """
        self.api_key = api_key or os.getenv("HELIUS_API_KEY", "")
        if not self.api_key:
            # Try to extract from RPC URL if available
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
            if "api-key=" in rpc_url:
                self.api_key = rpc_url.split("api-key=")[1].split("&")[0].split("?")[0]
        
        # Use Helius API v0 endpoint (same as operator uses)
        self.base_url = "https://api.helius.xyz/v0"
        self.rate_limit_delay = 0.1  # 10 requests per second max
        self.last_request_time = 0.0

    def _rate_limit(self):
        """Ensure we don't exceed rate limits."""
        current_time = time.time()
        time_since_last = current_time - self.last_request_time
        if time_since_last < self.rate_limit_delay:
            time.sleep(self.rate_limit_delay - time_since_last)
        self.last_request_time = time.time()

    def _make_request(self, endpoint: str, params: Optional[Dict[str, Any]] = None) -> Optional[Dict[str, Any]]:
        """
        Make a request to Helius API.

        Args:
            endpoint: API endpoint path
            params: Query parameters

        Returns:
            JSON response or None if request failed
        """
        if not self.api_key:
            return None

        self._rate_limit()

        url = f"{self.base_url}{endpoint}"
        if params is None:
            params = {}
        params["api-key"] = self.api_key

        try:
            response = requests.get(url, params=params, timeout=30)
            response.raise_for_status()
            return response.json()
        except requests.exceptions.RequestException as e:
            print(f"Helius API request failed: {e}")
            if hasattr(e.response, 'text'):
                print(f"Response: {e.response.text[:200]}")
            return None

    def discover_wallets_from_recent_swaps(
        self,
        limit: int = 100,
        min_trade_count: int = 5,
    ) -> List[str]:
        """
        Discover wallet addresses from recent swap transactions.

        This method queries recent swap transactions and extracts wallet addresses
        that have been actively trading.

        Args:
            limit: Maximum number of transactions to query
            min_trade_count: Minimum number of trades a wallet must have to be included

        Returns:
            List of unique wallet addresses
        """
        if not self.api_key:
            print("Warning: No Helius API key configured, cannot discover wallets")
            return []

        print(f"[Helius] Discovering wallets from recent swaps (limit: {limit})...")

        # Query recent transactions using Helius Enhanced Transactions API
        # Note: We'll query for recent token transfers/swaps by querying known DEX program IDs
        # For now, we'll use a different approach: query recent transactions from known DEX addresses
        # Or use the parseTransactionNamespaces endpoint
        
        # Alternative: Query transactions from Jupiter aggregator or Raydium
        # For simplicity, we'll query transactions from a known active wallet or DEX
        # This is a simplified approach - in production you'd use webhooks or more sophisticated discovery
        
        # For now, return empty list and log that discovery needs enhancement
        print("[Helius] Note: Wallet discovery from transactions requires Enhanced Transactions API")
        print("[Helius] For now, using manual wallet list or sample data")
        print("[Helius] To discover wallets, provide a list of known wallet addresses")
        
        # Return empty list - wallet discovery from raw transactions needs more work
        return []
        if not data:
            print("[Helius] Failed to fetch transactions")
            return []

        # Extract wallet addresses from transactions
        wallet_counts: Dict[str, int] = {}
        transactions = data if isinstance(data, list) else data.get("transactions", [])

        for tx in transactions:
            # Extract wallet address from transaction
            # Helius format: transactions have accountData with wallet addresses
            if isinstance(tx, dict):
                # Try different possible fields
                wallet = None
                
                # Check for accountData
                if "accountData" in tx:
                    for acc in tx["accountData"]:
                        if "account" in acc and acc.get("account"):
                            wallet = acc["account"]
                            break
                
                # Check for nativeTransfers
                if not wallet and "nativeTransfers" in tx:
                    for transfer in tx["nativeTransfers"]:
                        if "fromUserAccount" in transfer:
                            wallet = transfer["fromUserAccount"]
                            break
                        if "toUserAccount" in transfer:
                            wallet = transfer["toUserAccount"]
                            break
                
                # Check for tokenTransfers
                if not wallet and "tokenTransfers" in tx:
                    for transfer in tx["tokenTransfers"]:
                        if "fromUserAccount" in transfer:
                            wallet = transfer["fromUserAccount"]
                            break
                        if "toUserAccount" in transfer:
                            wallet = transfer["toUserAccount"]
                            break

                if wallet:
                    wallet_counts[wallet] = wallet_counts.get(wallet, 0) + 1

        # Filter by minimum trade count
        candidate_wallets = [
            wallet for wallet, count in wallet_counts.items()
            if count >= min_trade_count
        ]

        print(f"[Helius] Discovered {len(candidate_wallets)} wallets with {min_trade_count}+ trades")
        return candidate_wallets

    def get_wallet_transactions(
        self,
        wallet_address: str,
        days: int = 30,
        limit: int = 100,
    ) -> List[Dict[str, Any]]:
        """
        Get transaction history for a wallet.

        Args:
            wallet_address: Wallet address to query
            days: Number of days to look back
            limit: Maximum number of transactions to return

        Returns:
            List of transaction dictionaries
        """
        if not self.api_key:
            return []

        endpoint = f"/v0/addresses/{wallet_address}/transactions"
        params = {
            "type": "SWAP",
            "limit": limit,
        }

        # Calculate timestamp for days ago
        if days > 0:
            cutoff = datetime.utcnow() - timedelta(days=days)
            params["before"] = int(cutoff.timestamp())

        data = self._make_request(endpoint, params)
        if not data:
            return []

        transactions = data if isinstance(data, list) else data.get("transactions", [])
        return transactions

    def parse_swap_transaction(self, tx: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """
        Parse a swap transaction to extract trade information.

        Args:
            tx: Transaction dictionary from Helius API

        Returns:
            Parsed trade dictionary or None if not a valid swap
        """
        if not isinstance(tx, dict):
            return None

        # Extract swap information
        # Helius Enhanced Transactions format
        swap_info = None

        # Check for tokenTransfers (indicates a swap)
        if "tokenTransfers" in tx and tx["tokenTransfers"]:
            transfers = tx["tokenTransfers"]
            if len(transfers) >= 2:
                # First transfer is usually the input, second is output
                in_transfer = transfers[0]
                out_transfer = transfers[1] if len(transfers) > 1 else None

                if out_transfer:
                    swap_info = {
                        "token_in": in_transfer.get("mint", ""),
                        "token_out": out_transfer.get("mint", ""),
                        "amount_in": in_transfer.get("tokenAmount", 0),
                        "amount_out": out_transfer.get("tokenAmount", 0),
                        "timestamp": tx.get("timestamp", int(datetime.utcnow().timestamp())),
                        "signature": tx.get("signature", ""),
                        "direction": "BUY" if out_transfer.get("mint") != "So11111111111111111111111111111111111111112" else "SELL",
                    }

        return swap_info
