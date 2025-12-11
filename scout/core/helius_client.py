"""
Helius API client for wallet discovery and transaction fetching.
"""

import os
import time
import json
from datetime import datetime, timedelta
from typing import List, Optional, Dict, Any, Set, Tuple
from dataclasses import dataclass
from pathlib import Path
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
import requests


@dataclass
class DiscoveryStats:
    """Statistics for wallet discovery run."""
    strategy_used: str
    wallets_found: int
    api_calls_made: int
    errors_encountered: int
    time_taken_seconds: float


class HeliusClient:
    """Client for Helius API to discover wallets and fetch transactions."""

    # Known DEX program IDs
    JUPITER_PROGRAM = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"
    RAYDIUM_PROGRAM = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
    ORCA_PROGRAM = "9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP"
    
    # Known system accounts to filter out
    SYSTEM_ACCOUNTS = {
        "11111111111111111111111111111111",  # System Program
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  # Token Program
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",  # Associated Token Program
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  # Token-2022 Program
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",  # Memo Program
        "Sysvar1nstructions1111111111111111111111111",  # Sysvar Instructions
        "SysvarRent111111111111111111111111111111111",  # Sysvar Rent
        "SysvarC1ock11111111111111111111111111111111",  # Sysvar Clock
    }

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
        
        # Caching
        self._discovery_cache: Optional[Dict[str, Any]] = None
        self._discovery_cache_time: Optional[float] = None
        self._token_list_cache: Optional[List[str]] = None
        self._token_list_cache_time: Optional[float] = None
        
        # Circuit breaker
        self._circuit_breaker_failures = 0
        self._circuit_breaker_threshold = 5
        self._circuit_breaker_reset_time: Optional[float] = None
        
        # API call tracking
        self._api_calls_made = 0
        self._max_api_calls = int(os.getenv("SCOUT_MAX_API_CALLS_PER_RUN", "500"))
        
        # Known wallets (for deduplication)
        self._known_wallets_cache: Set[str] = set()
        self._discovered_this_run: Set[str] = set()

    def _rate_limit(self):
        """Ensure we don't exceed rate limits."""
        current_time = time.time()
        time_since_last = current_time - self.last_request_time
        if time_since_last < self.rate_limit_delay:
            time.sleep(self.rate_limit_delay - time_since_last)
        self.last_request_time = time.time()
    
    def _check_circuit_breaker(self) -> bool:
        """Check if circuit breaker should prevent requests."""
        if self._circuit_breaker_reset_time and time.time() > self._circuit_breaker_reset_time:
            self._circuit_breaker_failures = 0
            self._circuit_breaker_reset_time = None
        
        if self._circuit_breaker_failures >= self._circuit_breaker_threshold:
            return False  # Circuit is open, don't make requests
        return True  # Circuit is closed, allow requests
    
    def _record_failure(self):
        """Record a failure for circuit breaker."""
        self._circuit_breaker_failures += 1
        if self._circuit_breaker_failures >= self._circuit_breaker_threshold:
            # Open circuit for 60 seconds
            self._circuit_breaker_reset_time = time.time() + 60
    
    def _record_success(self):
        """Record a success, reset circuit breaker if needed."""
        if self._circuit_breaker_failures > 0:
            self._circuit_breaker_failures = max(0, self._circuit_breaker_failures - 1)
    
    def _retry_with_backoff(self, func, max_retries: int = 3, *args, **kwargs):
        """Retry a function with exponential backoff."""
        for attempt in range(max_retries):
            try:
                result = func(*args, **kwargs)
                self._record_success()
                return result
            except Exception as e:
                if attempt == max_retries - 1:
                    self._record_failure()
                    raise
                backoff_time = 2 ** attempt  # 1s, 2s, 4s
                time.sleep(backoff_time)
        return None

    def _make_request(self, endpoint: str, params: Optional[Dict[str, Any]] = None, use_retry: bool = True) -> Optional[Dict[str, Any]]:
        """
        Make a request to Helius API.

        Args:
            endpoint: API endpoint path
            params: Query parameters
            use_retry: Whether to use retry logic

        Returns:
            JSON response or None if request failed
        """
        if not self.api_key:
            return None
        
        if not self._check_circuit_breaker():
            print("[Helius] Circuit breaker is open, skipping request")
            return None
        
        if self._api_calls_made >= self._max_api_calls:
            print(f"[Helius] Max API calls ({self._max_api_calls}) reached")
            return None

        def _do_request():
            self._rate_limit()
            url = f"{self.base_url}{endpoint}"
            request_params = params.copy() if params else {}
            request_params["api-key"] = self.api_key

            response = requests.get(url, params=request_params, timeout=30)
            
            # Handle rate limiting
            if response.status_code == 429:
                retry_after = int(response.headers.get("Retry-After", 5))
                print(f"[Helius] Rate limited, waiting {retry_after}s")
                time.sleep(retry_after)
                response = requests.get(url, params=request_params, timeout=30)
            
            response.raise_for_status()
            self._api_calls_made += 1
            return response.json()
        
        try:
            if use_retry:
                return self._retry_with_backoff(_do_request)
            else:
                return _do_request()
        except requests.exceptions.RequestException as e:
            print(f"[Helius] API request failed: {e}")
            if hasattr(e, 'response') and e.response is not None:
                try:
                    print(f"[Helius] Response: {e.response.text[:200]}")
                except:
                    pass
            return None

    def _load_active_tokens(self) -> List[str]:
        """Load active token addresses from config file or environment."""
        # Check environment variable first
        env_tokens = os.getenv("SCOUT_ACTIVE_TOKENS", "")
        if env_tokens:
            return [t.strip() for t in env_tokens.split(",") if t.strip()]
        
        # Check cache
        if self._token_list_cache and self._token_list_cache_time:
            if time.time() - self._token_list_cache_time < 86400:  # 24 hours
                return self._token_list_cache
        
        # Load from config file
        config_path = Path(__file__).parent.parent / "config" / "active_tokens.txt"
        tokens = []
        
        if config_path.exists():
            try:
                with open(config_path, 'r') as f:
                    for line in f:
                        line = line.strip()
                        if line and not line.startswith('#'):
                            tokens.append(line)
            except Exception as e:
                print(f"[Helius] Warning: Failed to load token list: {e}")
        
        # Default tokens if none loaded
        if not tokens:
            tokens = [
                "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",  # BONK
                "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  # WIF
                "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr",  # POPCAT
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
                "So11111111111111111111111111111111111111112",  # SOL
            ]
        
        # Cache the result
        self._token_list_cache = tokens
        self._token_list_cache_time = time.time()
        
        return tokens
    
    def _load_seed_wallets(self) -> List[str]:
        """Load seed wallet addresses from config file or environment."""
        # Check environment variable first
        env_wallets = os.getenv("SCOUT_SEED_WALLETS", "")
        if env_wallets:
            return [w.strip() for w in env_wallets.split(",") if w.strip()]
        
        # Load from config file
        config_path = Path(__file__).parent.parent / "config" / "seed_wallets.txt"
        wallets = []
        
        if config_path.exists():
            try:
                with open(config_path, 'r') as f:
                    for line in f:
                        line = line.strip()
                        if line and not line.startswith('#'):
                            wallets.append(line)
            except Exception as e:
                print(f"[Helius] Warning: Failed to load seed wallets: {e}")
        
        return wallets
    
    def _is_wallet_known(self, wallet_address: str, check_database: bool = True) -> bool:
        """
        Check if wallet is already known (in database or discovered this run).
        
        Args:
            wallet_address: Wallet address to check
            check_database: Whether to check database (default: True)
                           Set to False to allow rediscovery of existing wallets
        """
        if wallet_address in self._known_wallets_cache:
            return True
        if wallet_address in self._discovered_this_run:
            return True
        
        # Check database if available and enabled
        if check_database:
            try:
                db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
                if os.path.exists(db_path):
                    import sqlite3
                    conn = sqlite3.connect(db_path)
                    cursor = conn.cursor()
                    cursor.execute("SELECT 1 FROM wallets WHERE address = ? LIMIT 1", (wallet_address,))
                    exists = cursor.fetchone() is not None
                    conn.close()
                    
                    if exists:
                        self._known_wallets_cache.add(wallet_address)
                        return True
            except Exception:
                pass  # Ignore database errors
        
        return False
    
    def _validate_wallet_address(self, address: str) -> bool:
        """Validate that an address is a valid Solana wallet address."""
        if not address or not isinstance(address, str):
            return False
        
        # Check length (Solana addresses are 32-44 base58 characters)
        if not (32 <= len(address) <= 44):
            return False
        
        # Check if it's a known system account
        if address in self.SYSTEM_ACCOUNTS:
            return False
        
        # Check if it's a known DEX program
        if address in [self.JUPITER_PROGRAM, self.RAYDIUM_PROGRAM, self.ORCA_PROGRAM]:
            return False
        
        # Filter out known token mints and programs
        known_programs = {
            "So11111111111111111111111111111111111111112",  # Wrapped SOL
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",  # USDT
            "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",  # BONK
            "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  # WIF
            "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr",  # POPCAT
            "ComputeBudget111111111111111111111111111111",  # Compute Budget Program
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",  # Whirlpool Program
        }
        if address in known_programs:
            return False
        
        # Filter addresses that look like programs (ending in many 1s or common patterns)
        if address.endswith("11111111111111111111111111111111"):
            return False
        
        # Basic base58 character check (simplified - Solana uses base58)
        valid_chars = set("123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz")
        if not all(c in valid_chars for c in address):
            return False
        
        return True
    
    def _extract_wallets_from_transaction(self, tx: Dict[str, Any]) -> List[str]:
        """
        Extract multiple wallet addresses from a transaction.
        
        Args:
            tx: Transaction dictionary from Helius API
            
        Returns:
            List of unique valid wallet addresses
        """
        if not isinstance(tx, dict):
            return []
        
        wallets: Set[str] = set()
        
        # Primary: Extract fee payer (transaction signer) - most reliable
        if "feePayer" in tx:
            fee_payer = tx["feePayer"]
            if self._validate_wallet_address(fee_payer):
                wallets.add(fee_payer)
        
        # Secondary: Extract from accountData array (user accounts)
        if "accountData" in tx:
            for acc in tx.get("accountData", []):
                if isinstance(acc, dict) and "account" in acc:
                    account_addr = acc.get("account")
                    if account_addr and self._validate_wallet_address(account_addr):
                        wallets.add(account_addr)
        
        # Tertiary: Extract from nativeTransfers
        if "nativeTransfers" in tx:
            for transfer in tx.get("nativeTransfers", []):
                if isinstance(transfer, dict):
                    for key in ["fromUserAccount", "toUserAccount"]:
                        if key in transfer:
                            addr = transfer[key]
                            if self._validate_wallet_address(addr):
                                wallets.add(addr)
        
        # Tertiary: Extract from tokenTransfers
        if "tokenTransfers" in tx:
            for transfer in tx.get("tokenTransfers", []):
                if isinstance(transfer, dict):
                    for key in ["fromUserAccount", "toUserAccount", "userAccount"]:
                        if key in transfer:
                            addr = transfer[key]
                            if self._validate_wallet_address(addr):
                                wallets.add(addr)
        
        return list(wallets)
    
    def _validate_wallet_activity(
        self,
        wallet_address: str,
        min_trades: int = 3,
        days_back: int = 7
    ) -> bool:
        """
        Quick validation of wallet activity.
        
        Args:
            wallet_address: Wallet address to validate
            min_trades: Minimum number of trades required
            days_back: Number of days to look back
            
        Returns:
            True if wallet meets activity criteria
        """
        try:
            # Quick transaction count check
            transactions = self.get_wallet_transactions(wallet_address, days=days_back, limit=min_trades + 1)
            return len(transactions) >= min_trades
        except Exception:
            return False  # If we can't validate, assume invalid
    
    def _query_token_transactions(
        self,
        token_addr: str,
        cutoff_time: int,
        limit_per_token: int
    ) -> Tuple[str, List[Dict[str, Any]]]:
        """Query transactions for a single token (for parallel processing)."""
        try:
            endpoint = f"/addresses/{token_addr}/transactions"
            request_params = {
                "type": "SWAP",
            }
            # Note: Helius API 'before' parameter expects a transaction signature, not timestamp
            # We'll query recent transactions without time filtering for now
            
            data = self._make_request(endpoint, request_params)
            if not data:
                return token_addr, []
            
            transactions = data if isinstance(data, list) else data.get("transactions", [])
            
            # Filter by time window
            if cutoff_time > 0:
                filtered_transactions = []
                for tx in transactions:
                    tx_timestamp = tx.get("timestamp")
                    if tx_timestamp and tx_timestamp >= cutoff_time:
                        filtered_transactions.append(tx)
                transactions = filtered_transactions
            
            # Limit results
            if limit_per_token > 0:
                transactions = transactions[:limit_per_token]
            
            return token_addr, transactions
        except Exception as e:
            print(f"[Helius] Warning: Failed to query token {token_addr[:8]}...: {e}")
            return token_addr, []
    
    def _discover_from_active_tokens(
        self,
        token_addresses: Optional[List[str]] = None,
        hours_back: int = 24,
        limit_per_token: int = 200,
        use_parallel: bool = True
    ) -> Dict[str, int]:
        """
        Discover wallets from active token swap transactions.
        
        Args:
            token_addresses: List of token addresses to query (None to use defaults)
            hours_back: Number of hours to look back
            limit_per_token: Maximum transactions per token
            use_parallel: Whether to use parallel processing (respects rate limits)
            
        Returns:
            Dictionary mapping wallet addresses to trade counts
        """
        if token_addresses is None:
            token_addresses = self._load_active_tokens()
        
        wallet_counts: Dict[str, int] = defaultdict(int)
        cutoff_time = int((datetime.utcnow() - timedelta(hours=hours_back)).timestamp())
        
        print(f"[Helius] Discovering from {len(token_addresses)} active tokens...")
        
        if use_parallel and len(token_addresses) > 1:
            # Use parallel processing with rate limiting
            max_workers = min(5, len(token_addresses))  # Limit concurrent requests
            
            with ThreadPoolExecutor(max_workers=max_workers) as executor:
                futures = {
                    executor.submit(self._query_token_transactions, token_addr, cutoff_time, limit_per_token): token_addr
                    for token_addr in token_addresses
                    if self._api_calls_made < self._max_api_calls
                }
                
                for future in as_completed(futures):
                    if self._api_calls_made >= self._max_api_calls:
                        break
                    
                    token_addr, transactions = future.result()
                    
                    for tx in transactions:
                        wallets = self._extract_wallets_from_transaction(tx)
                        for wallet in wallets:
                            # Count each transaction appearance (wallet can appear multiple times)
                            # Only validate address, don't check if known
                            if self._validate_wallet_address(wallet):
                                wallet_counts[wallet] += 1
                                # Track for deduplication within this run
                                self._discovered_this_run.add(wallet)
                    
                    if transactions:
                        print(f"[Helius] Processed {len(transactions)} transactions from token {token_addr[:8]}...")
        else:
            # Sequential processing
            for token_addr in token_addresses:
                if self._api_calls_made >= self._max_api_calls:
                    print(f"[Helius] Reached max API calls, stopping token queries")
                    break
                
                token_addr, transactions = self._query_token_transactions(token_addr, cutoff_time, limit_per_token)
                
                for tx in transactions:
                    wallets = self._extract_wallets_from_transaction(tx)
                    for wallet in wallets:
                        # Count each transaction appearance (wallet can appear multiple times)
                        # Only validate address, don't check if known
                        if self._validate_wallet_address(wallet):
                            wallet_counts[wallet] += 1
                            # Track for deduplication within this run
                            self._discovered_this_run.add(wallet)
                
                if transactions:
                    print(f"[Helius] Processed {len(transactions)} transactions from token {token_addr[:8]}...")
        
        print(f"[Helius] Found {len(wallet_counts)} unique wallets from token queries")
        return dict(wallet_counts)
    
    def _query_recent_blocks(self, num_blocks: int = 100) -> List[Dict]:
        """
        Query recent blocks using RPC.
        
        Args:
            num_blocks: Number of recent blocks to query
            
        Returns:
            List of block data dictionaries
        """
        # Note: This requires RPC endpoint, not just Helius Enhanced API
        # For now, return empty list - this would need RPC client implementation
        print("[Helius] Block-based discovery requires RPC client (not implemented yet)")
        return []
    
    def _discover_from_recent_blocks(
        self,
        num_blocks: int = 100
    ) -> Dict[str, int]:
        """
        Discover wallets from recent blocks by filtering DEX transactions.
        
        Args:
            num_blocks: Number of recent blocks to query
            
        Returns:
            Dictionary mapping wallet addresses to trade counts
        """
        wallet_counts: Dict[str, int] = defaultdict(int)
        
        # Get recent blocks
        blocks = self._query_recent_blocks(num_blocks)
        
        if not blocks:
            return {}
        
        dex_programs = {self.JUPITER_PROGRAM, self.RAYDIUM_PROGRAM, self.ORCA_PROGRAM}
        
        for block in blocks:
            transactions = block.get("transactions", [])
            for tx in transactions:
                # Check if transaction involves DEX programs
                if "instructions" in tx:
                    for inst in tx.get("instructions", []):
                        if "programId" in inst and inst["programId"] in dex_programs:
                            # Extract wallet from transaction
                            wallets = self._extract_wallets_from_transaction(tx)
                            for wallet in wallets:
                                if wallet not in self._discovered_this_run:
                                    wallet_counts[wallet] += 1
                                    self._discovered_this_run.add(wallet)
                            break
        
        return dict(wallet_counts)
    
    def _discover_from_dex_programs(
        self,
        hours_back: int = 24,
        limit: int = 500
    ) -> Dict[str, int]:
        """
        Discover wallets by querying DEX program accounts.
        
        Args:
            hours_back: Number of hours to look back
            limit: Maximum transactions to query per program
            
        Returns:
            Dictionary mapping wallet addresses to trade counts
        """
        wallet_counts: Dict[str, int] = defaultdict(int)
        cutoff_time = int((datetime.utcnow() - timedelta(hours=hours_back)).timestamp())
        
        dex_programs = [
            self.JUPITER_PROGRAM,
            self.RAYDIUM_PROGRAM,
            self.ORCA_PROGRAM,
        ]
        
        print(f"[Helius] Discovering from {len(dex_programs)} DEX programs...")
        
        # Use RPC method getTransactionsForAddress
        rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
        
        if not rpc_url or "helius" not in rpc_url.lower():
            print("[Helius] RPC URL not configured for program account queries")
            return {}
        
        for program_id in dex_programs:
            if self._api_calls_made >= self._max_api_calls:
                break
            
            try:
                # Use RPC POST request
                payload = {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getTransactionsForAddress",
                    "params": [
                        program_id,
                        {
                            "transactionDetails": "full",
                            "sortOrder": "desc",
                            "limit": limit,
                            "filters": {
                                "blockTime": {
                                    "gte": cutoff_time
                                },
                                "status": "succeeded"
                            }
                        }
                    ]
                }
                
                # Extract API key from RPC URL
                api_key = self.api_key
                if "api-key=" in rpc_url:
                    api_key = rpc_url.split("api-key=")[1].split("&")[0].split("?")[0]
                
                # Make RPC request
                self._rate_limit()
                response = requests.post(
                    rpc_url.split("?")[0] if "?" in rpc_url else rpc_url,
                    json=payload,
                    params={"api-key": api_key} if api_key else {},
                    timeout=30
                )
                
                if response.status_code == 429:
                    retry_after = int(response.headers.get("Retry-After", 5))
                    time.sleep(retry_after)
                    response = requests.post(
                        rpc_url.split("?")[0] if "?" in rpc_url else rpc_url,
                        json=payload,
                        params={"api-key": api_key} if api_key else {},
                        timeout=30
                    )
                
                response.raise_for_status()
                self._api_calls_made += 1
                
                data = response.json()
                if "result" in data and "data" in data["result"]:
                    transactions = data["result"]["data"]
                    
                    for tx in transactions:
                        wallets = self._extract_wallets_from_transaction(tx)
                        for wallet in wallets:
                            if self._validate_wallet_address(wallet):
                                wallet_counts[wallet] += 1
                                self._discovered_this_run.add(wallet)
                
            except Exception as e:
                print(f"[Helius] Warning: Failed to query program {program_id[:8]}...: {e}")
                continue
        
        return dict(wallet_counts)
    
    def _discover_from_seed_wallets(
        self,
        hours_back: int = 24,
        limit_per_wallet: int = 50
    ) -> Dict[str, int]:
        """
        Discover wallets from seed wallet transactions.
        
        Args:
            hours_back: Number of hours to look back
            limit_per_wallet: Maximum transactions per seed wallet
            
        Returns:
            Dictionary mapping wallet addresses to trade counts
        """
        seed_wallets = self._load_seed_wallets()
        
        if not seed_wallets:
            return {}
        
        wallet_counts: Dict[str, int] = defaultdict(int)
        
        print(f"[Helius] Discovering from {len(seed_wallets)} seed wallets...")
        
        for seed_wallet in seed_wallets[:10]:  # Limit to 10 seed wallets
            if self._api_calls_made >= self._max_api_calls:
                break
            
            try:
                transactions = self.get_wallet_transactions(
                    seed_wallet,
                    days=hours_back // 24 + 1,
                    limit=limit_per_wallet
                )
                
                for tx in transactions:
                    wallets = self._extract_wallets_from_transaction(tx)
                    for wallet in wallets:
                        # Don't count the seed wallet itself
                        if wallet != seed_wallet and self._validate_wallet_address(wallet):
                            wallet_counts[wallet] += 1
                            self._discovered_this_run.add(wallet)
                
            except Exception as e:
                print(f"[Helius] Warning: Failed to query seed wallet {seed_wallet[:8]}...: {e}")
                continue
        
        return dict(wallet_counts)
    
    def discover_wallets_from_recent_swaps(
        self,
        limit: int = 1000,
        min_trade_count: int = 3,
        max_wallets: int = 200,
        hours_back: int = 24,
    ) -> List[str]:
        """
        Discover wallet addresses from recent swap transactions using multiple strategies.

        This method uses a fallback chain:
        1. Active token queries (primary)
        2. Recent blocks (secondary)
        3. DEX program accounts (tertiary)
        4. Seed wallets (fallback)

        Args:
            limit: Maximum number of transactions to query (deprecated, kept for compatibility)
            min_trade_count: Minimum number of trades a wallet must have to be included
            max_wallets: Maximum number of wallets to return
            hours_back: Number of hours to look back for transactions

        Returns:
            List of unique wallet addresses, sorted by activity
        """
        start_time = time.time()
        strategy_used = "none"
        errors_encountered = 0
        
        # Reset discovery state
        self._discovered_this_run.clear()
        self._api_calls_made = 0
        
        if not self.api_key:
            print("[Helius] Warning: No Helius API key configured, cannot discover wallets")
            return []

        print(f"[Helius] Discovering wallets from recent swaps...")
        print(f"[Helius] Config: min_trades={min_trade_count}, max_wallets={max_wallets}, hours_back={hours_back}")

        # Check discovery cache
        cache_ttl = int(os.getenv("SCOUT_DISCOVERY_CACHE_TTL", "3600"))
        if self._discovery_cache and self._discovery_cache_time:
            if time.time() - self._discovery_cache_time < cache_ttl:
                print("[Helius] Using cached discovery results")
                return self._discovery_cache.get("wallets", [])[:max_wallets]

        wallet_counts: Dict[str, int] = defaultdict(int)
        
        # Strategy 1: Active Token Discovery (Primary)
        try:
            print("[Helius] Strategy 1: Querying active tokens...")
            token_wallets = self._discover_from_active_tokens(hours_back=hours_back, limit_per_token=200)
            for wallet, count in token_wallets.items():
                wallet_counts[wallet] += count
            strategy_used = "tokens"
            print(f"[Helius] Strategy 1 found {len(token_wallets)} wallets")
        except Exception as e:
            errors_encountered += 1
            print(f"[Helius] Strategy 1 failed: {e}")
        
        # Strategy 2: Recent Blocks (Secondary) - Skip if we have enough wallets
        if len(wallet_counts) < max_wallets // 2:
            try:
                print("[Helius] Strategy 2: Querying recent blocks...")
                block_wallets = self._discover_from_recent_blocks(num_blocks=100)
                for wallet, count in block_wallets.items():
                    wallet_counts[wallet] += count
                if block_wallets:
                    strategy_used = f"{strategy_used}+blocks"
                print(f"[Helius] Strategy 2 found {len(block_wallets)} wallets")
            except Exception as e:
                errors_encountered += 1
                print(f"[Helius] Strategy 2 failed: {e}")
        
        # Strategy 3: DEX Program Accounts (Tertiary) - Skip if we have enough wallets
        if len(wallet_counts) < max_wallets // 2:
            try:
                print("[Helius] Strategy 3: Querying DEX program accounts...")
                program_wallets = self._discover_from_dex_programs(hours_back=hours_back, limit=500)
                for wallet, count in program_wallets.items():
                    wallet_counts[wallet] += count
                if program_wallets:
                    strategy_used = f"{strategy_used}+programs"
                print(f"[Helius] Strategy 3 found {len(program_wallets)} wallets")
            except Exception as e:
                errors_encountered += 1
                print(f"[Helius] Strategy 3 failed: {e}")
        
        # Strategy 4: Seed Wallets (Fallback) - Skip if we have enough wallets
        if len(wallet_counts) < max_wallets // 2:
            try:
                print("[Helius] Strategy 4: Querying seed wallets...")
                seed_wallets = self._discover_from_seed_wallets(hours_back=hours_back, limit_per_wallet=50)
                for wallet, count in seed_wallets.items():
                    wallet_counts[wallet] += count
                if seed_wallets:
                    strategy_used = f"{strategy_used}+seeds"
                print(f"[Helius] Strategy 4 found {len(seed_wallets)} wallets")
            except Exception as e:
                errors_encountered += 1
                print(f"[Helius] Strategy 4 failed: {e}")

        if not wallet_counts:
            print("[Helius] No wallets discovered from any strategy")
            print("[Helius] Suggestions:")
            print("[Helius]   1. Configure SCOUT_ACTIVE_TOKENS environment variable")
            print("[Helius]   2. Add seed wallets to scout/config/seed_wallets.txt")
            print("[Helius]   3. Ensure Helius API key is configured")
            return []

        # Filter by minimum trade count and validate addresses
        candidate_wallets = [
            wallet for wallet, count in wallet_counts.items()
            if count >= min_trade_count and self._validate_wallet_address(wallet)
        ]
        
        # Optional: Validate wallet activity (can be slow, so make it optional)
        validate_activity = os.getenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "false").lower() == "true"
        if validate_activity:
            print("[Helius] Validating wallet activity...")
            validated_wallets = []
            for wallet in candidate_wallets:
                if self._validate_wallet_activity(wallet, min_trades=min_trade_count, days_back=7):
                    validated_wallets.append(wallet)
                if len(validated_wallets) >= max_wallets:
                    break
            candidate_wallets = validated_wallets
        
        # Sort by trade count (most active first)
        candidate_wallets.sort(key=lambda w: wallet_counts[w], reverse=True)
        
        # Limit to max_wallets
        candidate_wallets = candidate_wallets[:max_wallets]
        
        # Cache results
        self._discovery_cache = {
            "wallets": candidate_wallets,
            "wallet_counts": dict(wallet_counts),
        }
        self._discovery_cache_time = time.time()
        
        time_taken = time.time() - start_time
        
        print(f"[Helius] Discovery complete:")
        print(f"[Helius]   Strategy: {strategy_used}")
        print(f"[Helius]   Wallets found: {len(candidate_wallets)}")
        print(f"[Helius]   API calls: {self._api_calls_made}")
        print(f"[Helius]   Errors: {errors_encountered}")
        print(f"[Helius]   Time: {time_taken:.2f}s")
        
        if candidate_wallets:
            top_wallet = candidate_wallets[0]
            print(f"[Helius]   Top wallet: {top_wallet[:8]}... ({wallet_counts[top_wallet]} trades)")
        
        return candidate_wallets
    
    def _extract_wallet_from_transaction(self, tx: Dict[str, Any]) -> Optional[str]:
        """
        Extract wallet address from a transaction (legacy method for compatibility).
        
        Uses the enhanced _extract_wallets_from_transaction and returns first wallet.
        
        Args:
            tx: Transaction dictionary from Helius API
            
        Returns:
            Wallet address or None if not found
        """
        wallets = self._extract_wallets_from_transaction(tx)
        return wallets[0] if wallets else None

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

        endpoint = f"/addresses/{wallet_address}/transactions"
        params = {
            "type": "SWAP",
        }
        # Note: Helius API 'before' parameter expects a transaction signature, not timestamp
        # For time-based filtering, we'll filter results after fetching

        data = self._make_request(endpoint, params)
        if not data:
            return []

        transactions = data if isinstance(data, list) else data.get("transactions", [])
        
        # Filter by time window if specified
        if days > 0:
            cutoff = datetime.utcnow() - timedelta(days=days)
            cutoff_timestamp = int(cutoff.timestamp())
            filtered_transactions = []
            for tx in transactions:
                tx_timestamp = tx.get("timestamp")
                if tx_timestamp and tx_timestamp >= cutoff_timestamp:
                    filtered_transactions.append(tx)
            transactions = filtered_transactions
        
        # Limit results
        if limit > 0:
            transactions = transactions[:limit]
        
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
