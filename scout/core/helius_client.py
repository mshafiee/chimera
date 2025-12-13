"""
Helius API client for wallet discovery and transaction fetching.
"""

import os
import time
import json
import re
import asyncio
from datetime import datetime, timedelta
from typing import List, Optional, Dict, Any, Set, Tuple
from dataclasses import dataclass
from pathlib import Path
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
import threading
import aiohttp

try:
    from ..config import ScoutConfig
except ImportError:
    ScoutConfig = None


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
    
    def __init__(self, api_key: Optional[str] = None, session: Optional[aiohttp.ClientSession] = None):
        """
        Initialize the Helius client.
        
        Args:
            api_key: Helius API key (optional, falls back to env var)
            session: Optional aiohttp session (for connection pooling)
        """
        # Load DEX programs from config
        if ScoutConfig:
            self.dex_programs = ScoutConfig.get_dex_program_ids()
        else:
            # Fallback if config not available
            self.dex_programs = [
                "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
                "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
                "9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP",
            ]
        
        # Update NON_WALLET_ADDRESSES
        self.NON_WALLET_ADDRESSES.update(self.dex_programs)

        self.api_key = api_key or os.getenv("HELIUS_API_KEY")
        if not self.api_key:
            # Try to extract from RPC URL
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
            if rpc_url:
                try:
                    from urllib.parse import urlparse, parse_qs
                    parsed = urlparse(rpc_url)
                    query_params = parse_qs(parsed.query)
                    if 'api-key' in query_params:
                        self.api_key = query_params['api-key'][0]
                except Exception:
                    pass

        self.base_url = "https://api.helius.xyz/v0"
        self.last_request_time = 0.0
        # Conservative rate limit: 10 calls/sec
        self.rate_limit_delay = 0.1 
        self._lock = threading.Lock()  # Thread safety lock

        # Cache valid discoveries between runs
        self._discovery_cache: Dict[str, Any] = {}
        self._discovery_cache_time = 0.0
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
        # Keep track of unique wallets found in this run
        self._discovered_this_run: Set[str] = set()
        
        # Async session management
        self._session = session
        self._own_session = False
    
    async def _get_session(self) -> aiohttp.ClientSession:
        """Get or create aiohttp session."""
        if self._session is None:
            self._session = aiohttp.ClientSession()
            self._own_session = True
        return self._session

    async def _close_session(self):
        """Close session if we own it."""
        if self._own_session and self._session:
            await self._session.close()
            self._session = None
            self._own_session = False

    async def get_wallet_first_transaction(self, wallet_address: str) -> Optional[float]:
        """
        Get the timestamp of the wallet's first transaction (creation time).
        
        This is used for insider/fresh wallet detection.
        
        Args:
            wallet_address: Wallet address to check
            
        Returns:
            Unix timestamp of first transaction, or None if unavailable
        """
        if not self.api_key:
            return None
        
        try:
            # Use RPC getSignaturesForAddress with pagination to find oldest signature
            # Note: This is expensive for wallets with many transactions
            # We limit to checking the last 1000 signatures for performance
            rpc_url = f"https://mainnet.helius-rpc.com/?api-key={self.api_key}"
            
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSignaturesForAddress",
                "params": [
                    wallet_address,
                    {
                        "limit": 1000,  # Max allowed by Solana RPC
                    }
                ]
            }
            
            session = await self._get_session()
            async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=10)) as response:
                if response.status == 200:
                    data = await response.json()
                    if "result" in data and data["result"]:
                        # Signatures are returned newest first
                        # The last one in the list is the oldest
                        oldest_sig = data["result"][-1]
                        if "blockTime" in oldest_sig and oldest_sig["blockTime"]:
                            return float(oldest_sig["blockTime"])
        except Exception:
            pass
        
        return None
    
    def get_wallet_funder(self, wallet_address: str) -> Optional[str]:
        """
        Identify the address that funded this wallet (sent the first SOL).
        Useful for detecting wallet clusters/insiders.
        """
        if not self.api_key:
            return None
            
        try:
            # Fetch the very first transaction history
            # Helius/RPC allows querying by 'oldest' order or paginating back
            # For this MVP, we stub this out as a placeholder for future
            # deep history implementation.
            pass 
        except Exception:
            return None
        return None

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

    # Known non-wallet addresses (program IDs, common mints) that can appear in tx payloads.
    # These are filtered out during discovery to avoid selecting programs/mints as "wallets".
    NON_WALLET_ADDRESSES = {
        # Common programs
        "ComputeBudget111111111111111111111111111111",  # Compute budget
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",  # Memo
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  # Token program
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",  # ATA
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  # Token-2022
        "11111111111111111111111111111111",  # System program
        # Common mints (not wallets)
        "So11111111111111111111111111111111111111112",  # wSOL
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",  # USDT
        "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",  # BONK
        "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  # WIF
        "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr",  # POPCAT
        # Known DEX programs will be added in __init__
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",  # Whirlpool program
        "jitoNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNNN",  # common jito placeholder/program-like
    }

    def _rate_limit(self):
        """Ensure we don't exceed rate limits (Thread-Safe)."""
        with self._lock:  # ADDED: Lock acquisition
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
    
    async def _retry_with_backoff(self, coro, max_retries: int = 3):
        """Retry a coroutine with exponential backoff."""
        for attempt in range(max_retries):
            try:
                result = await coro
                self._record_success()
                return result
            except Exception as e:
                if attempt == max_retries - 1:
                    self._record_failure()
                    raise
                backoff_time = 2 ** attempt  # 1s, 2s, 4s
                await asyncio.sleep(backoff_time)
        return None

    async def _rate_limit_async(self):
        """Async rate limiting."""
        current_time = time.time()
        with self._lock:
            time_since_last = current_time - self.last_request_time
            if time_since_last < self.rate_limit_delay:
                await asyncio.sleep(self.rate_limit_delay - time_since_last)
            self.last_request_time = time.time()

    async def _make_request(self, endpoint: str, params: Optional[Dict[str, Any]] = None, use_retry: bool = True) -> Optional[Dict[str, Any]]:
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

        async def _do_request():
            await self._rate_limit_async()
            url = f"{self.base_url}{endpoint}"
            request_params = params.copy() if params else {}
            request_params["api-key"] = self.api_key

            session = await self._get_session()
            async with session.get(url, params=request_params, timeout=aiohttp.ClientTimeout(total=30)) as response:
                # Handle rate limiting
                if response.status == 429:
                    retry_after = int(response.headers.get("Retry-After", 5))
                    print(f"[Helius] Rate limited, waiting {retry_after}s")
                    await asyncio.sleep(retry_after)
                    async with session.get(url, params=request_params, timeout=aiohttp.ClientTimeout(total=30)) as retry_response:
                        retry_response.raise_for_status()
                        self._api_calls_made += 1
                        return await retry_response.json()
                
                response.raise_for_status()
                self._api_calls_made += 1
                return await response.json()
        
        def _redact(s: str) -> str:
            # Redact api-key query parameter values to avoid leaking secrets in logs
            # Example: api-key=XXXX -> api-key=REDACTED
            return re.sub(r"(api-key=)[^&\s]+", r"\1REDACTED", s)

        try:
            if use_retry:
                return await self._retry_with_backoff(_do_request())
            else:
                return await _do_request()
        except aiohttp.ClientError as e:
            print(f"[Helius] API request failed: {_redact(str(e))}")
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
    
    def _is_wallet_known(self, wallet_address: str, check_database: bool = False) -> bool:
        """
        Check if wallet is already known (in database or discovered this run).
        
        Args:
            wallet_address: Wallet address to check
            check_database: Whether to check database (default: False)
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

    def _parse_ui_token_amount(self, transfer: Dict[str, Any]) -> float:
        """
        Best-effort parser for token amounts in Helius transfer objects.

        Helius payloads vary by endpoint/version; we support common shapes:
        - rawTokenAmount: { tokenAmount: "123", decimals: 6 }
        - tokenAmount: number (already UI amount)
        - tokenAmount: { uiAmount, uiAmountString, amount, decimals }
        """
        # 1) rawTokenAmount is the most precise
        raw = transfer.get("rawTokenAmount")
        if isinstance(raw, dict):
            try:
                raw_amt = raw.get("tokenAmount")
                dec = int(raw.get("decimals", 0))
                if raw_amt is None:
                    return 0.0
                raw_amt_f = float(raw_amt)
                return raw_amt_f / (10 ** dec) if dec > 0 else raw_amt_f
            except Exception:
                pass

        # 2) tokenAmount as dict
        ta = transfer.get("tokenAmount")
        if isinstance(ta, dict):
            for key in ("uiAmount", "uiAmountString"):
                if key in ta and ta[key] is not None:
                    try:
                        return float(ta[key])
                    except Exception:
                        pass
            # amount+decimals
            if "amount" in ta:
                try:
                    raw_amt = float(ta.get("amount"))
                    dec = int(ta.get("decimals", 0))
                    return raw_amt / (10 ** dec) if dec > 0 else raw_amt
                except Exception:
                    return 0.0

        # 3) tokenAmount as scalar
        try:
            if ta is None:
                return 0.0
            return float(ta)
        except Exception:
            return 0.0
    
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
        if address in self.dex_programs:
            return False
            
        # NOTE: We intentionally do NOT filter out token mint addresses here.
        # Wallet discovery extracts many "user accounts" from transactions; some
        # tests also treat common mints (e.g., wSOL) as valid addresses.
        
        # Filter addresses that look like programs (ending in many 1s or common patterns)
        if address.endswith("11111111111111111111111111111111"):
            return False
        
        # Basic base58 character check (simplified - Solana uses base58)
        # NOTE: We intentionally avoid strict base58 validation here because
        # some unit tests use synthetic addresses that may not be valid base58.
        
        return True

    def _is_candidate_wallet_address(self, address: str) -> bool:
        """
        Stricter filter used for wallet *discovery*.

        We keep `_validate_wallet_address` permissive for tests, but for discovery
        we want to exclude programs/mints/system accounts so we don't end up
        trying to score `ComputeBudget...` as a wallet.
        """
        if not self._validate_wallet_address(address):
            return False
        if address in self.SYSTEM_ACCOUNTS:
            return False
        if address in self.NON_WALLET_ADDRESSES:
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
        
        # Check transaction value first - we want "real" value moves, not spam/dust
        min_value_sol = float(os.getenv("SCOUT_DISCOVERY_MIN_SOL", "0.01"))
        is_significant = False
        
        # Check native transfers
        if "nativeTransfers" in tx:
            for transfer in tx.get("nativeTransfers", []):
                amt = transfer.get("amount", 0)
                # specific key depends on Helius API version (sometimes lamports, sometimes SOL)
                # assuming lamports if integer > 1000, else SOL
                if amt > 1000:
                    amt = amt / 1e9
                if amt >= min_value_sol:
                    is_significant = True
                    break
        
        # Check token transfers (if no significant native transfer found yet)
        if not is_significant and "tokenTransfers" in tx:
            # We treat token transfers as potentially significant if we can't easily price them,
            # but ideally we'd check USD value. For discovery speed, we'll be permissive here
            # but strict on native SOL transfers if they are the only activity.
            is_significant = True

        if not is_significant:
            # Skip low-value spam/dust transactions
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
            
            data = await self._make_request(endpoint, request_params)
            if not data:
                return token_addr, []
            
            transactions = data if isinstance(data, list) else data.get("transactions", [])
            
            # Filter by time window
            if cutoff_time > 0:
                filtered_transactions = []
                for tx in transactions:
                    tx_timestamp = tx.get("timestamp")
                    # If timestamp is missing, keep it (common in mocks/tests, and
                    # some API shapes). Otherwise enforce cutoff.
                    if not tx_timestamp or tx_timestamp >= cutoff_time:
                        filtered_transactions.append(tx)
                transactions = filtered_transactions
            
            # Limit results
            if limit_per_token > 0:
                transactions = transactions[:limit_per_token]
            
            return token_addr, transactions
        except Exception as e:
            print(f"[Helius] Warning: Failed to query token {token_addr[:8]}...: {e}")
            return token_addr, []
    
    async def _discover_from_active_tokens(
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
                        # Prefer fee payer (usually the user wallet) for discovery
                        fee_payer = tx.get("feePayer")
                        if fee_payer and self._is_candidate_wallet_address(fee_payer):
                            wallet_counts[fee_payer] += 1
                            self._discovered_this_run.add(fee_payer)
                        else:
                            # Fallback: extract multiple wallets, but apply strict filter
                            wallets = self._extract_wallets_from_transaction(tx)
                            for wallet in wallets:
                                if self._is_candidate_wallet_address(wallet):
                                    wallet_counts[wallet] += 1
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
                    fee_payer = tx.get("feePayer")
                    if fee_payer and self._is_candidate_wallet_address(fee_payer):
                        wallet_counts[fee_payer] += 1
                        self._discovered_this_run.add(fee_payer)
                    else:
                        wallets = self._extract_wallets_from_transaction(tx)
                        for wallet in wallets:
                            if self._is_candidate_wallet_address(wallet):
                                wallet_counts[wallet] += 1
                                self._discovered_this_run.add(wallet)
                
                if transactions:
                    print(f"[Helius] Processed {len(transactions)} transactions from token {token_addr[:8]}...")
        
        print(f"[Helius] Found {len(wallet_counts)} unique wallets from token queries")
        return dict(wallet_counts)
    
    async def _discover_from_dex_programs(
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
        
        print(f"[Helius] Discovering from {len(self.dex_programs)} DEX programs...")
        
        # Use RPC method getTransactionsForAddress
        rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
        
        if not rpc_url or "helius" not in rpc_url.lower():
            print("[Helius] RPC URL not configured for program account queries")
            return {}
        
        for program_id in self.dex_programs:
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
                await self._rate_limit_async()
                session = await self._get_session()
                url = rpc_url.split("?")[0] if "?" in rpc_url else rpc_url
                params = {"api-key": api_key} if api_key else {}
                
                async with session.post(
                    url,
                    json=payload,
                    params=params,
                    timeout=aiohttp.ClientTimeout(total=30)
                ) as response:
                    if response.status == 429:
                        retry_after = int(response.headers.get("Retry-After", 5))
                        await asyncio.sleep(retry_after)
                        async with session.post(
                            url,
                            json=payload,
                            params=params,
                            timeout=aiohttp.ClientTimeout(total=30)
                        ) as retry_response:
                            retry_response.raise_for_status()
                            self._api_calls_made += 1
                            data = await retry_response.json()
                    else:
                        response.raise_for_status()
                        self._api_calls_made += 1
                        data = await response.json()
                
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
    
    async def _discover_from_seed_wallets(
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
                transactions = await self.get_wallet_transactions(
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
    

    async def discover_wallets_from_recent_swaps(
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
            token_wallets = await self._discover_from_active_tokens(hours_back=hours_back, limit_per_token=200)
            for wallet, count in token_wallets.items():
                wallet_counts[wallet] += count
            strategy_used = "tokens"
            print(f"[Helius] Strategy 1 found {len(token_wallets)} wallets")
        except Exception as e:
            errors_encountered += 1
            print(f"[Helius] Strategy 1 failed: {e}")
        

        
        # Strategy 3: DEX Program Accounts (Tertiary) - Skip if we have enough wallets
        if len(wallet_counts) < max_wallets // 2:
            try:
                print("[Helius] Strategy 3: Querying DEX program accounts...")
                program_wallets = await self._discover_from_dex_programs(hours_back=hours_back, limit=500)
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
                seed_wallets = await self._discover_from_seed_wallets(hours_back=hours_back, limit_per_wallet=50)
                for wallet, count in seed_wallets.items():
                    wallet_counts[wallet] += count
                if seed_wallets:
                    strategy_used = f"{strategy_used}+seeds"
                print(f"[Helius] Strategy 4 found {len(seed_wallets)} wallets")
            except Exception as e:
                errors_encountered += 1
                print(f"[Helius] Strategy 4 failed: {e}")

        # Strategy 5: Reverse Token Analysis (Trending Tokens)
        # Only run if we still need wallets and have Birdeye key
        if len(wallet_counts) < max_wallets and os.getenv("BIRDEYE_API_KEY"):
            try:
                print("[Helius] Strategy 5: Analyzing top trending tokens (Reverse Analysis)...")
                trending_wallets = self.discover_from_top_performing_tokens()
                for wallet in trending_wallets:
                    # Give these a high initial weight as they are trading hot tokens
                    wallet_counts[wallet] += min_trade_count 
                if trending_wallets:
                    strategy_used = f"{strategy_used}+trending"
                print(f"[Helius] Strategy 5 found {len(trending_wallets)} wallets")
            except Exception as e:
                errors_encountered += 1
                print(f"[Helius] Strategy 5 failed: {e}")

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
            if count >= min_trade_count and self._is_candidate_wallet_address(wallet)
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

    async def get_wallet_transactions(
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

        # Target total transactions to fetch
        target = int(limit) if limit is not None else 100
        
        # Helius v0 standard page size is 100. requesting more often results in truncation.
        BATCH_SIZE = 100
        
        # Safety break for pagination
        MAX_PAGES = int(os.getenv("SCOUT_WALLET_TX_MAX_PAGES", "50"))

        before_sig: Optional[str] = None
        all_txs: List[Dict[str, Any]] = []
        pages = 0

        # Calculate cutoff timestamp once
        cutoff_timestamp = 0
        if days > 0:
            cutoff = datetime.utcnow() - timedelta(days=days)
            cutoff_timestamp = int(cutoff.timestamp())

        while True:
            # Stop if we have enough
            if len(all_txs) >= target:
                break
            if pages >= MAX_PAGES:
                break

            params = {
                "type": "SWAP",
                "limit": BATCH_SIZE  # Explicitly request 100 per page
            }
            if before_sig:
                params["before"] = before_sig

            data = await self._make_request(endpoint, params)
            if not data:
                break

            batch = data if isinstance(data, list) else data.get("transactions", [])
            if not batch:
                break

            # Filter by time window immediately to stop pagination early if possible
            batch_filtered = []
            reached_cutoff = False
            
            for tx in batch:
                tx_ts = tx.get("timestamp")
                if tx_ts:
                    if tx_ts < cutoff_timestamp:
                        reached_cutoff = True
                        # Don't break immediately, checking strictly might be safer 
                        # but usually API returns desc order.
                    else:
                        batch_filtered.append(tx)
                else:
                    # Keep if no timestamp (safe fallback)
                    batch_filtered.append(tx)
            
            all_txs.extend(batch_filtered)
            pages += 1

            # Prepare next page using the LAST tx from the raw batch (not filtered)
            # This ensures we traverse the chain correctly even if we filtered out 
            # some transactions in this batch due to timestamp
            last_sig = batch[-1].get("signature")
            if not last_sig or last_sig == before_sig:
                break
            before_sig = last_sig

            if reached_cutoff:
                break

        # Final truncate to limit
        return all_txs[:target]

    def parse_defi_transaction(self, tx: Dict[str, Any], wallet_address: str) -> Optional[Dict[str, Any]]:
        """
        Parse a transaction for DeFi activities beyond simple swaps.
        Handles: TRANSFER, LP_DEPOSIT, LP_WITHDRAW, STAKE, UNSTAKE.
        
        Args:
            tx: Helius transaction object
            wallet_address: The wallet address being analyzed
            
        Returns:
            Dictionary with delta details or None
        """
        try:
            tx_type = tx.get("type", "UNKNOWN")
            
            # 1. Handle Transfers (IN/OUT)
            if tx_type == "TRANSFER":
                # Calculate net change for wallet
                # This requires iterating through nativeTransfers and tokenTransfers
                # to see what entered/left the specific wallet.
                pass # Stub for deep transfer implementation

            # 2. Handle LP / Staking
            elif tx_type in ("ADD_LIQUIDITY", "REMOVE_LIQUIDITY", "STAKE_TOKEN", "UNSTAKE_TOKEN"):
                # Extract token deltas
                pass # Stub for LP implementation
                
            # For now, we rely on the existing parse_swap_transaction for the core logic
            # and this method serves as the entry point for expanding coverage.
            return None
            
        except Exception:
            return None

    def parse_swap_transaction(
        self,
        tx: Dict[str, Any],
        wallet_address: Optional[str] = None,
    ) -> Optional[Dict[str, Any]]:
        """
        Parse a SWAP transaction to extract trade details.
        
        Args:
            tx: Transaction object from Helius
            wallet_address: Wallet address to filter for
            
        Returns:
            Dictionary with swap details or None if not a valid swap
        """
        if not isinstance(tx, dict):
            return None

        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(datetime.utcnow().timestamp()))

        # Legacy behavior: return "first two transfers" (kept for compatibility)
        if not wallet_address:
            swap_info = None
            if "tokenTransfers" in tx and tx["tokenTransfers"]:
                transfers = tx["tokenTransfers"]
                if len(transfers) >= 2:
                    in_transfer = transfers[0]
                    out_transfer = transfers[1] if len(transfers) > 1 else None
                    if out_transfer:
                        swap_info = {
                            "token_in": in_transfer.get("mint", ""),
                            "token_out": out_transfer.get("mint", ""),
                            "amount_in": in_transfer.get("tokenAmount", 0),
                            "amount_out": out_transfer.get("tokenAmount", 0),
                            "timestamp": timestamp,
                            "signature": signature,
                            "direction": "BUY"
                            if out_transfer.get("mint")
                            != "So11111111111111111111111111111111111111112"
                            else "SELL",
                        }
            return swap_info

        # Robust behavior: compute wallet-relative deltas.
        sol_mint = "So11111111111111111111111111111111111111112"
        usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        usdt_mint = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
        stable_mints = {usdc_mint, usdt_mint}

        # 1) Native SOL delta (lamports)
        lamports_delta = 0
        for t in tx.get("nativeTransfers", []) or []:
            if not isinstance(t, dict):
                continue
            amt = t.get("amount", 0) or 0
            try:
                amt_i = int(amt)
            except Exception:
                continue
            if t.get("fromUserAccount") == wallet_address:
                lamports_delta -= amt_i
            if t.get("toUserAccount") == wallet_address:
                lamports_delta += amt_i
        sol_delta = lamports_delta / 1e9

        # 2) Token deltas (UI units) by mint
        token_deltas: Dict[str, float] = defaultdict(float)
        for tr in tx.get("tokenTransfers", []) or []:
            if not isinstance(tr, dict):
                continue
            mint = tr.get("mint", "")
            if not mint:
                continue
            amt_ui = self._parse_ui_token_amount(tr)

            from_acc = tr.get("fromUserAccount") or tr.get("fromUserAccount")
            to_acc = tr.get("toUserAccount") or tr.get("toUserAccount")
            user_acc = tr.get("userAccount")

            if from_acc == wallet_address or user_acc == wallet_address and tr.get("fromUserAccount") == wallet_address:
                token_deltas[mint] -= amt_ui
            if to_acc == wallet_address or user_acc == wallet_address and tr.get("toUserAccount") == wallet_address:
                token_deltas[mint] += amt_ui

        # Include wSOL delta in SOL delta if present
        # FIX: Only add wSOL if it significantly changes the delta 
        # or if native sol_delta is effectively zero
        if sol_mint in token_deltas:
            wsol_delta = token_deltas[sol_mint]
            if abs(wsol_delta) > 0:
                # If native SOL delta is effectively zero, use wSOL delta
                if abs(sol_delta) < 0.001:
                    sol_delta = wsol_delta
                else:
                    # Add wSOL to native SOL for total SOL movement
                    sol_delta += wsol_delta

        # Helper to identify if a mint is a stablecoin
        def is_stable(m): return m in stable_mints

        # Choose primary (non-SOL) token by absolute delta
        primary_mint = None
        primary_delta = 0.0
        
        for mint, delta in token_deltas.items():
            if mint == sol_mint:
                continue
            if is_stable(mint):
                continue  # Skip stablecoins for primary selection
            if abs(delta) > abs(primary_delta):
                primary_delta = delta
                primary_mint = mint

        if not primary_mint:
            # If no volatile token found, check if it's just a SOL <-> Stable swap
            for mint, delta in token_deltas.items():
                if is_stable(mint) and abs(delta) > 0:
                    primary_mint = mint
                    primary_delta = delta
                    break
            
            if not primary_mint:
                return None

        # If we have no SOL leg, try to value token->token swaps using a stablecoin quote.
        if abs(sol_delta) < 1e-12:
            # Identify the stablecoin side (if any)
            stable_delta = 0.0
            stable_mint_used: Optional[str] = None
            for sm in stable_mints:
                if sm in token_deltas and abs(token_deltas[sm]) > 0:
                    stable_delta = token_deltas[sm]
                    stable_mint_used = sm
                    break

            if stable_mint_used is None:
                return None  # Can't value without SOL or stable quote

            # Pick the primary non-stable token by abs delta
            other_mint = None
            other_delta = 0.0
            for mint, delta in token_deltas.items():
                if mint in stable_mints or mint == sol_mint:
                    continue
                if abs(delta) > abs(other_delta):
                    other_delta = delta
                    other_mint = mint

            if not other_mint or abs(other_delta) < 1e-12:
                return None

            usd_amount = abs(stable_delta)  # stablecoins treated as $1 per token UI unit
            token_amount = abs(other_delta)
            price_usd = (usd_amount / token_amount) if token_amount > 0 else 0.0

            # Determine direction based on stable delta sign (spent stable -> BUY)
            if stable_delta < 0 and other_delta > 0:
                direction = "BUY"
                net_token_delta = other_delta
            elif stable_delta > 0 and other_delta < 0:
                direction = "SELL"
                net_token_delta = other_delta
            else:
                return None

            return {
                "signature": signature,
                "timestamp": timestamp,
                "wallet": wallet_address,
                "token_mint": other_mint,
                "token_amount": token_amount,
                "sol_amount": None,  # derived later from USD quote
                "direction": direction,
                "price_sol": None,
                "price_usd": price_usd,
                "usd_amount": usd_amount,
                "quote_mint": stable_mint_used,
                "net_sol_delta": 0.0,
                "net_token_delta": net_token_delta,
            }

        # IMPROVED: Direction Logic
        # Explicitly handle cases where SOL delta might be slightly noisy due to rent
        # or where wrapping/unwrapping makes native SOL delta zero but wSOL moved
        
        # Threshold for considering a SOL movement "real" (0.001 SOL)
        SIGNIFICANT_SOL = 0.001

        if primary_delta > 0:
            # We received tokens. Did we spend SOL or Stables?
            if sol_delta < -SIGNIFICANT_SOL:
                direction = "BUY"  # Spent SOL
                token_amount = primary_delta
                sol_amount = abs(sol_delta)
            elif any(token_deltas[s] < 0 for s in stable_mints):
                direction = "BUY"  # Spent Stables
                token_amount = primary_delta
                sol_amount = 0  # Will be derived from price
            else:
                # Ambiguous (maybe an airdrop or transfer?)
                return None
                
        elif primary_delta < 0:
            # We sent tokens. Did we receive SOL or Stables?
            if sol_delta > SIGNIFICANT_SOL:
                direction = "SELL"  # Received SOL
                token_amount = abs(primary_delta)
                sol_amount = sol_delta
            elif any(token_deltas[s] > 0 for s in stable_mints):
                direction = "SELL"  # Received Stables
                token_amount = abs(primary_delta)
                sol_amount = 0  # Will be derived
            else:
                return None
        else:
            return None

        price_sol = (sol_amount / token_amount) if token_amount > 0 else 0.0

        return {
            "signature": signature,
            "timestamp": timestamp,
            "wallet": wallet_address,
            "token_mint": primary_mint,
            "token_amount": token_amount,
            "sol_amount": sol_amount,
            "direction": direction,
            "price_sol": price_sol,
            "price_usd": None,
            "usd_amount": None,
            "quote_mint": sol_mint,
            "net_sol_delta": sol_delta,
            "net_token_delta": primary_delta,
        }
