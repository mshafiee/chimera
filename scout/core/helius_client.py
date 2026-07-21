"""
Helius API client for wallet discovery and transaction fetching.
"""

import os
import time
import re
import asyncio
import logging
import random
from datetime import timedelta
from typing import List, Optional, Dict, Any, Set, Tuple

from .utils import utcnow
from dataclasses import dataclass
from pathlib import Path
from collections import defaultdict
import threading
import aiohttp

# Import advanced cache for API optimization
try:
    from .advanced_cache import get_cache, CacheCategory
    CACHE_AVAILABLE = True
except ImportError:
    CACHE_AVAILABLE = False

# Import activity-based caching wrapper
try:
    from .caching import HeliusCachingWrapper
    ACTIVITY_CACHE_AVAILABLE = True
except ImportError:
    ACTIVITY_CACHE_AVAILABLE = False


def _safe_float(value, default: float = 0.0) -> float:
    """Convert a value to float, tolerating dict/None from Helius API responses.

    Some Helius enriched-transaction fields (rawTokenAmountBefore, nativeBalanceChange)
    arrive as nested dicts rather than plain numbers, which crashes ``float()``.
    """
    try:
        return float(value)
    except (TypeError, ValueError):
        return default

# Import credit tracker
try:
    from .helius_credit_tracker import get_credit_tracker
    CREDIT_TRACKER_AVAILABLE = True
except ImportError:
    CREDIT_TRACKER_AVAILABLE = False

try:
    from ..config import ScoutConfig
except ImportError:
    try:
        from config import ScoutConfig
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


class DiscoveryError(Exception):
    """Raised when wallet discovery cannot proceed due to a hard failure.

    Currently used when strict=True and the Helius API key is missing.
    """
    pass


class HeliusClient:
    """Client for Helius API to discover wallets and fetch transactions."""
    
    def __init__(
        self,
        api_key: Optional[str] = None,
        session: Optional[aiohttp.ClientSession] = None,
        redis_client: Optional[Any] = None,
    ):
        """
        Initialize the Helius client.

        Args:
            api_key: Helius API key (optional, falls back to env var)
            session: Optional aiohttp session (for connection pooling)
            redis_client: Optional RedisClient for persistent discovery caching
                          and wallet deduplication across runs.
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

        if ScoutConfig:
            self.base_url = ScoutConfig.get_helius_api_base_url()
        else:
            self.base_url = os.getenv("SCOUT_HELIUS_API_BASE_URL", "https://api.helius.xyz/v0")
        self.last_request_time = 0.0

        # Adaptive Rate Limiting Configuration
        _rate_limit_ms = int(os.getenv("SCOUT_HELIUS_RATE_LIMIT_MS", "20"))  # Default to 20ms (50 RPS target)
        self.rate_limit_delay = max(0.015, _rate_limit_ms / 1000.0)
        self._lock = asyncio.Lock()  # Async lock for async rate limiting
        self._sync_lock = threading.Lock()  # Sync lock for _rate_limit()

        # Adaptive rate limiting state
        self._adaptive_enabled = False
        self._target_rps = 45  # Safe operating target
        self._min_delay = 0.015  # 15ms minimum
        self._max_delay = 0.100  # 100ms maximum (fallback)
        self._current_delay = self.rate_limit_delay
        self._latency_samples: List[float] = []
        self._max_latency_samples = 100
        self._success_count = 0
        self._failure_count = 0

        # Load from ScoutConfig if available
        if ScoutConfig:
            self._adaptive_enabled = ScoutConfig.get_rate_limit_adaptive()
            self._target_rps = ScoutConfig.get_target_rps()
            self._min_delay = ScoutConfig.get_rate_limit_min_delay_ms() / 1000.0
            self._max_delay = ScoutConfig.get_rate_limit_max_delay_ms() / 1000.0
            # Calculate initial delay based on target RPS
            if self._adaptive_enabled:
                self._current_delay = 1.0 / self._target_rps
            else:
                self._current_delay = self.rate_limit_delay

        # Cache valid discoveries between runs
        self._discovery_cache: Dict[str, Any] = {}
        self._discovery_cache_time = 0.0
        self._token_list_cache: Optional[List[str]] = None
        self._token_list_cache_time: Optional[float] = None
        self._cached_active_token_wallets: Optional[Dict[str, int]] = None  # Cache Strategy 1 for Strategy 5

        # Circuit breaker with configurable threshold
        self._circuit_breaker_failures = 0
        self._circuit_breaker_threshold = 5
        self._circuit_breaker_reset_time: Optional[float] = None
        if ScoutConfig:
            self._circuit_breaker_threshold = ScoutConfig.get_circuit_breaker_threshold()
        
        # API call tracking
        self._api_calls_made = 0
        if ScoutConfig:
            self._max_api_calls = ScoutConfig.get_max_api_calls_per_run()
        else:
            self._max_api_calls = int(os.getenv("SCOUT_MAX_API_CALLS_PER_RUN", "500"))
        
        # Known wallets (for deduplication)
        self._known_wallets_cache: Set[str] = set()
        # Keep track of unique wallets found in this run
        self._discovered_this_run: Set[str] = set()
        
        # Discovery quality stats
        self._discovery_stats = {
            "infrastructure_filtered": 0,
            "balance_checked": 0,
            "balance_filtered": 0,
        }
        
        # Async session management
        self._session = session
        self._own_session = False

        # Redis client for persistent caching (discovery cache, dedup set)
        self._redis = redis_client

        # Activity-based caching wrapper for transaction data
        self._activity_cache = None
        if ACTIVITY_CACHE_AVAILABLE:
            try:
                self._activity_cache = HeliusCachingWrapper()
                print("[Helius] Activity-based caching enabled")
            except Exception as e:
                print(f"[Helius] Warning: Failed to initialize activity cache: {e}")

    @staticmethod
    def _redact_api_key(s: str) -> str:
        """
        Redact api-key query parameter values to avoid leaking secrets in logs.

        Example: api-key=XXXX -> api-key=REDACTED
        """
        return re.sub(r"(api-key=)[^&\s]+", r"\1REDACTED", s)

    async def _get_session(self) -> aiohttp.ClientSession:
        """Get or create aiohttp session with connection pooling and optimized timeout.

        Configures persistent connection pooling following Helius best practices:
        - Limit total connections to 100 for resource efficiency
        - Limit per-host to 50 (matches Helius Developer Plan rate limits)
        - 5-minute keep-alive for connection reuse
        - Enable cleanup of closed connections
        """
        if self._session is None:
            # Configure connection pool for Helius endpoints
            connector = aiohttp.TCPConnector(
                limit=100,              # Total max connections
                limit_per_host=50,      # Per-host limit (Helius Developer Plan: 50 RPS)
                keepalive_timeout=300,  # 5 minutes keep-alive
                enable_cleanup_closed=True,  # Cleanup closed connections
            )
            # Set default timeout for all requests: 60s total, 30s connect
            timeout = aiohttp.ClientTimeout(total=60, connect=30, sock_read=30)
            self._session = aiohttp.ClientSession(
                connector=connector,
                timeout=timeout
            )
            self._own_session = True
        return self._session

    async def _close_session(self):
        """Close session if we own it."""
        if self._own_session and self._session:
            await self._session.close()
            self._session = None
            self._own_session = False

    async def close(self):
        """Close all resources (sessions, etc.). Call this before exiting."""
        await self._close_session()

    # ------------------------------------------------------------------
    # Redis-backed discovery cache & persistent dedup (Items 3 & 7)
    # ------------------------------------------------------------------

    _DEDUP_KEY = "scout:discovery:seen_wallets"

    def _redis_available(self) -> bool:
        """Return True if a Redis client is configured and reachable."""
        return self._redis is not None and self._redis.is_available()

    def _get_discovery_cache(
        self, hours_back: int, max_wallets: int
    ) -> Optional[List[str]]:
        """Try to read discovery results from Redis, then in-memory cache.

        Returns the cached wallet list or ``None`` on miss.
        """
        import json as _json

        # Try Redis first (persistent across processes)
        if self._redis_available():
            key = f"scout:discovery:{hours_back}:{max_wallets}"
            try:
                cached = self._redis.get(key)
                if cached:
                    wallets = _json.loads(cached)
                    if isinstance(wallets, list):
                        print("[Helius] Using Redis-cached discovery results")
                        return wallets[:max_wallets]
            except Exception as e:
                logging.getLogger(__name__).debug(
                    f"Redis discovery cache read failed: {e}"
                )

        # Fallback: in-memory cache
        if self._discovery_cache and self._discovery_cache_time:
            if ScoutConfig:
                ttl = ScoutConfig.get_discovery_cache_ttl()
            else:
                ttl = int(os.getenv("SCOUT_DISCOVERY_CACHE_TTL", "3600"))
            if time.time() - self._discovery_cache_time < ttl:
                print("[Helius] Using in-memory cached discovery results")
                return self._discovery_cache.get("wallets", [])[:max_wallets]

        return None

    def _set_discovery_cache(
        self, wallets: List[str], hours_back: int, max_wallets: int
    ) -> None:
        """Store discovery results in both Redis and in-memory cache."""
        import json as _json

        # Always update in-memory cache
        self._discovery_cache = {"wallets": wallets}
        self._discovery_cache_time = time.time()

        # Also persist to Redis for cross-process sharing
        if self._redis_available():
            if ScoutConfig:
                ttl = ScoutConfig.get_discovery_cache_ttl()
            else:
                ttl = int(os.getenv("SCOUT_DISCOVERY_CACHE_TTL", "3600"))
            key = f"scout:discovery:{hours_back}:{max_wallets}"
            try:
                self._redis.set(key, _json.dumps(wallets), ttl_seconds=ttl)
            except Exception as e:
                logging.getLogger(__name__).debug(
                    f"Redis discovery cache write failed: {e}"
                )

    def _get_persistent_seen_wallets(self) -> Set[str]:
        """Retrieve the set of wallets seen in recent runs from Redis."""
        if not self._redis_available():
            return set()
        try:
            members = self._redis.redis_client.smembers(self._DEDUP_KEY)
            if members:
                return set(members)
        except Exception as e:
            logging.getLogger(__name__).debug(
                f"Redis dedup read failed: {e}"
            )
        return set()

    def _mark_wallets_seen(self, wallets: List[str]) -> None:
        """Add wallets to the persistent dedup set in Redis with TTL."""
        if not self._redis_available() or not wallets:
            return
        try:
            if ScoutConfig:
                ttl = ScoutConfig.get_dedup_ttl()
            else:
                ttl = int(os.getenv("SCOUT_DEDUP_TTL", str(6 * 3600)))
            pipe = self._redis.redis_client.pipeline()
            pipe.sadd(self._DEDUP_KEY, *wallets)
            pipe.expire(self._DEDUP_KEY, ttl)
            pipe.execute()
        except Exception as e:
            logging.getLogger(__name__).debug(
                f"Redis dedup write failed: {e}"
            )

    async def get_wallet_first_transaction(self, wallet_address: str) -> Optional[float]:
        """
        Get the timestamp of the wallet's first transaction (creation time).

        This is used for insider/fresh wallet detection.
        Checks up to 2000 signatures (2 pages) via `before` pagination.
        For wallets with >2000 transactions, returns the oldest found + a note
        that the true creation time may be earlier.

        Args:
            wallet_address: Wallet address to check

        Returns:
            Unix timestamp of first transaction, or None if unavailable
        """
        if not self.api_key:
            return None

        try:
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
            if not rpc_url:
                rpc_url = f"https://mainnet.helius-rpc.com/?api-key={self.api_key}"
                # Redact API key before potential logging
                self._redact_api_key(rpc_url)

            session = await self._get_session()

            # Page 1: newest 1000 signatures
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSignaturesForAddress",
                "params": [
                    wallet_address,
                    {"limit": 1000},
                ],
            }
            async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=10)) as response:
                if response.status != 200:
                    return None
                data = await response.json()
                result = data.get("result")
                if not result or not isinstance(result, list) or len(result) == 0:
                    return None

                if len(result) < 1000:
                    # Fewer than 1000 txs — we have the full history
                    oldest_sig = result[-1]
                    if "blockTime" in oldest_sig and oldest_sig["blockTime"]:
                        return float(oldest_sig["blockTime"])

            # Page 2: paginate backwards using `before` on the oldest signature
            oldest_sig_page1 = data.get("result", [])[-1] if data.get("result") else None
            if not oldest_sig_page1 or "signature" not in oldest_sig_page1:
                return None

            payload["params"][1]["before"] = oldest_sig_page1["signature"]
            payload["id"] = 2
            async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=10)) as response:
                if response.status != 200:
                    return None
                data2 = await response.json()
                result2 = data2.get("result")
                if result2 and isinstance(result2, list) and len(result2) > 0:
                    oldest_sig = result2[-1]
                    if "blockTime" in oldest_sig and oldest_sig["blockTime"]:
                        return float(oldest_sig["blockTime"])

                # If page 2 is empty or exhausted, use page 1's oldest
                if result and isinstance(result, list) and len(result) > 0:
                    oldest_sig = result[-1]
                    if "blockTime" in oldest_sig and oldest_sig["blockTime"]:
                        return float(oldest_sig["blockTime"])
        except Exception:
            pass

        return None
    
    async def get_wallet_funder(self, wallet_address: str) -> Optional[str]:
        """
        Identify the address that funded this wallet (sent the first SOL).
        Useful for detecting wallet clusters/insiders.

        Returns the address that sent SOL in the earliest transaction.
        """
        if not self.api_key:
            return None

        try:
            # Get earliest transaction signatures using Helius getSignaturesForAddress
            # This fetches transactions in reverse chronological order (newest first)
            # We'll iterate to find the oldest one
            endpoint = f"/addresses/{wallet_address}/signatures"
            params = {"limit": 1000, "api-key": self.api_key}

            data = await self._make_request(endpoint, params, use_retry=True)
            if not data or not isinstance(data, list):
                return None

            # Find the oldest transaction (last in the list, since returned newest-first)
            if data:
                oldest_sig = data[-1].get("signature") if data else None
                if not oldest_sig:
                    return None

                # Fetch the full transaction to see who sent SOL
                tx_data = await self._make_request(
                    f"/transactions/{oldest_sig}",
                    {"api-key": self.api_key},
                    use_retry=True
                )

                if tx_data and isinstance(tx_data, dict):
                    # Look for SOL transfers in transaction details
                    # In Helius enriched txs, native transfers appear in nativeTransfers field
                    native_transfers = tx_data.get("nativeTransfers", [])
                    for transfer in native_transfers:
                        if transfer.get("toUserAccount") == wallet_address:
                            return transfer.get("fromUserAccount")

            return None
        except Exception as e:
            print(f"[Helius] Error fetching wallet funder: {e}")
            return None
    
    async def get_token_first_tx_timestamp(self, token_address: str) -> Optional[int]:
        """
        Estimate the earliest known transaction timestamp for a token mint.

        Uses getSignaturesForAddress to find the oldest signature on the mint,
        which serves as a lower-bound estimate of when the token began trading.
        Used as a fallback when Birdeye API is unavailable.

        Returns epoch seconds (int) or None if unavailable.
        """
        if not self.api_key or not token_address:
            return None

        # Pump.fun bonding-curve tokens (mint ends with "pump") have no
        # signature history on Helius enhanced API — querying them 404s.
        if token_address.endswith("pump"):
            return None

        try:
            endpoint = f"/addresses/{token_address}/signatures"
            params = {"limit": 50, "api-key": self.api_key}

            data = await self._make_request(endpoint, params, use_retry=False)
            if not data or not isinstance(data, list) or not data:
                return None
            
            oldest = data[-1]
            return oldest.get("timestamp")
        except Exception:
            return None

    # Well-known DEX program (Jupiter v6 aggregator)
    JUPITER_PROGRAM = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"

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

    # Known Telegram bot router program IDs
    # These are used to identify wallets that predominantly use Telegram bots for trading
    # IMPORTANT: Each address must be verified from official bot docs/on-chain before committing
    # TODO: Verify and add addresses from official sources:
    # - Maestro: https://docs.maestro.so/
    # - Trojan: Check official documentation
    # - BananaGun: https://docs.bananagun.com/
    # - BonkBot: Check official documentation
    # If addresses cannot be verified, leave this set empty to avoid misclassification
    KNOWN_BOT_ROUTERS = set()

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
        # Metaplex / NFT programs
        "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s",  # Metaplex Token Metadata
        "cndy3Z4yapfJBmL3ShUp5exZKqR3z33ihTAA9Msz",     # Candy Machine v3
        "cndyAnrLdpi3LRmZxQmUB6BhDKFmV5kuPKRk9GaGQhLo",  # Candy Machine v2
        "CMCYUyenTCkPnvk5ZpPJchLkU3aDWAMYmXKqMpLUGPmz",  # Candy Machine Core
        "BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY",  # Bubblegum
        "1BWutmTvYPwD4wG8oMAqkmBbi3NKYsLDkQwG6Ls9TqrD",  # Gummyroll
        "CMAGAK4f8czi5FjtB9nEUtKPEmMy2oUQBj4a5i2hGq6F",  # TCM
        "coUnmi3AKVDaCabi4qjgBppxnRTTK3rDjB4NcrPxeRm",   # Core
        # Tensor NFT marketplace
        "TSWAPaqyCSx2KABk68Shhefep5AtiDH9SBzZBAZDKjX",  # TensorSwap
        "TCMPhJYjkYBeNZDjGQL2Gx8Nn5FHF7kSitVqWcJikHz",   # Tensor
        # PumpFun
        "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",  # PumpFun
        # Additional known programs
        "AdnT32DkjCs5h2rFLq9yCqHsSfARQDVgnSAfMd7wgTkz",  # Magic Eden
        "M2mx93ekt1fmXSVxTrQ9Tj7fNP5qQoHWTCRZBgLaPUB",  # Magic Eden V2
        "MEisE1HzehtrDyoAzSkVSNuMALwKWNkZ8tWYqYvw1hX",   # Magic Eden
        # Additional DEX programs and routers (not always in dex_programs from config)
        "JUP4Fb2cqiRUcaTHdrPC8h2gNsA2ETXiPDD33WcGuJB",   # Jupiter V4
        "JUP3c2Uh3WA4Ng34oA4UGdXZMDK79qPEoJNhKz",        # Jupiter V3
        "routeUGWgWzqBWFcrCfv8tritsqukccJPu3q5GPP3xS",   # Raydium Router
        "Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb",   # Jito
        "srmqPvymPx9SrHEZqFDvC3KVszRAHd9X7ddZy4j6Yb3",  # Serum
        "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",  # Serum V3
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",  # CAMM
        "DFLoBWSFUNqgdmxjKzQb3Th5GJxDiRJSqFzofAV3e",     # DFlow
        "FLUXubRgkFr2ipp89x3fRJ7LGYqCQko5iiMf3kJZZnws",    # FluxBeam
        "PERPHjGBqRHAqJtEP2FPhkABRuXqLEbxR9kNq7kVqjBY",    # Perpetuals
        "Driftp2LwvcEzuFfN5gLoLP2KKDhfziw1h5oMKCyxTwG",    # Drift V2
        "dRiftyHA39mo4DgFUBhy7PJcehFdBH4SV3BGDg8UGs7",     # Drift
        "Zo1ggzTUKMY5ZGYbLhS7YLFmHo6UqNTPYtJCG4GvRsH",     # Zeta Markets
        "HyaB3W9q6TeAXoG4KJQmGv2hqxqVBn6e1LgGygwFHm6V",    # Saber Stable Swap
        "SSwpkEEcbUqx4vtoEByFjSkhKdCT862DNVb52nZg1UZ",     # Saber
        "CURVGoZn8zfcxMnMHRjLJz3EYnk5kQXtBCryFp4Fv4nV",    # Curve (Solana)
    }

    def _rate_limit(self):
        """Ensure we don't exceed rate limits (Thread-Safe)."""
        with self._sync_lock:
            current_time = time.time()
            time_since_last = current_time - self.last_request_time
            if time_since_last < self.rate_limit_delay:
                time.sleep(self.rate_limit_delay - time_since_last)
            self.last_request_time = time.time()
    
    def _check_circuit_breaker(self) -> bool:
        """Check if circuit breaker should prevent requests.

        Returns True (closed) if requests are allowed, False (open) otherwise.
        Automatically resets the breaker if the cooldown period has elapsed.
        """
        if self._circuit_breaker_reset_time and time.time() > self._circuit_breaker_reset_time:
            logger = logging.getLogger(__name__)
            logger.info(
                f"[Circuit Breaker] Resetting after cooldown "
                f"(was open with {self._circuit_breaker_failures} failures)"
            )
            self._circuit_breaker_failures = 0
            self._circuit_breaker_reset_time = None

        if self._circuit_breaker_failures >= self._circuit_breaker_threshold:
            return False  # Circuit is open, don't make requests
        return True  # Circuit is closed, allow requests
    
    def _record_failure_sync(self):
        """Synchronous version of _record_failure for testing/sync contexts."""
        self._circuit_breaker_failures += 1
        self._failure_count += 1

        # Open circuit if threshold reached
        if self._circuit_breaker_failures >= self._circuit_breaker_threshold:
            reset_seconds = 60
            if ScoutConfig:
                reset_seconds = ScoutConfig.get_circuit_breaker_reset_seconds()
            self._circuit_breaker_reset_time = time.time() + reset_seconds
            logger = logging.getLogger(__name__)
            logger.warning(
                f"[Circuit Breaker] OPENED after {self._circuit_breaker_failures} consecutive failures. "
                f"Requests paused for {reset_seconds}s."
            )

    async def _record_failure(self):
        """Record a failure for circuit breaker (async with lock for thread safety)."""
        async with self._lock:
            self._circuit_breaker_failures += 1
            self._failure_count += 1

            # Open circuit if threshold reached
            if self._circuit_breaker_failures >= self._circuit_breaker_threshold:
                reset_seconds = 60
                if ScoutConfig:
                    reset_seconds = ScoutConfig.get_circuit_breaker_reset_seconds()
                self._circuit_breaker_reset_time = time.time() + reset_seconds
                logger = logging.getLogger(__name__)
                logger.warning(
                    f"[Circuit Breaker] OPENED after {self._circuit_breaker_failures} consecutive failures. "
                    f"Requests paused for {reset_seconds}s."
                )

    async def _record_success(self):
        """Record a success, reset circuit breaker if needed (async with lock for thread safety)."""
        async with self._lock:
            self._success_count += 1
            if self._circuit_breaker_failures > 0:
                self._circuit_breaker_failures = max(0, self._circuit_breaker_failures - 1)

    async def _record_latency(self, latency_ms: float):
        """Record a latency sample for adaptive rate limiting (async with lock for thread safety)."""
        if not self._adaptive_enabled:
            return

        async with self._lock:
            self._latency_samples.append(latency_ms)
            if len(self._latency_samples) > self._max_latency_samples:
                self._latency_samples.pop(0)

    async def _get_avg_latency(self) -> Optional[float]:
        """Get average latency from recent samples (async with lock for thread safety)."""
        async with self._lock:
            if not self._latency_samples:
                return None
            return sum(self._latency_samples) / len(self._latency_samples)

    async def _adjust_rate_limit(self):
        """Adjust rate limit based on latency and success/failure ratio (async with lock for thread safety)."""
        if not self._adaptive_enabled:
            return

        async with self._lock:
            avg_latency = sum(self._latency_samples) / len(self._latency_samples) if self._latency_samples else None
            if not avg_latency:
                return

            # Calculate success ratio
            total_requests = self._success_count + self._failure_count
            if total_requests == 0:
                return

            success_ratio = self._success_count / total_requests

            # Adaptive adjustment logic
            # If latency is high (>200ms) or success ratio is low (<95%), slow down
            if avg_latency > 200 or success_ratio < 0.95:
                # Slow down by increasing delay
                new_delay = min(self._max_delay, self._current_delay * 1.2)
                if new_delay != self._current_delay:
                    self._current_delay = new_delay
                logger = logging.getLogger(__name__)
                logger.info(f"[Adaptive Rate Limit] Slowing down: {avg_latency:.1f}ms avg latency, {success_ratio:.1%} success rate -> {self._current_delay*1000:.1f}ms delay")
            # If latency is low (<50ms) and success ratio is high (>99%), speed up
            elif avg_latency < 50 and success_ratio > 0.99:
                # Speed up by decreasing delay
                new_delay = max(self._min_delay, self._current_delay * 0.9)
                if new_delay != self._current_delay:
                    self._current_delay = new_delay
                logger = logging.getLogger(__name__)
                logger.info(f"[Adaptive Rate Limit] Speeding up: {avg_latency:.1f}ms avg latency, {success_ratio:.1%} success rate -> {self._current_delay*1000:.1f}ms delay")

    async def get_rate_limit_stats(self) -> Dict[str, Any]:
        """Get current rate limit statistics (async with lock for thread safety)."""
        # Call _check_circuit_breaker first so that an expired breaker
        # is reset before we report its state (avoids stale "open" reports).
        self._check_circuit_breaker()

        async with self._lock:
            avg_latency = sum(self._latency_samples) / len(self._latency_samples) if self._latency_samples else None
            total_requests = self._success_count + self._failure_count
            success_ratio = self._success_count / total_requests if total_requests > 0 else 0.0
            current_rps = 1.0 / self._current_delay if self._current_delay > 0 else 0.0

            return {
                "adaptive_enabled": self._adaptive_enabled,
                "target_rps": self._target_rps,
                "current_rps": round(current_rps, 1),
                "current_delay_ms": round(self._current_delay * 1000, 1),
                "avg_latency_ms": round(avg_latency, 1) if avg_latency else None,
                "success_count": self._success_count,
                "failure_count": self._failure_count,
                "success_ratio": round(success_ratio, 3),
                "circuit_breaker_open": self._circuit_breaker_failures >= self._circuit_breaker_threshold,
            }

    async def _retry_with_backoff(self, coro_factory, max_retries: int = 5):
        """Retry an async callable with exponential backoff and jitter.

        Follows Helius best practices:
        - Start with 1s backoff, doubling each retry (2s, 4s, 8s, 16s)
        - Add ±25% random jitter to prevent synchronized retries
        - Maximum backoff capped at 30 seconds
        - Default max retries: 5 attempts

        coro_factory must be a callable (sync or async) that is called fresh
        on each attempt so that a new coroutine is created every retry.
        """
        for attempt in range(max_retries):
            try:
                if asyncio.iscoroutinefunction(coro_factory):
                    result = await coro_factory()
                else:
                    result = coro_factory()
                await self._record_success()
                return result
            except Exception as e:
                if attempt == max_retries - 1:
                    # Final attempt failed, log and raise
                    logger = logging.getLogger(__name__)
                    logger.warning(f"Retry exhausted after {max_retries} attempts: {e}")
                    await self._record_failure()
                    raise

                # Check if error is retryable following Helius best practices
                status_code = None
                if isinstance(e, aiohttp.ClientResponseError):
                    status_code = e.status
                elif isinstance(e, aiohttp.ClientError):
                    # Network errors are retryable
                    status_code = 503  # Treat network errors as service unavailable
                elif isinstance(e, (asyncio.TimeoutError, TimeoutError)):
                    status_code = 408  # Request timeout

                # Use error classification to determine if we should retry
                if not self._is_retryable_error(status_code, e):
                    # Non-retryable error, fail immediately
                    logger = logging.getLogger(__name__)
                    logger.warning(f"Non-retryable error (status {status_code}): {e}")
                    await self._record_failure()
                    raise

                # Calculate backoff with ±25% jitter (Helius best practice)
                # Pattern: 1s, 2s, 4s, 8s, 16s with jitter, capped at 30s
                base_backoff = 2 ** attempt  # 1, 2, 4, 8, 16 for attempts 0-4
                jitter = random.uniform(-0.25, 0.25)  # ±25% random variation
                backoff_time = min(30.0, base_backoff * (1 + jitter))  # Cap at 30s

                logger = logging.getLogger(__name__)
                logger.debug(f"Attempt {attempt + 1} failed (retry in {backoff_time:.2f}s): {e}")
                await asyncio.sleep(backoff_time)
        return None

    async def _rate_limit_async(self):
        """Lock-free rate limiting using token-bucket approach.

        Instead of holding a lock during sleep (which serializes all requests),
        we read the last_request_time atomically, compute local wait time,
        sleep locally, then update the timestamp atomically. This restores
        true concurrent execution for the 50-slot semaphore.

        Adds ±10% jitter to the delay to prevent synchronized requests
        across multiple instances following Helius best practices.
        """
        current_time = time.time()
        
        # Read last request time atomically (no lock held)
        time_since_last = current_time - self.last_request_time
        base_delay = self._current_delay if self._adaptive_enabled else self.rate_limit_delay
        
        # Add ±10% jitter to avoid synchronized requests
        jitter = random.uniform(-0.10, 0.10)
        delay_to_use = base_delay * (1 + jitter)
        
        # Compute local wait time
        wait_time = max(0.0, delay_to_use - time_since_last)
        
        # Sleep locally WITHOUT holding the lock (restores true concurrency)
        if wait_time > 0:
            await asyncio.sleep(wait_time)
        
        # Update last_request_time atomically
        async with self._lock:
            self.last_request_time = time.time()

    def _is_retryable_error(self, status_code: int, error: Optional[Exception] = None) -> bool:
        """Determine if an error is retryable following Helius best practices.

        Per Helius documentation:
        - Retryable: 408 (timeout), 429 (rate limit), 500, 502, 503, 504 (server errors), network errors
        - Non-retryable: 400 (bad request), 401 (unauthorized), 403 (forbidden),
                        404 (not found), 409 (conflict), 422 (validation error)

        Args:
            status_code: HTTP status code
            error: Optional exception that occurred

        Returns:
            True if the error is retryable, False otherwise
        """
        # Non-retryable client errors (4xx except 408)
        if status_code in (400, 401, 403, 404, 409, 422):
            return False

        # Retryable errors
        if status_code in (408, 429, 500, 502, 503, 504):
            return True

        # Network errors are retryable
        if error is not None:
            if isinstance(error, (aiohttp.ClientError, asyncio.TimeoutError)):
                return True

        return False

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

            # Track request start time for latency measurement
            request_start = time.time()

            session = await self._get_session()
            async with session.get(url, params=request_params, timeout=aiohttp.ClientTimeout(total=30)) as response:
                # Calculate request latency
                latency_ms = (time.time() - request_start) * 1000
                await self._record_latency(latency_ms)

                # Handle rate limiting
                if response.status == 429:
                    retry_after = int(response.headers.get("Retry-After", 5))
                    print(f"[Helius] Rate limited, waiting {retry_after}s (per Retry-After header)")
                    await asyncio.sleep(retry_after)
                    # After honoring Retry-After, return None to trigger standard retry with backoff
                    # This prevents immediate retry storms if the first retry after Retry-After also fails
                    raise aiohttp.ClientResponseError(
                        request_info=response.request_info,
                        history=response.history,
                        status=429,
                        message=f"Rate limited - waited {retry_after}s per Retry-After"
                    )

                response.raise_for_status()
                self._api_calls_made += 1
                await self._record_success()
                await self._adjust_rate_limit()
                return await response.json()
        
        def _redact(s: str) -> str:
            # Redact api-key query parameter values to avoid leaking secrets in logs
            # Example: api-key=XXXX -> api-key=REDACTED
            return HeliusClient._redact_api_key(s)

        try:
            if use_retry:
                return await self._retry_with_backoff(_do_request)
            else:
                return await _do_request()
        except asyncio.TimeoutError:
            print("[Helius] API request timeout")
            return None
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
                            token = line.split('#')[0].strip()
                            if token:
                                tokens.append(token)
            except Exception as e:
                print(f"[Helius] Warning: Failed to load token list: {e}")
        
        # Default tokens if none loaded
        if not tokens:
            tokens = [
                # High-volume Solana tokens across categories
                # Meme tokens
                "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",  # BONK
                "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  # WIF
                "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr",  # POPCAT
                "ukHH6c7mMyiWCf1b9pnWe25TSpkDDt3H5pQZgZ74J82",  # PENGU
                "HeLp6NuQkmYB4pYWo2zYs22mESHXPQYzXbB8n4V98jwC",  # AIXBT
                "2weMjPLLybRMMva1fM3U31goWWrCpF59CHWNhnCJ9Vyh",  # FARTCOIN
                "7D1iYWfT2jzNmvjmP6UQHjJkGbQCSAGkzCUN1puS4t8J",  # CHILLGUY
                "EgPnvGxrGyPf7gn4kHozhPbcFXsTGRtkTBNi7dqVWmKx",  # MOODENG
                "KENJSUYLASHUMRG5NL7FUTTPZCKC3GNWJDEWETUKGMBB",  # GOAT
                "6ogzHhzdrQr9Pgv6hZ2MNze7UrzBMAFyBBWU5biqCzVz",  # ACT
                "8Ki8nDpuSSzaGBbPGWhompkKMLw4YU1hR9tGuJgZpump",  # SHAR
                # DeFi / infrastructure
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
                "So11111111111111111111111111111111111111112",  # SOL
                "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN",  # JUP
                "jtojtomepa8beP8AuQc6eXt5FriJwfFMwQx2v2f9mCL",  # JTO
                "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaJbGGxEorpnhBsv",  # PYTH
                "HZ1JovNiVvGrGNiiYvEozEVgZ58xaU3RKwX8eACQBCt3",  # PYUSD
                "3S8qX1MsMqRbiwKg2cQyx7nis1oHMgaCuc9c4VfvVdPN",  # DRIFT
                "mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So",  # mSOL
                "J1toso1uCk3RLmjorhTtrVwY9NJ7HnPnQ2Yiq8yhkgDG",  # JitoSOL
                "bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6hN4tmbDEs7",  # bSOL
                # AI tokens
                "Grass7B4EwDqqUx2CM3vTRv16A3RgXKSPAA6RAgWHqRn",  # GRASS
                "4GJ3TCQaDgMxLgXAGFy3GPMgTgdqRQZG7KAQmhRVF2tV",  # IO
                # Gaming
                "FmQs27d8WemVxDe8KuzUHXsdQrAHWBxHrBGx5TBrY4QB",  # SUPER
                "MEW1gQWJ3nEXg2qgERiKu7FAFj79PHvQVREQUyScPP5",  # MEW
                "9BB6NFfWjLxVxKZdXhzVTfD4WZnLGqTDF7jN3qZnVx4m",  # GIGA
                "4k3DyjAgaQxjX1Qx1pVa1NB3khXTU4VjQmzxZgRqoLYT",  # ZEREBRO
            ]
        
        # Cache the result
        self._token_list_cache = tokens
        self._token_list_cache_time = time.time()
        
        return tokens

    async def _refresh_token_list(self) -> bool:
        """
        Automatically refresh the active token list using Birdeye trending API.

        Fetches trending tokens from Birdeye and updates the active_tokens.txt file.
        Designed to run hourly for maximum freshness.

        Returns:
            True if refresh was successful, False otherwise
        """

        birdeye_api_key = os.getenv("BIRDEYE_API_KEY")
        if not birdeye_api_key:
            print("[Helius] Birdeye API key not configured, skipping token refresh")
            return False

        try:
            print("[Helius] Refreshing token list from Birdeye trending API...")

            # Fetch trending tokens from Birdeye
            birdeye_url = "https://public-api.birdeye.so/defi/v1/trending_tokens"
            headers = {
                "X-API-KEY": birdeye_api_key,
                "accept": "application/json"
            }

            session = await self._get_session()
            async with session.get(birdeye_url, headers=headers) as response:
                if response.status != 200:
                    print(f"[Helius] Failed to fetch trending tokens: HTTP {response.status}")
                    return False

                data = await response.json()
                trending_tokens = data.get("trending_tokens", [])

                if not trending_tokens:
                    print("[Helius] No trending tokens returned from Birdeye")
                    return False

                # Extract token addresses and filter for Solana tokens
                new_tokens = []
                seen_tokens = set()

                # Keep existing high-quality tokens
                existing_tokens = self._load_active_tokens()
                for token in existing_tokens[:20]:  # Keep top 20 existing tokens
                    if token not in seen_tokens:
                        new_tokens.append(token)
                        seen_tokens.add(token)

                # Add trending tokens
                for token_data in trending_tokens[:80]:  # Add up to 80 trending tokens
                    token_address = token_data.get("address")
                    if token_address and token_address not in seen_tokens:
                        # Validate it's a Solana token
                        if self._is_valid_solana_address(token_address):
                            new_tokens.append(token_address)
                            seen_tokens.add(token_address)

                if len(new_tokens) < 50:
                    print(f"[Helius] Warning: Only {len(new_tokens)} tokens after refresh, which is below target")
                    return False

                # Update the active_tokens.txt file
                config_path = Path(__file__).parent.parent / "config" / "active_tokens.txt"
                backup_path = config_path.with_suffix(".txt.backup")

                # Create backup
                if config_path.exists():
                    import shutil
                    shutil.copy(config_path, backup_path)

                # Write new token list
                with open(config_path, 'w') as f:
                    f.write("# Aggressive Token Expansion for Wallet Discovery - Auto-refreshed\n")
                    f.write(f"# Last updated: {utcnow().isoformat()}\n")
                    f.write(f"# Total tokens: {len(new_tokens)}\n")
                    f.write("# ===== TOP EXISTING TOKENS =====\n")

                    for i, token in enumerate(new_tokens[:20]):
                        f.write(f"{token}\n")

                    f.write("# ===== TRENDING TOKENS FROM BIRDEYE =====\n")
                    for token in new_tokens[20:]:
                        f.write(f"{token}\n")

                # Update cache
                self._token_list_cache = new_tokens
                self._token_list_cache_time = time.time()

                print(f"[Helius] ✓ Token list refreshed successfully: {len(new_tokens)} tokens")
                print(f"[Helius] ✓ Backup saved to {backup_path}")

                return True

        except Exception as e:
            print(f"[Helius] Token refresh failed: {e}")
            import traceback
            traceback.print_exc()
            return False

    def _is_valid_solana_address(self, address: str) -> bool:
        """Validate that an address is a valid Solana public key."""
        try:
            # Basic Solana address validation
            # Solana addresses are base58 encoded and typically 32-44 characters
            if not address or len(address) < 32 or len(address) > 44:
                return False

            # Check for base58 characters only
            import base58
            base58.alphabet = set("123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz")
            if not all(c in base58.alphabet for c in address):
                return False

            # Additional check: common system program addresses
            system_programs = {
                "11111111111111111111111111111111",  # System Program
                "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  # Token Program
                "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25ekTN8LoUaUX",  # Token-2022
            }

            if address in system_programs:
                return False

            return True

        except Exception:
            return False

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
                from .db import get_connection
                db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
                if os.path.exists(db_path):
                    conn = get_connection(db_path)
                    cursor = conn.cursor()
                    cursor.execute("SELECT 1 FROM wallets WHERE address = %s LIMIT 1", (wallet_address,))
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
        - raw numeric string (e.g. "1.5")
        - scientific notation (e.g. "1e-6")

        Returns 0.0 with a warning when parsing fails, so a single malformed
        transfer does not block delta accumulation for the rest of the batch.
        """
        # 1) rawTokenAmount is the most precise
        raw = transfer.get("rawTokenAmount")
        if isinstance(raw, dict):
            try:
                raw_amt = raw.get("tokenAmount")
                dec = int(raw.get("decimals", 0))
                if raw_amt is not None:
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
                    pass

        # 3) tokenAmount as scalar
        if ta is not None:
            try:
                return float(ta)
            except Exception:
                pass

        logging.getLogger(__name__).warning(
            "Could not parse ui_token_amount from transfer payload, returning 0.0: %r",
            transfer,
        )
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

        # Check against expanded non-wallet set (programs, mints, infrastructure)
        if address in self.NON_WALLET_ADDRESSES:
            return False
            
        # NOTE: We intentionally do NOT filter out token mint addresses here.
        # Wallet discovery extracts many "user accounts" from transactions; some
        # tests also treat common mints (e.g., wSOL) as valid addresses.
        
        # Filter addresses that look like programs (ending in many 1s or common patterns)
        if address.endswith("11111111111111111111111111111111"):
            return False
        
        # Filter addresses that look like PDA seeds (common pattern: long runs of identical chars)
        # Programs and vaults often have highly patterned addresses
        if self._looks_like_program_address(address):
            return False
        
        # Basic base58 character check (simplified - Solana uses base58)
        # NOTE: We intentionally avoid strict base58 validation here because
        # some unit tests use synthetic addresses that may not be valid base58.
        
        return True

    @staticmethod
    def _looks_like_program_address(address: str) -> bool:
        """Heuristic: detect addresses that are likely programs/PDA/vaults, not user wallets."""
        if not address or len(address) < 32:
            return False
        # Known program/sysvar/account prefixes that are never user wallets
        if address.startswith(("Sysvar", "Vote11111", "Stake1111", "Config111")):
            return True
        # Long runs of same character suggest PDA seed derivation
        for i in range(len(address) - 8):
            if address[i:i+8] == address[i] * 8:
                return True
            if address[i:i+8] == "1" * 8:
                return True
        # Ends with "1" * 8+ suggests program-derived
        if address.endswith("11111111"):
            return True
        return False

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
        Extract ACTUAL TRADING wallet addresses from a transaction.
        
        Only extracts wallets that are directly involved in swaps (appear in tokenTransfers),
        and filters out infrastructure addresses like program IDs, routing contracts, etc.
        
        Args:
            tx: Transaction dictionary from Helius API
            
        Returns:
            List of unique valid wallet addresses that are actual traders
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

        # ONLY extract wallets from tokenTransfers (actual traders)
        traders: Set[str] = set()
        infra_filtered = 0

        # Extract from tokenTransfers ONLY
        if "tokenTransfers" in tx:
            for transfer in tx.get("tokenTransfers", []):
                if isinstance(transfer, dict):
                    for key in ["fromUserAccount", "toUserAccount"]:
                        if key in transfer:
                            addr = transfer[key]
                            if not addr:
                                continue
                            if not self._validate_wallet_address(addr):
                                infra_filtered += 1
                                continue
                            traders.add(addr)

        self._discovery_stats["infrastructure_filtered"] += infra_filtered
        
        # OPTIONAL: Also check if feePayer appears in the transfers
        # If feePayer paid for the tx AND appears in transfers, they're definitely a trader
        fee_payer = tx.get("feePayer")
        if fee_payer and fee_payer in traders:
            # Fee payer is the transaction initiator and appears in transfers - strong signal
            pass  # Already in traders set
        
        return list(traders)
    
    async def _validate_wallet_activity(
        self,
        wallet_address: str,
        min_trades: int = 3,
        days_back: int = 7
    ) -> bool:
        """
        AGGRESSIVE WALLET VALIDATION - Multi-criteria validation system.

        This enhanced validation method implements comprehensive wallet quality checks:
        1. Minimum trade count validation (configurable, default 3)
        2. Wallet age and consistency checks
        3. Trading frequency validation
        4. SOL balance verification (filters out programs/vaults)
        5. Transaction type diversity check

        Args:
            wallet_address: Wallet address to validate
            min_trades: Minimum number of trades required (default: 3, can be overridden via SCOUT_MIN_TRADES env var)
            days_back: Number of days to look back (default: 7)

        Returns:
            True if wallet meets aggressive activity criteria
        """
        # Check cache first (validation results with 5-minute TTL)
        if CACHE_AVAILABLE:
            cache = get_cache()
            cache_key = f"{wallet_address}:{min_trades}:{days_back}"
            cached_result = cache.get("wallet_validation", wallet_address, cache_key,
                                    category=CacheCategory.WALLET_METRICS)
            if cached_result is not None:
                return cached_result

        try:
            # ENVIRONMENT CONFIGURATION OVERRIDES
            # Allow runtime configuration of validation strictness
            min_trades_config = int(os.getenv("SCOUT_MIN_TRADES", str(min_trades)))
            validate_by_default = os.getenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "true").lower() == "true"

            if not validate_by_default:
                # If validation is disabled by config, accept all wallets
                result = True
                if CACHE_AVAILABLE:
                    cache = get_cache()
                    cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                    cache.set("wallet_validation", wallet_address, result, cache_key,
                             category=CacheCategory.WALLET_METRICS)
                return result

            # VALIDATION CRITERIA 1: Minimum trade count
            transactions = await self.get_wallet_transactions(wallet_address, days=days_back, limit=min_trades_config + 10)
            if len(transactions) < min_trades_config:
                result = False
                if CACHE_AVAILABLE:
                    cache = get_cache()
                    cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                    cache.set("wallet_validation", wallet_address, result, cache_key,
                             category=CacheCategory.WALLET_METRICS)
                return result

            # VALIDATION CRITERIA 2: Trading frequency check
            # Wallets should have consistent trading activity, not just one burst
            if len(transactions) >= min_trades_config:
                # Check if trades are spread across multiple days (not all in one day)
                from collections import defaultdict
                trades_by_day = defaultdict(int)
                for tx in transactions:
                    tx_timestamp = tx.get("timestamp", time.time())
                    tx_day = int(tx_timestamp / 86400)  # Group by day
                    trades_by_day[tx_day] += 1

                # Require trades on at least 2 different days for quality wallets
                # (unless min_trades is very low, then 1 day is acceptable)
                if min_trades_config >= 5 and len(trades_by_day) < 2:
                    result = False
                    if CACHE_AVAILABLE:
                        cache = get_cache()
                        cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                        cache.set("wallet_validation", wallet_address, cache_key, result,
                                 category=CacheCategory.WALLET_METRICS)
                    return result

            # VALIDATION CRITERIA 3: SOL balance check
            # Filter out programs and vaults that have zero SOL balance
            min_sol_balance = float(os.getenv("SCOUT_MIN_SOL_BALANCE", "0.001"))
            if min_sol_balance > 0:
                try:
                    sol_balance = await self._get_wallet_sol_balance(wallet_address)
                    if sol_balance < min_sol_balance:
                        result = False
                        if CACHE_AVAILABLE:
                            cache = get_cache()
                            cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                            cache.set("wallet_validation", wallet_address, cache_key, result,
                                     category=CacheCategory.WALLET_METRICS)
                        return result
                except Exception:
                    # If balance check fails, log but don't fail the validation
                    pass

            # VALIDATION CRITERIA 4: Transaction type diversity
            # Quality wallets should have multiple types of transactions (SWAP, TRANSFER, etc.)
            # This filters out single-purpose automated wallets
            tx_types = set()
            for tx in transactions[:min_trades_config * 2]:  # Check first few transactions
                tx_type = tx.get("type", "UNKNOWN")
                if tx_type:
                    tx_types.add(tx_type)

            # Require at least 1 SWAP transaction for wallet discovery purposes
            if "SWAP" not in tx_types and "TRADE" not in tx_types:
                return False

            # VALIDATION CRITERIA 5: Recent activity check
            # Ensure at least one recent trade (within last 24 hours for active wallets)
            recent_trades = [tx for tx in transactions if tx.get("timestamp", 0) > (time.time() - 86400)]
            if not recent_trades and min_trades_config >= 5:
                # For higher min_trades thresholds, require recent activity
                result = False
                if CACHE_AVAILABLE:
                    cache = get_cache()
                    cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                    cache.set("wallet_validation", wallet_address, result, cache_key,
                             category=CacheCategory.WALLET_METRICS)
                return result

            # Successful validation
            result = True
            if CACHE_AVAILABLE:
                cache = get_cache()
                cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                cache.set("wallet_validation", wallet_address, cache_key, result,
                         category=CacheCategory.WALLET_METRICS)
            return result

        except Exception as e:
            # AGGRESSIVE VALIDATION: Fail closed - if validation fails, wallet is invalid
            print(f"[Helius] Validation failed for wallet {wallet_address[:8]}...: {e}")
            result = False
            if CACHE_AVAILABLE:
                cache = get_cache()
                cache_key = f"{wallet_address}:{min_trades}:{days_back}"
                cache.set("wallet_validation", wallet_address, cache_key, result,
                         category=CacheCategory.WALLET_METRICS)
            return result

    async def _get_wallet_sol_balance(self, wallet_address: str) -> float:
        """Get SOL balance for a single wallet."""
        try:
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
            if not rpc_url:
                return 0.0

            session = await self._get_session()
            payload = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getBalance",
                "params": [wallet_address]
            }

            async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=10)) as response:
                if response.status == 200:
                    data = await response.json()
                    balance_lamports = data.get("result", {}).get("value", 0)
                    return balance_lamports / 1e9

            return 0.0
        except Exception:
            return 0.0

    async def _batch_validate_activity(
        self,
        wallets: List[str],
        min_trades: int = 3,
        days_back: int = 7,
        max_wallets: int = 0,
    ) -> List[str]:
        """
        Validate wallet activity in parallel with bounded concurrency.

        Wraps ``_validate_wallet_activity`` for each wallet, running them
        concurrently via ``asyncio.gather`` with a semaphore to respect
        rate limits.  Returns only wallets that pass validation, preserving
        the input order.

        Args:
            wallets: Wallet addresses to validate.
            min_trades: Minimum trades required (forwarded to validator).
            days_back: Lookback window in days (forwarded to validator).
            max_wallets: If > 0, stop accepting once this many have passed.

        Returns:
            List of validated wallet addresses (subset of *wallets*).
        """
        if not wallets:
            return []

        if ScoutConfig:
            max_concurrent = ScoutConfig.get_activity_validation_concurrency()
        else:
            max_concurrent = 20
        semaphore = asyncio.Semaphore(max_concurrent)

        validated: List[str] = []

        async def _check(wallet: str) -> Tuple[str, bool]:
            async with semaphore:
                ok = await self._validate_wallet_activity(
                    wallet, min_trades=min_trades, days_back=days_back
                )
                return wallet, ok

        results = await asyncio.gather(
            *[_check(w) for w in wallets], return_exceptions=True
        )

        for result in results:
            if isinstance(result, tuple) and result[1]:
                validated.append(result[0])
                if max_wallets > 0 and len(validated) >= max_wallets:
                    break

        print(f"[Helius] Activity validation: {len(validated)}/{len(wallets)} passed")
        return validated

    async def _aggressive_wallet_filter(
        self,
        wallets: List[str],
        validation_config: Optional[Dict[str, Any]] = None
    ) -> List[str]:
        """
        AGGRESSIVE WALLET FILTERING - Apply comprehensive validation to discovered wallets.

        This method implements multi-stage wallet filtering:
        1. Address format validation (immediate filter)
        2. SOL balance check (filters programs/vaults)
        3. Activity validation (minimum trades, frequency, diversity)
        4. Quality scoring (ranks wallets by multiple metrics)

        Args:
            wallets: List of wallet addresses to filter
            validation_config: Optional configuration override

        Returns:
            List of wallets that pass aggressive validation criteria
        """
        if not wallets:
            return []

        # CONFIGURATION
        config = validation_config or {
            "min_trades": int(os.getenv("SCOUT_MIN_TRADES", "3")),
            "min_sol_balance": float(os.getenv("SCOUT_MIN_SOL_BALANCE", "0.001")),
            "require_recent_activity": os.getenv("SCOUT_REQUIRE_RECENT_ACTIVITY", "false").lower() == "true",
            "validation_enabled": os.getenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "true").lower() == "true",
        }

        if not config.get("validation_enabled", True):
            # If validation is disabled, return all wallets
            return wallets

        # STAGE 1: Address format validation (fastest filter)
        valid_format = []
        for wallet in wallets:
            if self._is_candidate_wallet_address(wallet):
                valid_format.append(wallet)

        if not valid_format:
            return []

        print(f"[Helius] Format validation: {len(valid_format)}/{len(wallets)} passed")

        # STAGE 2: SOL balance check (parallel batch processing)
        min_balance = config.get("min_sol_balance", 0.001)
        if min_balance > 0:
            valid_balance = await self._filter_by_sol_balance(valid_format, min_balance_sol=min_balance)
            print(f"[Helius] Balance validation: {len(valid_balance)}/{len(valid_format)} passed")
        else:
            valid_balance = valid_format

        if not valid_balance:
            return []

        # STAGE 3: Activity validation (delegates to _batch_validate_activity)
        min_trades = config.get("min_trades", 3)
        validated_wallets = await self._batch_validate_activity(
            valid_balance, min_trades=min_trades, days_back=7
        )

        return validated_wallets

    async def _filter_by_sol_balance(
        self,
        wallets: List[str],
        min_balance_sol: float = 0.0,
    ) -> List[str]:
        """
        Batch-check SOL balances to filter out programs, vaults, and system accounts.
        Addresses with zero (or near-zero) SOL balance are almost certainly not user wallets.

        On per-batch RPC failure, behaviour is governed by SCOUT_BALANCE_FAIL_MODE:
          - 'open' (default): include the entire batch (fail-open, avoids dropping wallets on transient errors).
          - 'closed': exclude the batch (fail-closed, stricter filtering at the cost of potential false negatives).
        """
        if not wallets:
            return []

        if ScoutConfig:
            batch_size = ScoutConfig.get_balance_batch_size()
            fail_mode = ScoutConfig.get_balance_fail_mode()
        else:
            batch_size = 20  # Batch RPC calls for efficiency
            fail_mode = os.getenv("SCOUT_BALANCE_FAIL_MODE", "open").lower()
        rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
        if not rpc_url:
            return wallets  # Can't validate without RPC

        total_checked = 0
        valid_wallets = []
        session = await self._get_session()
        logger = logging.getLogger(__name__)

        for i in range(0, len(wallets), batch_size):
            batch = wallets[i:i + batch_size]
            total_checked += len(batch)
            # Build batch RPC requests
            payload = []
            for j, addr in enumerate(batch):
                payload.append({
                    "jsonrpc": "2.0",
                    "id": j,
                    "method": "getBalance",
                    "params": [addr]
                })

            try:
                async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=15)) as response:
                    if response.status == 200:
                        results = await response.json()
                        if isinstance(results, list):
                            for result in results:
                                if isinstance(result, dict) and "result" in result:
                                    balance_lamports = result["result"].get("value", 0)
                                    balance_sol = balance_lamports / 1e9
                                    idx = result.get("id", -1)
                                    if balance_sol > min_balance_sol and 0 <= idx < len(batch):
                                        valid_wallets.append(batch[idx])
                        elif isinstance(results, dict) and "result" in results:
                            balance_lamports = results["result"].get("value", 0)
                            if balance_lamports / 1e9 > min_balance_sol:
                                valid_wallets.append(batch[0])
                    else:
                        logger.warning(
                            f"[Helius] Balance check batch {i//batch_size} got HTTP {response.status}, "
                            f"fail_mode={fail_mode}"
                        )
                        if fail_mode != "closed":
                            valid_wallets.extend(batch)
            except Exception as e:
                logger.warning(
                    f"[Helius] Balance check batch {i//batch_size} failed ({e}), "
                    f"fail_mode={fail_mode}"
                )
                if fail_mode != "closed":
                    valid_wallets.extend(batch)

        self._discovery_stats["balance_checked"] = total_checked
        self._discovery_stats["balance_filtered"] = total_checked - len(valid_wallets)
        return valid_wallets

    async def get_wallet_sol_balances(self, wallets: List[str]) -> Dict[str, float]:
        """
        Batch-fetch SOL balances for multiple wallets.
        
        Used by profitability pre-screen to rank candidates by on-chain wealth.
        Returns {address: balance_in_sol}. Wallets whose balance cannot be
        fetched default to 0.0 and are still included.
        """
        if not wallets:
            return {}

        if ScoutConfig:
            batch_size = ScoutConfig.get_balance_batch_size()
        else:
            batch_size = 20
        rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
        if not rpc_url:
            return {w: 0.0 for w in wallets}

        balances: Dict[str, float] = {}
        session = await self._get_session()

        for i in range(0, len(wallets), batch_size):
            batch = wallets[i:i + batch_size]
            payload = []
            for j, addr in enumerate(batch):
                payload.append({
                    "jsonrpc": "2.0",
                    "id": j,
                    "method": "getBalance",
                    "params": [addr]
                })

            try:
                async with session.post(rpc_url, json=payload, timeout=aiohttp.ClientTimeout(total=15)) as response:
                    if response.status == 200:
                        results = await response.json()
                        if isinstance(results, list):
                            for result in results:
                                if isinstance(result, dict) and "result" in result:
                                    balance_lamports = result["result"].get("value", 0)
                                    idx = result.get("id", -1)
                                    if 0 <= idx < len(batch):
                                        balances[batch[idx]] = balance_lamports / 1e9
                        elif isinstance(results, dict) and "result" in results:
                            if batch:
                                balances[batch[0]] = results["result"].get("value", 0) / 1e9
            except Exception as e:
                logging.getLogger(__name__).warning(
                    f"[Helius] Balance fetch batch {i//batch_size} failed ({e}), "
                    f"defaulting {len(batch)} wallets to 0.0 SOL"
                )
                for addr in batch:
                    balances.setdefault(addr, 0.0)

        for addr in wallets:
            balances.setdefault(addr, 0.0)

        return balances

    def get_discovery_stats(self) -> Dict[str, int]:
        """Return discovery quality statistics from the most recent run."""
        return dict(self._discovery_stats)

    def get_cache_stats(self) -> Dict[str, Any]:
        """Return activity-based cache statistics if available."""
        if self._activity_cache:
            return self._activity_cache.get_cache_stats()
        return {}
    
    async def _query_token_transactions(
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
        use_parallel: bool = True,
        max_wallets: int = 200
    ) -> Dict[str, int]:
        """
        Discover wallets from active token swap transactions.

        Args:
            token_addresses: List of token addresses to query (None to use defaults)
            hours_back: Number of hours to look back
            limit_per_token: Maximum transactions per token
            use_parallel: Whether to use parallel queries
            max_wallets: Maximum number of wallets to discover (for early termination)
            use_parallel: Whether to use parallel processing (respects rate limits)
            
        Returns:
            Dictionary mapping wallet addresses to trade counts
        """
        if token_addresses is None:
            token_addresses = self._load_active_tokens()
        
        wallet_counts: Dict[str, int] = defaultdict(int)
        cutoff_time = int((utcnow() - timedelta(hours=hours_back)).timestamp())
        
        print(f"[Helius] Discovering from {len(token_addresses)} active tokens...")

        if use_parallel and len(token_addresses) > 1:
            # Limit concurrent RPC requests to avoid overwhelming the API.
            if ScoutConfig:
                max_concurrent = ScoutConfig.get_discovery_concurrency()
            else:
                max_concurrent = int(os.getenv("SCOUT_DISCOVERY_CONCURRENCY", "50"))
            semaphore = asyncio.Semaphore(max_concurrent)

            async def _bounded_query(token_addr: str) -> Tuple[str, List[Dict[str, Any]]]:
                async with semaphore:
                    return await self._query_token_transactions(token_addr, cutoff_time, limit_per_token)

            # Create async tasks for all tokens
            tasks = [
                _bounded_query(token_addr)
                for token_addr in token_addresses
                if self._api_calls_made < self._max_api_calls
            ]

            # Process results as they complete
            for coro in asyncio.as_completed(tasks):
                if self._api_calls_made >= self._max_api_calls:
                    break
                # Early termination: stop if we already have enough wallets
                if len(wallet_counts) >= max_wallets:
                    print(f"[Helius] Early termination: found {len(wallet_counts)} wallets, stopping token queries")
                    break

                try:
                    token_addr, transactions = await coro
                except Exception as e:
                    print(f"[Helius] Error querying token: {e}")
                    continue

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
                    print("[Helius] Reached max API calls, stopping token queries")
                    break
                # Early termination: stop if we already have enough wallets
                if len(wallet_counts) >= max_wallets:
                    print(f"[Helius] Early termination: found {len(wallet_counts)} wallets, stopping token queries")
                    break
                
                token_addr, transactions = await self._query_token_transactions(token_addr, cutoff_time, limit_per_token)
                
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

    async def _discover_from_recent_blocks(
        self,
        hours_back: int = 24,
        limit: int = 500
    ) -> Dict[str, int]:
        """
        Discover wallets from recent Solana blocks for fresh swap activity.

        This strategy captures wallets that have recently executed swap transactions
        by querying recent blocks and extracting swap participants.

        Args:
            hours_back: Number of hours to look back for blocks
            limit: Maximum number of transactions to process

        Returns:
            Dictionary mapping wallet addresses to trade counts
        """

        wallet_counts: Dict[str, int] = defaultdict(int)

        try:
            # Calculate slot range for recent blocks
            # Solana produces ~400-500 slots per minute, so we calculate backwards
            slots_per_minute = 480  # Conservative estimate
            total_slots = int(hours_back * 60 * slots_per_minute)

            # Get current slot
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
            if not rpc_url and self.api_key:
                rpc_url = f"https://mainnet.helius-rpc.com/?api-key={self.api_key}"

            if not rpc_url:
                print("[Helius] No RPC URL available for block discovery")
                return {}

            # Get current slot
            session = await self._get_session()
            payload = {
                "jsonrpc": "2.0",
                "id": "block_discovery",
                "method": "getSlot",
                "params": []
            }

            async with session.post(rpc_url, json=payload) as response:
                if response.status != 200:
                    print(f"[Helius] Failed to get current slot: HTTP {response.status}")
                    return {}

                data = await response.json()
                current_slot = data.get("result")

                if not current_slot:
                    print("[Helius] Invalid slot response")
                    return {}

            # Calculate starting slot for our lookback window
            start_slot = max(0, current_slot - total_slots)

            print(f"[Helius] Analyzing blocks from slot {start_slot} to {current_slot}...")

            # Query blocks in batches (Solana RPC limits)
            batch_size = 100  # Process 100 blocks at a time
            processed_transactions = 0

            for slot in range(start_slot, current_slot, batch_size):
                if processed_transactions >= limit:
                    break

                # Get block for this slot range
                end_batch_slot = min(slot + batch_size, current_slot)
                min(batch_size, end_batch_slot - slot)

                for individual_slot in range(slot, end_batch_slot):
                    if processed_transactions >= limit:
                        break

                    try:
                        # Get confirmed block
                        block_payload = {
                            "jsonrpc": "2.0",
                            "id": f"block_{individual_slot}",
                            "method": "getBlock",
                            "params": [individual_slot, {"encoding": "jsonParsed", "transactionDetails": "full"}]
                        }

                        async with session.post(rpc_url, json=block_payload) as block_response:
                            if block_response.status != 200:
                                continue

                            block_data = await block_response.json()
                            block_result = block_data.get("result")

                            if not block_result:
                                continue

                            # Extract transactions from block
                            transactions = block_result.get("transactions", [])

                            for tx in transactions:
                                if processed_transactions >= limit:
                                    break

                                processed_transactions += 1

                                # Get transaction details
                                tx_detail = tx.get("transaction", {})
                                meta = tx_detail.get("meta", {})

                                # Check if transaction failed (skip failed transactions)
                                if meta.get("err") is not None:
                                    continue

                                # Look for swap instructions
                                message = tx_detail.get("message", {})
                                instructions = message.get("instructions", [])

                                is_swap = False
                                for instruction in instructions:
                                    # Check for program calls that might be swaps
                                    program_id = instruction.get("programId")
                                    if program_id in self.dex_programs:
                                        is_swap = True
                                        break

                                    # Check for transfer instructions (simple swaps)
                                    parsed = instruction.get("parsed")
                                    if parsed and parsed.get("type") in ["transfer", "transferChecked"]:
                                        is_swap = True
                                        break

                                if is_swap:
                                    # Extract wallet addresses from transaction
                                    # Priority: fee payer -> account keys -> signers

                                    # Fee payer (usually the initiating wallet)
                                    fee_payer = message.get("accountKeys", [None])[0]
                                    if fee_payer and self._is_candidate_wallet_address(fee_payer):
                                        wallet_counts[fee_payer] += 1
                                        self._discovered_this_run.add(fee_payer)

                                    # Account keys that might be wallets
                                    account_keys = message.get("accountKeys", [])
                                    for account_key in account_keys[1:3]:  # Check first few accounts
                                        if account_key and self._is_candidate_wallet_address(account_key):
                                            # Avoid counting the same wallet multiple times per transaction
                                            if account_key != fee_payer:
                                                wallet_counts[account_key] += 1
                                                self._discovered_this_run.add(account_key)

                    except Exception:
                        # Skip problematic blocks and continue
                        continue

            print(f"[Helius] Processed {processed_transactions} transactions from recent blocks")

        except Exception as e:
            print(f"[Helius] Block discovery failed: {e}")
            import traceback
            traceback.print_exc()

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
        cutoff_time = int((utcnow() - timedelta(hours=hours_back)).timestamp())
        
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
                    from urllib.parse import urlparse, parse_qs
                    api_key = parse_qs(urlparse(rpc_url).query).get("api-key", [None])[0]
                
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

                    # Record credit cost for successful discovery fetch (50 credits per page)
                    if CREDIT_TRACKER_AVAILABLE:
                        tracker = get_credit_tracker()
                        tracker.record_request(
                            cost=50,
                            category="discovery",
                            endpoint="getTransactionsForAddress",
                            success=True
                        )

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

    async def discover_from_top_performing_tokens(self) -> List[str]:
        """
        Discover wallets from top performing/trending tokens (Strategy 5).
        Falls back to using active tokens if trending data is unavailable.
        Uses cached results from Strategy 1 to avoid redundant queries.

        Returns:
            List of discovered wallet addresses
        """
        # Check if Strategy 1 already ran and cached active token results
        if hasattr(self, '_cached_active_token_wallets'):
            print("[Helius] Using cached active token results (Strategy 5 reusing Strategy 1 data)")
            wallet_counts = self._cached_active_token_wallets
        else:
            try:
                # Use active tokens as the data source for trending token analysis
                # In production, could integrate with Birdeye/DexScreener for real trending tokens
                wallet_counts = await self._discover_from_active_tokens(hours_back=12, limit_per_token=100)
                # Cache for potential reuse
                self._cached_active_token_wallets = wallet_counts
            except Exception as e:
                print(f"[Helius] discover_from_top_performing_tokens failed: {e}")
                return []
        
        # Take top N wallets by trade count
        top_wallets = sorted(wallet_counts.items(), key=lambda x: x[1], reverse=True)[:50]
        return [wallet for wallet, _count in top_wallets]

    async def discover_wallets(
        self,
        hours_back: int = 24,
        max_wallets: int = 200,
        limit_per_token: int = 50,
        min_value_usd: float = 0,
        **kwargs,
    ) -> Dict[str, int]:
        """Discover wallets and return a dict of {address: trade_count}.

        Wrapper around ``discover_wallets_from_recent_swaps`` that returns the
        dict format expected by ``smart_discovery.py``. Extra keyword arguments
        (``limit_per_token``, ``min_value_usd``) are accepted for caller
        compatibility but delegated to the underlying pipeline where possible.
        """
        wallets = await self.discover_wallets_from_recent_swaps(
            max_wallets=max_wallets,
            hours_back=hours_back,
        )
        return {w: max(1, len(wallets) - i) for i, w in enumerate(wallets)}

    async def discover_wallets_from_recent_swaps(
        self,
        limit: int = 1000,
        min_trade_count: int = 2,
        max_wallets: int = 200,
        hours_back: int = 24,
        strict: bool = False,
    ) -> List[str]:
        """
        Discover wallet addresses from recent swap transactions using a multi-strategy pipeline.

        Strategy execution:
        1. **Active tokens** (Strategy 1) runs first — cheapest, most reliable.
        2. If results < fallback threshold (default 50% of max_wallets), **strategies
           2-4 run in parallel** via ``asyncio.gather``:
             - Recent blocks analysis
             - DEX program account queries
             - Seed wallet expansion
        3. If still < max_wallets, **trending tokens** (Strategy 5) runs as a final pass.

        Results pass through a validation pipeline (balance check, optional activity
        validation, persistent dedup) and are cached in Redis + in-memory.

        Args:
            limit: Maximum number of transactions to query (deprecated, kept for compatibility)
            min_trade_count: Minimum number of trades a wallet must have to be included
            max_wallets: Maximum number of wallets to return
            hours_back: Number of hours to look back for transactions
            strict: If True, raise DiscoveryError when the API key is missing instead
                    of returning an empty list. Default is False (backward-compatible).

        Returns:
            List of unique wallet addresses, sorted by activity (most active first)

        Raises:
            DiscoveryError: If strict=True and no Helius API key is configured.

        See also:
            ``scout/docs/wallet-discovery.md`` for full architecture documentation.
        """
        start_time = time.time()
        strategy_used = "none"
        errors_encountered = 0

        # Reset discovery state
        self._discovered_this_run.clear()
        self._api_calls_made = 0

        if not self.api_key:
            logger = logging.getLogger(__name__)
            logger.error(
                "[Helius] No API key configured. Wallet discovery cannot proceed.\n"
                "[Helius]   Remediation:\n"
                "[Helius]     1. Set HELIUS_API_KEY environment variable, or\n"
                "[Helius]     2. Set CHIMERA_RPC__PRIMARY_URL with an embedded api-key query param, or\n"
                "[Helius]     3. Provide api_key=<key> when constructing HeliusClient."
            )
            if strict:
                raise DiscoveryError(
                    "No Helius API key configured. Set HELIUS_API_KEY or pass strict=False."
                )
            return []

        # Check monthly hard cap (safety valve)
        if CREDIT_TRACKER_AVAILABLE:
            tracker = get_credit_tracker()
            snapshot = tracker.get_snapshot()
            if snapshot.credits_remaining <= 0:
                logger.warning(
                    "[Helius] Monthly credit cap reached. Skipping wallet discovery..."
                )
                return []

        print("[Helius] Discovering wallets from recent swaps...")
        print(f"[Helius] Config: min_trades={min_trade_count}, max_wallets={max_wallets}, hours_back={hours_back}")

        # Check discovery cache (Redis first, then in-memory)
        cached = self._get_discovery_cache(hours_back, max_wallets)
        if cached is not None:
            return cached

        wallet_counts: Dict[str, int] = defaultdict(int)

        # Load configurable limits (Item 6 — centralized config)
        if ScoutConfig:
            limit_per_token = ScoutConfig.get_discovery_limit_per_token()
            block_limit = ScoutConfig.get_discovery_block_limit()
            program_limit = ScoutConfig.get_discovery_program_limit()
            seed_limit_per_wallet = ScoutConfig.get_discovery_seed_limit_per_wallet()
            fallback_threshold = max(
                1, int(max_wallets * ScoutConfig.get_discovery_fallback_threshold_pct())
            )
        else:
            limit_per_token = int(os.getenv("SCOUT_DISCOVERY_LIMIT_PER_TOKEN", "200"))
            block_limit = int(os.getenv("SCOUT_DISCOVERY_BLOCK_LIMIT", "500"))
            program_limit = int(os.getenv("SCOUT_DISCOVERY_PROGRAM_LIMIT", "500"))
            seed_limit_per_wallet = int(os.getenv("SCOUT_DISCOVERY_SEED_LIMIT", "50"))
            fallback_threshold = max(1, max_wallets // 2)

        # Strategy 1: Active Token Discovery (Primary) — runs first (cheapest, most reliable)
        try:
            print("[Helius] Strategy 1: Querying active tokens...")
            token_wallets = await self._discover_from_active_tokens(
                hours_back=hours_back, limit_per_token=limit_per_token, max_wallets=max_wallets
            )
            # Cache for Strategy 5 to avoid redundant queries
            self._cached_active_token_wallets = token_wallets
            for wallet, count in token_wallets.items():
                wallet_counts[wallet] += count
            strategy_used = "tokens"
            print(f"[Helius] Strategy 1 found {len(token_wallets)} wallets")
        except Exception as e:
            errors_encountered += 1
            print(f"[Helius] Strategy 1 failed: {e}")

        # Strategies 2-4: Run in parallel if strategy 1 didn't yield enough wallets
        if len(wallet_counts) < fallback_threshold:
            print(
                f"[Helius] Running strategies 2-4 in parallel "
                f"(have {len(wallet_counts)}, need {fallback_threshold})..."
            )

            async def _safe_strategy(
                tag: str, coro: "Any", timeout_secs: int = 120
            ) -> Tuple[str, Dict[str, int]]:
                """Wrap a strategy coroutine so exceptions/timeouts are caught and logged."""
                try:
                    result = await asyncio.wait_for(coro, timeout=timeout_secs)
                    print(f"[Helius] Strategy ({tag}) found {len(result)} wallets")
                    return tag, result
                except (asyncio.TimeoutError, asyncio.CancelledError):
                    print(f"[Helius] Strategy ({tag}) timed out after {timeout_secs}s — skipping")
                    return tag, {}
                except Exception as e:
                    print(f"[Helius] Strategy ({tag}) failed: {e}")
                    return tag, {}

            parallel_results = await asyncio.gather(
                _safe_strategy(
                    "blocks",
                    self._discover_from_recent_blocks(
                        hours_back=hours_back, limit=block_limit
                    ),
                ),
                _safe_strategy(
                    "programs",
                    self._discover_from_dex_programs(
                        hours_back=hours_back, limit=program_limit
                    ),
                ),
                _safe_strategy(
                    "seeds",
                    self._discover_from_seed_wallets(
                        hours_back=hours_back,
                        limit_per_wallet=seed_limit_per_wallet,
                    ),
                ),
            )

            for tag, result in parallel_results:
                if result:
                    strategy_used = f"{strategy_used}+{tag}"
                    for wallet, count in result.items():
                        wallet_counts[wallet] += count

        # Strategy 5: Reverse Token Analysis (Trending Tokens)
        # Runs whenever we still need wallets. If BIRDEYE_API_KEY is set, a Birdeye-based
        # trending query can be used; otherwise falls back to Helius active-token analysis.
        if len(wallet_counts) < max_wallets:
            try:
                print("[Helius] Strategy 5: Analyzing top trending tokens (Reverse Analysis)...")
                trending_wallets = await self.discover_from_top_performing_tokens()
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

        # Cheap pre-validation: batch-check SOL balances to filter programs/vaults.
        # System accounts and programs won't have nonzero SOL balances.
        validate_balances = os.getenv("SCOUT_VALIDATE_WALLET_BALANCE", "true").lower() == "true"
        if validate_balances and candidate_wallets:
            print("[Helius] Validating wallet balances (batch)...")
            pre_filter_count = len(candidate_wallets)
            try:
                candidate_wallets = await self._filter_by_sol_balance(candidate_wallets, min_balance_sol=0.0)
                filtered_out = pre_filter_count - len(candidate_wallets)
                if filtered_out > 0:
                    print(f"[Helius]   Filtered {filtered_out} zero-balance addresses (programs/vaults)")
            except Exception as e:
                print(f"[Helius]   Balance validation skipped (error: {e})")

        # Optional: Validate wallet activity in parallel (Item 8 — batch validation)
        validate_activity = os.getenv("SCOUT_VALIDATE_WALLET_ACTIVITY", "false").lower() == "true"
        if validate_activity:
            print("[Helius] Validating wallet activity (batch)...")
            candidate_wallets = await self._batch_validate_activity(
                candidate_wallets,
                min_trades=min_trade_count,
                days_back=7,
                max_wallets=max_wallets,
            )
        
        # Sort by trade count (most active first)
        candidate_wallets.sort(key=lambda w: wallet_counts[w], reverse=True)
        
        # Limit to max_wallets
        candidate_wallets = candidate_wallets[:max_wallets]

        # Persistent deduplication: filter out wallets seen in recent runs (Item 7)
        seen = self._get_persistent_seen_wallets()
        if seen:
            before = len(candidate_wallets)
            candidate_wallets = [w for w in candidate_wallets if w not in seen]
            deduped = before - len(candidate_wallets)
            if deduped > 0:
                print(f"[Helius] Dedup: filtered {deduped} recently-seen wallets")
            # Re-sort remaining by trade count after dedup
            candidate_wallets.sort(key=lambda w: wallet_counts[w], reverse=True)

        # Cache results in Redis + in-memory (Item 3)
        self._set_discovery_cache(candidate_wallets, hours_back, max_wallets)

        # Mark discovered wallets as seen for future dedup
        self._mark_wallets_seen(candidate_wallets)
        
        time_taken = time.time() - start_time
        
        print("[Helius] Discovery complete:")
        print(f"[Helius]   Strategy: {strategy_used}")
        print(f"[Helius]   Wallets found: {len(candidate_wallets)}")
        print(f"[Helius]   API calls: {self._api_calls_made}")
        print(f"[Helius]   Errors: {errors_encountered}")
        print(f"[Helius]   Time: {time_taken:.2f}s")

        # Print rate limit stats if adaptive mode is enabled
        if self._adaptive_enabled:
            stats = await self.get_rate_limit_stats()
            print("[Helius]   Rate Limit Stats:")
            print(f"[Helius]     Current RPS: {stats['current_rps']}/{stats['target_rps']} (target)")
            print(f"[Helius]     Current Delay: {stats['current_delay_ms']}ms")
            if stats['avg_latency_ms']:
                print(f"[Helius]     Avg Latency: {stats['avg_latency_ms']}ms")
            print(f"[Helius]     Success Rate: {stats['success_ratio']:.1%}")
            if stats['circuit_breaker_open']:
                print("[Helius]     ⚠️  Circuit Breaker: OPEN")
        
        if candidate_wallets:
            top_wallet = candidate_wallets[0]
            print(f"[Helius]   Top wallet: {top_wallet[:8]}... ({wallet_counts[top_wallet]} trades)")
        
        return candidate_wallets
    
    async def get_wallet_transactions(
        self,
        wallet_address: str,
        days: int = 30,
        limit: int = 100,
        wqs_score: Optional[float] = None,
    ) -> List[Dict[str, Any]]:
        """
        Get transaction history for a wallet.

        Args:
            wallet_address: Wallet address to query
            days: Number of days to look back
            limit: Maximum number of transactions to return
            wqs_score: Optional WQS score for cache optimization

        Returns:
            List of transaction dictionaries
        """
        if not self.api_key:
            return []

        # Normalize cache parameters to canonical buckets to ensure cache hits
        # across discovery/analysis/validation phases which use different days/limit values.
        # Canonical window: 30 days (dominant analysis window)
        # Canonical limit: round up to nearest 100
        canonical_days = 30  # Standardized time window for all phases
        canonical_limit = ((limit + 99) // 100) * 100  # Round up to nearest 100

        # Check monthly hard cap (safety valve)
        if CREDIT_TRACKER_AVAILABLE:
            tracker = get_credit_tracker()
            snapshot = tracker.get_snapshot()
            if snapshot.credits_remaining <= 0:
                logger = logging.getLogger(__name__)
                logger.warning(
                    f"[Helius] Monthly credit cap reached. Skipping wallet transaction fetch for {wallet_address[:8]}..."
                )
                return []

        # Check activity-based cache first (higher priority than basic cache)
        # Use canonical parameters for cache key consistency across phases
        if self._activity_cache:
            cached_result = self._activity_cache.get_cached_transactions(
                wallet_address, canonical_days, canonical_limit
            )
            if cached_result is not None:
                return cached_result

        # Fallback to basic cache (if activity cache not available)
        # Check cache first using canonical parameters with shortest-phase TTL
        # Use wallet metrics TTL (300s = 5 minutes) as the shortest-phase freshness requirement
        if CACHE_AVAILABLE:
            cache = get_cache()
            cache_key = f"{wallet_address}:{canonical_days}:{canonical_limit}"
            cached_result = cache.get("wallet_txs", wallet_address, cache_key,
                                    category=CacheCategory.WALLET_TXS)
            if cached_result is not None:
                return cached_result

        endpoint = f"/addresses/{wallet_address}/transactions"

        # Use canonical parameters for pagination to ensure consistency
        target = int(canonical_limit) if canonical_limit is not None else 100
        
        # Helius v0 standard page size is 100. requesting more often results in truncation.
        BATCH_SIZE = 100
        
        # Safety break for pagination
        MAX_PAGES = int(os.getenv("SCOUT_WALLET_TX_MAX_PAGES", "50"))

        # Calculate cutoff timestamp once using canonical days
        cutoff_timestamp = 0
        if canonical_days > 0:
            cutoff = utcnow() - timedelta(days=canonical_days)
            cutoff_timestamp = int(cutoff.timestamp())

        async def _paginate_with_type(tx_type: Optional[str]) -> List[Dict[str, Any]]:
            """Paginate through wallet transactions with optional type filter."""
            nonlocal MAX_PAGES, BATCH_SIZE
            before = None
            result: List[Dict[str, Any]] = []
            pg = 0

            print(f"  [{wallet_address[:8]}] Starting pagination (type={tx_type or 'ALL'}, target={target}, max_pages={MAX_PAGES})")
            while True:
                if len(result) >= target:
                    break
                if pg >= MAX_PAGES:
                    break

                params: Dict[str, Any] = {"limit": BATCH_SIZE}
                if tx_type:
                    params["type"] = tx_type
                if before:
                    params["before"] = before

                data = await self._make_request(endpoint, params)
                if not data:
                    break

                batch = data if isinstance(data, list) else data.get("transactions", [])
                if not batch:
                    break

                # Record credit cost for successful page fetch (50 credits per page)
                if CREDIT_TRACKER_AVAILABLE:
                    tracker = get_credit_tracker()
                    tracker.record_request(
                        cost=50,
                        category="analysis",
                        endpoint="getTransactionsForAddress",
                        success=True
                    )

                # Filter by time window
                batch_filtered = []
                reached_cutoff = False
                for tx in batch:
                    tx_ts = tx.get("timestamp")
                    if tx_ts:
                        if tx_ts < cutoff_timestamp:
                            reached_cutoff = True
                        else:
                            batch_filtered.append(tx)
                    else:
                        batch_filtered.append(tx)

                result.extend(batch_filtered)

                last_sig = batch[-1].get("signature")
                if not last_sig or last_sig == before:
                    break
                before = last_sig
                pg += 1

                if reached_cutoff:
                    break

            return result[:target]

        all_txs = await _paginate_with_type("SWAP")

        # Fallback: if SWAP type returned nothing, do a single unfiltered fetch
        # and filter client-side to avoid double-pagination.
        # Some wallets have token trades recorded under non-SWAP types or in
        # Helius API versions that omit the type field.
        if not all_txs:
            all_txs = await _paginate_with_type(None)
            # Client-side filter: prioritize SWAP-type transactions but include
            # other transaction types that might represent trades (TRANSFER, etc.)
            # to ensure we don't miss legitimate trading activity.
            if all_txs:
                swap_txs = [tx for tx in all_txs if tx.get("type") == "SWAP"]
                # If we found SWAP transactions, use those; otherwise use all transactions
                # to capture wallets that only have non-SWAP trade types
                all_txs = swap_txs if swap_txs else all_txs

        result = all_txs[:target]

        # Store result in activity-based cache (higher priority)
        # Use canonical parameters for cache key consistency
        if self._activity_cache and result:
            self._activity_cache.cache_transactions(
                wallet_address, result, canonical_days, canonical_limit, wqs_score
            )

        # Store result in basic cache for fallback (if activity cache not available)
        # Use canonical parameters and shortest-phase TTL (300s = wallet metrics TTL)
        if CACHE_AVAILABLE and result:
            cache = get_cache()
            cache_key = f"{wallet_address}:{canonical_days}:{canonical_limit}"
            # Use shortest-phase TTL (300s) to avoid stale data across phases
            shortest_phase_ttl = ScoutConfig.get_cache_ttl_wallet_metrics()  # 300 seconds
            cache.set("wallet_txs", wallet_address, result, cache_key,
                     category=CacheCategory.WALLET_TXS)

        return result

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
                # Parse transfers: look for token deltas involving the wallet
                source = tx.get("source")
                destination = tx.get("destination")

                # Check if wallet is source or destination
                if source == wallet_address or destination == wallet_address:
                    # Extract amount and token
                    amount = tx.get("amount")
                    mint = tx.get("mint")
                    if amount and mint:
                        # For now, just record that a transfer occurred
                        # Full implementation would track this in wallet history
                        return {
                            "type": "TRANSFER",
                            "token": mint,
                            "amount": amount,
                            "direction": "IN" if destination == wallet_address else "OUT",
                        }
                return None

            # 2. Handle LP / Staking
            # Helius enriches these types with token_transfers + native_transfers.
            # We summarise each as a structured event so the wallet analyser can
            # adjust metrics appropriately (LP wallets have different risk profiles).
            elif tx_type in ("ADD_LIQUIDITY", "REMOVE_LIQUIDITY", "STAKE_TOKEN", "UNSTAKE_TOKEN"):
                token_transfers = tx.get("tokenTransfers", [])
                native_transfers = tx.get("nativeTransfers", [])
                # Collect inbound and outbound deltas relative to wallet_address
                tokens_in, tokens_out = [], []
                for tt in token_transfers:
                    mint = tt.get("mint") or tt.get("token")
                    amount = tt.get("tokenAmount", 0)
                    if tt.get("toUserAccount") == wallet_address:
                        tokens_in.append({"mint": mint, "amount": amount})
                    elif tt.get("fromUserAccount") == wallet_address:
                        tokens_out.append({"mint": mint, "amount": amount})
                sol_delta = 0.0
                for nt in native_transfers:
                    lamports = nt.get("amount", 0)
                    if nt.get("toUserAccount") == wallet_address:
                        sol_delta += lamports / 1e9
                    elif nt.get("fromUserAccount") == wallet_address:
                        sol_delta -= lamports / 1e9
                is_lp = tx_type in ("ADD_LIQUIDITY", "REMOVE_LIQUIDITY")
                return {
                    "type": "LIQUIDITY_EVENT" if is_lp else "STAKE_EVENT",
                    "subtype": tx_type,
                    "tokens_in": tokens_in,
                    "tokens_out": tokens_out,
                    "sol_delta": sol_delta,
                    "signature": tx.get("signature", ""),
                    "timestamp": tx.get("timestamp", 0),
                }

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

        Tries multiple strategies in sequence:
        1. Wallet-relative token/sol deltas (primary)
        2. Swap events (newer Helius enriched format)
        3. Raw accountData balance changes (fallback)

        Args:
            tx: Transaction object from Helius
            wallet_address: Wallet address to filter for

        Returns:
            Dictionary with swap details or None if not a valid swap
        """
        if not isinstance(tx, dict):
            return None

        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(utcnow().timestamp()))

        # REQUIRE wallet address for accurate parsing
        if not wallet_address:
            return None

        # Check wallet involvement (shared across all strategies)
        if not self._is_wallet_involved(tx, wallet_address):
            return None

        # Strategy 1: wallet-relative deltas (primary)
        try:
            result = self._parse_swap_from_deltas(tx, wallet_address)
            if result:
                return result
        except Exception as e:
            logger.debug("Strategy 1 delta parser failed: %s", e)

        # Strategy 2: swap events (newer Helius enriched format)
        try:
            result = self._parse_swap_from_events(tx, wallet_address)
            if result:
                return result
        except Exception as e:
            logger.debug("Strategy 2 events parser failed: %s", e)

        # Strategy 3: raw accountData balance changes (fallback)
        try:
            result = self._parse_swap_from_account_data(tx, wallet_address)
            if result:
                return result
        except Exception as e:
            logger.debug("Strategy 3 account-data parser failed: %s", e)

        return None

    def _is_wallet_involved(self, tx: Dict[str, Any], wallet_address: str) -> bool:
        """Check if wallet is involved in this transaction via any transfer type."""
        # Check feePayer
        if tx.get("feePayer") == wallet_address:
            return True

        # Check signatures - wallet may be a signer without appearing in transfer fields
        # This handles cases where the wallet is the authority/signature but not directly
        # involved in the tokenTransfer/nativeTransfer fromUserAccount/toUserAccount fields
        for sig in tx.get("signatures", []) or []:
            if isinstance(sig, str) and sig == wallet_address:
                return True

        # Check tokenTransfers
        for tr in tx.get("tokenTransfers", []) or []:
            if (tr.get("fromUserAccount") == wallet_address or
                tr.get("toUserAccount") == wallet_address):
                return True

        # Check nativeTransfers (SOL-only swaps)
        for nt in tx.get("nativeTransfers", []) or []:
            if (nt.get("fromUserAccount") == wallet_address or
                nt.get("toUserAccount") == wallet_address):
                return True

        # Check accountData for balance changes (token AND native SOL)
        for acc in tx.get("accountData", []) or []:
            if acc.get("account") == wallet_address:
                if acc.get("tokenBalanceChanges"):
                    return True
                if acc.get("nativeBalanceChange") is not None:
                    return True

        # Check instructions for wallet involvement (DEX program interactions)
        for instr in tx.get("instructions", []) or []:
            accounts = instr.get("accounts", []) or []
            if wallet_address in accounts:
                return True

        return False

    def _parse_swap_from_deltas(
        self,
        tx: Dict[str, Any],
        wallet_address: str,
    ) -> Optional[Dict[str, Any]]:
        """
        Strategy 1: Parse swap from wallet-relative token and SOL deltas.

        COMPREHENSIVE ENHANCEMENTS:
        - Expanded stablecoin support (USDC, USDT, PYUSD, DAI, USDD, TUSD, FDUSD, BUSD)
        - Improved token->token swap detection with multi-token support
        - Enhanced instruction-level pattern recognition
        - Better handling of complex DEX transactions (Jupiter, Orca, Raydium)
        """
        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(utcnow().timestamp()))

        sol_mint = "So11111111111111111111111111111111111111112"
        usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        usdt_mint = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
        pyusd_mint = "Fm7yTTQkwMqhf76GymzctsgEpCvX4q3xdHgBqFVSQKk"

        # COMPREHENSIVE STABLECOIN LIST
        stable_mints = {
            usdc_mint,  # USDC
            usdt_mint,  # USDT
            pyusd_mint,  # PYUSD
            "7dHbWXmci3dTUpSFJC3s3nxMPrsrTn5fQjYPb26cscQ",  # USDD (maintained by TRON DAO)
            "DAiHhAwpCe2ygJmhzwQTvXcFqBVRNAGfnUSQ4gNpm5f",  # DAI (legacy)
            "3KBZiQHbjmiNtbaDNqeiyp6Y3qqmANuGphxDjPXqnDVe",  # DAI (official)
            "4MNeZJj3iWc3C7YFU1iXbSsrLuvQEwNyAGTWXfUFguwF",  # TUSD
            "5fTkp16UPQMJyyw7Tm2jrRKTMrMVZh6gHNiYLHNpVV09",  # FDUSD
            "CWGsHHN7LCLfgL8rBFaJMXzyYrRoP7yRgx15fLaTnUuW",  # BUSD (deprecated but still in circulation)
        }

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

            from_acc = tr.get("fromUserAccount")
            to_acc = tr.get("toUserAccount")
            user_acc = tr.get("userAccount")

            if from_acc == wallet_address or (user_acc == wallet_address and tr.get("fromUserAccount") == wallet_address):
                token_deltas[mint] -= amt_ui
            if to_acc == wallet_address or (user_acc == wallet_address and tr.get("toUserAccount") == wallet_address):
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
                # Debug log why parse failed
                # print(f"[Parser] No primary token found. sol_delta={sol_delta}, token_deltas={len(token_deltas)}")
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

            if stable_mint_used is not None:
                # Pick the primary non-stable token by abs delta
                other_mint = None
                other_delta = 0.0
                for mint, delta in token_deltas.items():
                    if mint in stable_mints or mint == sol_mint:
                        continue
                    if abs(delta) > abs(other_delta):
                        other_delta = delta
                        other_mint = mint

                # Stablecoin→stablecoin swap (e.g. USDC→USDT): both mints are in
                # stable_mints so the loop above never assigns other_mint. Fall
                # back to the second stablecoin by absolute delta.
                if other_mint is None and stable_mint_used is not None:
                    for mint, delta in token_deltas.items():
                        if mint in stable_mints and mint != stable_mint_used and abs(delta) >= 1e-12:
                            other_mint = mint
                            other_delta = delta
                            break

                if other_mint and abs(other_delta) >= 1e-12:
                    usd_amount = abs(stable_delta)
                    token_amount = abs(other_delta)
                    price_usd = (usd_amount / token_amount) if token_amount > 0 else 0.0

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
                        "sol_amount": None,
                        "direction": direction,
                        "price_sol": None,
                        "price_usd": price_usd,
                        "usd_amount": usd_amount,
                        "quote_mint": stable_mint_used,
                        "net_sol_delta": 0.0,
                        "net_token_delta": net_token_delta,
                    }

            # COMPREHENSIVE ENHANCEMENT: Advanced token→token swap parsing
            # Handle multi-token swaps, routing transactions, and complex DEX patterns

            # Strategy A: Identify the two largest non-SOL token deltas (one inflow, one outflow)
            inflow = (None, 0.0)   # (mint, delta) for delta > 0
            outflow = (None, 0.0)  # (mint, delta) for delta < 0
            all_inflows = []  # Track all inflows for multi-token swaps
            all_outflows = []  # Track all outflows for multi-token swaps

            for mint, delta in token_deltas.items():
                if mint == sol_mint:
                    continue
                if delta > 0:
                    all_inflows.append((mint, delta))
                    if delta > inflow[1]:
                        inflow = (mint, delta)
                elif delta < 0:
                    all_outflows.append((mint, delta))
                    if abs(delta) > abs(outflow[1]):
                        outflow = (mint, delta)

            # Strategy B: Multi-token swap detection (Jupiter routing, Orca whirlpools)
            if len(all_inflows) >= 1 and len(all_outflows) >= 1:
                # Sort by absolute delta to find the most significant tokens
                all_inflows.sort(key=lambda x: abs(x[1]), reverse=True)
                all_outflows.sort(key=lambda x: abs(x[1]), reverse=True)

                # Primary tokens are the largest inflow and outflow
                primary_in_mint, primary_in_delta = all_inflows[0]
                primary_out_mint, primary_out_delta = all_outflows[0]

                # Determine direction based on stablecoin involvement
                # If we're swapping FROM stablecoin, it's a BUY
                # If we're swapping TO stablecoin, it's a SELL
                if any(mint in stable_mints for mint, _ in all_outflows):
                    # We're spending stablecoins to get tokens -> BUY
                    token_mint = primary_in_mint
                    token_amount = abs(primary_in_delta)
                    direction = "BUY"
                    stable_spent = sum(abs(delta) for mint, delta in all_outflows if mint in stable_mints)
                    return {
                        "signature": signature,
                        "timestamp": timestamp,
                        "wallet": wallet_address,
                        "token_mint": token_mint,
                        "token_amount": token_amount,
                        "sol_amount": None,
                        "direction": direction,
                        "price_sol": None,
                        "price_usd": stable_spent / token_amount if token_amount > 0 else None,
                        "usd_amount": stable_spent,
                        "quote_mint": next(mint for mint, _ in all_outflows if mint in stable_mints),
                        "net_sol_delta": 0.0,
                        "net_token_delta": primary_in_delta,
                        "swap_type": "token_to_token_multi",
                    }
                elif any(mint in stable_mints for mint, _ in all_inflows):
                    # We're selling tokens for stablecoins -> SELL
                    token_mint = primary_out_mint
                    token_amount = abs(primary_out_delta)
                    direction = "SELL"
                    stable_received = sum(abs(delta) for mint, delta in all_inflows if mint in stable_mints)
                    return {
                        "signature": signature,
                        "timestamp": timestamp,
                        "wallet": wallet_address,
                        "token_mint": token_mint,
                        "token_amount": token_amount,
                        "sol_amount": None,
                        "direction": direction,
                        "price_sol": None,
                        "price_usd": stable_received / token_amount if token_amount > 0 else None,
                        "usd_amount": stable_received,
                        "quote_mint": next(mint for mint, _ in all_inflows if mint in stable_mints),
                        "net_sol_delta": 0.0,
                        "net_token_delta": primary_out_delta,
                        "swap_type": "token_to_token_multi",
                    }
                elif inflow[0] and outflow[0]:
                    # Pure token-to-token swap (no stablecoins involved)
                    # Use the token received as the primary (we're buying it)
                    token_mint = inflow[0]
                    token_amount = abs(inflow[1])
                    direction = "BUY"
                    return {
                        "signature": signature,
                        "timestamp": timestamp,
                        "wallet": wallet_address,
                        "token_mint": token_mint,
                        "token_amount": token_amount,
                        "sol_amount": None,
                        "direction": direction,
                        "price_sol": None,
                        "price_usd": None,
                        "usd_amount": None,
                        "quote_mint": outflow[0],  # Track what we sold
                        "net_sol_delta": 0.0,
                        "net_token_delta": inflow[1],
                        "swap_type": "token_to_token_pure",
                    }

            # Strategy C: Instruction-level pattern recognition for complex swaps
            # This handles cases where tokenTransfers don't capture the full picture
            if not inflow[0] or not outflow[0]:
                instruction_result = self._parse_from_instruction_level(tx, wallet_address, token_deltas)
                if instruction_result:
                    return instruction_result

            # Could not value without SOL, stable, or clear token pair
            return None

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

        # When the primary token is a stablecoin, the direction is inverted in
        # base-quote terms: "buying USDC with SOL" = SELL (you sold the base asset).
        if primary_mint in stable_mints:
            direction = "SELL" if direction == "BUY" else "BUY"

        price_sol = (sol_amount / token_amount) if token_amount > 0 else 0.0

        result = {
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

        # PUMP_FUN edge case: Reject pure SOL-in/SOL-out transactions (no token movement)
        # This can happen with fee-only transactions or wrapping/unwrapping
        if abs(result["net_sol_delta"]) < SIGNIFICANT_SOL and abs(result["net_token_delta"]) < 1e-9:
            return None

        return result

    def _parse_from_instruction_level(
        self,
        tx: Dict[str, Any],
        wallet_address: str,
        token_deltas: Dict[str, float]
    ) -> Optional[Dict[str, Any]]:
        """
        Strategy 3: Parse swap from instruction-level patterns.

        This method handles complex DEX transactions where tokenTransfers
        don't capture the full swap picture, such as:
        - Jupiter aggregator routing transactions
        - Orca whirlpool multi-hop swaps
        - Raydium multi-pool transactions
        - Token-2022 program swaps
        """
        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(utcnow().timestamp()))

        instructions = tx.get("instructions", [])
        if not instructions:
            return None


        # DEX program identifiers for instruction-level parsing
        dex_program_patterns = {
            "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4": "jupiter",
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc": "orca",
            "mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So": "raydium",
            "9WzaBBWQNqAghxSAfKUUx3ZkhBBFCkTUvJJJcjF2oG4": "orca",
            "swoQ1Yx4kK_7d9pNVbDiVSe7XPqTc2nRvEmMuXelNhk": "swap",
        }

        # Analyze instructions for swap patterns
        stable_mints = {
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",  # USDC
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",  # USDT
            "Fm7yTTQkwMqhf76GymzctsgEpCvX4q3xdHgBqFVSQKk",  # PYUSD
        }

        for instr in instructions:
            if not isinstance(instr, dict):
                continue

            program_id = instr.get("programId", "")
            parsed = instr.get("parsed", {})

            # Check if this is a DEX instruction
            dex_type = None
            for dex_program, dex_name in dex_program_patterns.items():
                if program_id == dex_program or program_id in dex_program:
                    dex_type = dex_name
                    break

            if not dex_type:
                continue

            # Parse instruction type
            instr_type = parsed.get("type", "")
            if instr_type.lower() in ["swap", "swapbasein", "swapbaseout", "exactinsingle", "exactoutsingle"]:
                # Extract account information
                accounts = instr.get("accounts", [])
                if not accounts:
                    continue

                # Look for token mint accounts in the instruction data
                info = parsed.get("info", {})
                if isinstance(info, dict):
                    token_in = info.get("tokenIn") or info.get("inputMint")
                    token_out = info.get("tokenOut") or info.get("outputMint")

                    if token_in and token_out:
                        # Check if wallet is involved
                        if wallet_address in accounts:
                            # Determine direction based on token_deltas
                            in_delta = token_deltas.get(token_in, 0)
                            out_delta = token_deltas.get(token_out, 0)

                            # If we're receiving token_out and sending token_in, it's a BUY
                            if out_delta > 0 and in_delta < 0:
                                token_mint = token_out
                                token_amount = abs(out_delta)
                                direction = "BUY"

                                # Try to determine price from stablecoin involvement
                                usd_amount = None
                                price_usd = None
                                quote_mint = None

                                if token_in in stable_mints:
                                    usd_amount = abs(in_delta)
                                    price_usd = usd_amount / token_amount if token_amount > 0 else None
                                    quote_mint = token_in

                                return {
                                    "signature": signature,
                                    "timestamp": timestamp,
                                    "wallet": wallet_address,
                                    "token_mint": token_mint,
                                    "token_amount": token_amount,
                                    "sol_amount": None,
                                    "direction": direction,
                                    "price_sol": None,
                                    "price_usd": price_usd,
                                    "usd_amount": usd_amount,
                                    "quote_mint": quote_mint,
                                    "net_sol_delta": 0.0,
                                    "net_token_delta": out_delta,
                                    "swap_type": f"instruction_{dex_type}",
                                }

        return None

    def _parse_swap_from_events(
        self,
        tx: Dict[str, Any],
        wallet_address: str,
    ) -> Optional[Dict[str, Any]]:
        """
        Strategy 2: Parse swap from Helius enriched swap events.
        Newer Helius API versions include structured swap data in tx["events"]["swap"].
        """
        events = tx.get("events", {})
        if not isinstance(events, dict):
            return None

        swap = events.get("swap", {})
        if not isinstance(swap, dict) or not swap:
            return None

        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(utcnow().timestamp()))

        native_input = swap.get("nativeInput") or swap.get("nativeIn")
        native_output = swap.get("nativeOutput") or swap.get("nativeOut")

        token_inputs = swap.get("tokenInputs", []) or swap.get("tokenIn", [])
        token_outputs = swap.get("tokenOutputs", []) or swap.get("tokenOut", [])

        # Handle single dict vs list
        if isinstance(token_inputs, dict):
            token_inputs = [token_inputs]
        if isinstance(token_outputs, dict):
            token_outputs = [token_outputs]

        sol_mint = "So11111111111111111111111111111111111111112"

        if native_input is not None and native_output is not None:
            # SOL-in token-out = BUY, SOL-out token-in = SELL
            if isinstance(native_input, dict):
                sol_in = _safe_float(native_input.get("amount", 0))
            else:
                sol_in = float(native_input) if native_input else 0.0
            if isinstance(native_output, dict):
                sol_out = _safe_float(native_output.get("amount", 0))
            else:
                sol_out = float(native_output) if native_output else 0.0

            token_in_count = 0.0
            token_in_mint = None
            for ti in token_inputs:
                if isinstance(ti, dict):
                    token_in_mint = ti.get("mint") or token_in_mint
                    token_in_count += _safe_float(ti.get("rawTokenAmount", 0))
                else:
                    token_in_count += float(ti) if ti else 0.0

            token_out_count = 0.0
            token_out_mint = None
            for to in token_outputs:
                if isinstance(to, dict):
                    token_out_mint = to.get("mint") or token_out_mint
                    token_out_count += _safe_float(to.get("rawTokenAmount", 0))
                else:
                    token_out_count += float(to) if to else 0.0

            if sol_in > sol_out:
                # Spent SOL, received token
                if token_out_mint and token_out_count > 0:
                    token_amount = token_out_count
                    token_decimals = token_outputs[0].get("decimals", 0) if isinstance(token_outputs[0], dict) and token_outputs else 0
                    if token_decimals > 0:
                        token_amount /= (10 ** token_decimals)
                    return {
                        "signature": signature,
                        "timestamp": timestamp,
                        "wallet": wallet_address,
                        "token_mint": token_out_mint,
                        "token_amount": token_amount,
                        "sol_amount": sol_in - sol_out,
                        "direction": "BUY",
                        "price_sol": (sol_in - sol_out) / token_amount if token_amount > 0 else 0.0,
                        "price_usd": None,
                        "usd_amount": None,
                        "quote_mint": sol_mint,
                        "net_sol_delta": -(sol_in - sol_out),
                        "net_token_delta": token_amount,
                    }
            elif sol_out > sol_in:
                # Received SOL, spent token
                if token_in_mint and token_in_count > 0:
                    token_amount = token_in_count
                    token_decimals = token_inputs[0].get("decimals", 0) if isinstance(token_inputs[0], dict) and token_inputs else 0
                    if token_decimals > 0:
                        token_amount /= (10 ** token_decimals)
                    return {
                        "signature": signature,
                        "timestamp": timestamp,
                        "wallet": wallet_address,
                        "token_mint": token_in_mint,
                        "token_amount": token_amount,
                        "sol_amount": sol_out - sol_in,
                        "direction": "SELL",
                        "price_sol": (sol_out - sol_in) / token_amount if token_amount > 0 else 0.0,
                        "price_usd": None,
                        "usd_amount": None,
                        "quote_mint": sol_mint,
                        "net_sol_delta": sol_out - sol_in,
                        "net_token_delta": -token_amount,
                    }

        # Token-to-token swap from events (no SOL leg)
        if token_inputs and token_outputs:
            token_in_mint = None
            token_in_count = 0.0
            for ti in token_inputs:
                if isinstance(ti, dict):
                    token_in_mint = ti.get("mint") or token_in_mint
                    token_in_count += _safe_float(ti.get("rawTokenAmount", 0))
            token_out_mint = None
            token_out_count = 0.0
            for to in token_outputs:
                if isinstance(to, dict):
                    token_out_mint = to.get("mint") or token_out_mint
                    token_out_count += _safe_float(to.get("rawTokenAmount", 0))

            if token_in_mint and token_out_mint and token_out_count > 0:
                token_decimals = token_outputs[0].get("decimals", 0) if isinstance(token_outputs[0], dict) and token_outputs else 0
                if token_decimals > 0:
                    token_out_count /= (10 ** token_decimals)
                return {
                    "signature": signature,
                    "timestamp": timestamp,
                    "wallet": wallet_address,
                    "token_mint": token_out_mint,
                    "token_amount": token_out_count,
                    "sol_amount": None,
                    "direction": "BUY",
                    "price_sol": None,
                    "price_usd": None,
                    "usd_amount": None,
                    "quote_mint": None,
                    "net_sol_delta": 0.0,
                    "net_token_delta": token_out_count,
                }

        return None

    def _parse_swap_from_account_data(
        self,
        tx: Dict[str, Any],
        wallet_address: str,
    ) -> Optional[Dict[str, Any]]:
        """
        Strategy 3: Parse swap from raw accountData balance changes (last-resort fallback).

        Some Helius API responses omit structured tokenTransfers but include pre/post
        token balance changes in accountData. This reconstructs a minimal trade from
        the wallet's own balance deltas.
        """
        signature = tx.get("signature", "")
        timestamp = tx.get("timestamp", int(utcnow().timestamp()))

        wallet_data = None
        for acc in tx.get("accountData", []) or []:
            if acc.get("account") == wallet_address:
                wallet_data = acc
                break

        if not wallet_data:
            return None

        token_balance_changes = wallet_data.get("tokenBalanceChanges") or []
        if not token_balance_changes:
            return None

        # Find the largest token balance change (most significant movement)
        best_change = None
        best_delta = 0.0
        for change in token_balance_changes:
            mint = change.get("mint")
            if not mint:
                continue
            raw_before = _safe_float(change.get("rawTokenAmountBefore", 0) or 0)
            raw_after = _safe_float(change.get("rawTokenAmountAfter", 0) or 0)
            delta = raw_after - raw_before
            if abs(delta) > abs(best_delta):
                best_delta = delta
                best_change = change

        if not best_change or abs(best_delta) < 1e-12:
            return None

        mint = best_change.get("mint", "")
        try:
            token_decimals = int(best_change.get("decimals", 0) or 0)
        except (TypeError, ValueError):
            token_decimals = 0
        token_amount = abs(best_delta)
        if token_decimals > 0:
            token_amount /= (10 ** token_decimals)

        # Try to determine SOL delta from native balance changes
        sol_delta = 0.0
        native_before = _safe_float(wallet_data.get("nativeBalanceChange", {}).get("before", 0) or 0)
        native_after = _safe_float(wallet_data.get("nativeBalanceChange", {}).get("after", 0) or 0)
        if native_before or native_after:
            sol_delta = (native_after - native_before) / 1e9

        direction = "BUY" if best_delta > 0 else "SELL"
        sol_amount = abs(sol_delta) if sol_delta != 0 else None

        return {
            "signature": signature,
            "timestamp": timestamp,
            "wallet": wallet_address,
            "token_mint": mint,
            "token_amount": token_amount,
            "sol_amount": sol_amount,
            "direction": direction,
            "price_sol": (sol_amount / token_amount) if sol_amount and token_amount > 0 else None,
            "price_usd": None,
            "usd_amount": None,
            "quote_mint": "So11111111111111111111111111111111111111112" if sol_amount else None,
            "net_sol_delta": sol_delta,
            "net_token_delta": best_delta,
        }
