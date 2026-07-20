"""
Wallet Analyzer - On-chain data fetching and analysis

This module fetches wallet transaction data from Solana RPC/APIs
and computes performance metrics for WQS calculation.

In production, this connects to:
- Helius API for transaction history and wallet discovery
- Jupiter API for price data
- On-chain token data for position tracking
"""

from collections import OrderedDict

import asyncio
import os
import time
import logging
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import List, Optional, Dict, Any, Tuple

from .utils import utcnow

from .wqs import WalletMetrics
from .models import HistoricalTrade, TradeAction, LiquidityData, TraderArchetype
from .helius_client import HeliusClient
from .liquidity import LiquidityProvider
from .decimal_utils import float_to_decimal, decimal_to_float, safe_decimal_divide
from .denylist import is_known_scam_address, check_wallet_correlation

# Import state persistence for multi-timeframe discovery tracking
try:
    from .state_persistence import StatePersistence
    STATE_PERSISTENCE_AVAILABLE = True
except ImportError:
    STATE_PERSISTENCE_AVAILABLE = False
    StatePersistence = None

# Import config and security client
try:
    from config import ScoutConfig
    from .security_client import RugCheckClient
    SECURITY_AVAILABLE = True
    CONFIG_AVAILABLE = True
except ImportError:
    SECURITY_AVAILABLE = False
    CONFIG_AVAILABLE = False
    ScoutConfig = None
    RugCheckClient = None

logger = logging.getLogger(__name__)


class PortfolioTracker:
    """
    Reconstructs a wallet's current holdings to detect hidden losses (bag holders).
    
    This class replays trade history to determine current token positions and
    calculates unrealized PnL by comparing current prices to cost basis.
    """
    
    @staticmethod
    def calculate_unrealized_pnl(
        trades: List[HistoricalTrade], 
        current_prices: Dict[str, float],
        sol_price_usd: Optional[float] = None
    ) -> float:
        """
        Replays trades to find current holdings and calculates paper loss.
        
        Uses Decimal internally for all financial calculations to avoid floating-point errors.
        Converts to float at the boundary for API compatibility.
        
        Args:
            trades: List of historical trades (sorted by timestamp)
            current_prices: Dict mapping token_address -> current_price_usd
            sol_price_usd: Current SOL price in USD (for converting SOL cost basis to USD)
            
        Returns:
            Total unrealized loss in SOL (positive value = loss)
        """
        holdings = {}  # token_addr -> amount (token units) as Decimal
        cost_basis = {}  # token_addr -> total_sol_spent as Decimal
        
        # Convert SOL price to Decimal
        sol_price_decimal = float_to_decimal(sol_price_usd) if sol_price_usd is not None else Decimal('1.0')
        
        # 1. Replay history using FIFO logic
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                token_addr = t.token_address
                # Calculate token amount from trade
                token_amount = t.token_amount
                if token_amount is None or token_amount == Decimal('0'):
                    # Fallback: calculate from SOL amount and price
                    if t.price_sol and t.price_sol > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_sol)
                    elif t.price_at_trade and t.price_at_trade > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        continue  # Skip if we can't determine amount
                
                holdings[token_addr] = holdings.get(token_addr, Decimal('0')) + token_amount
                cost_basis[token_addr] = cost_basis.get(token_addr, Decimal('0')) + t.amount_sol
                
            elif t.action == TradeAction.SELL:
                token_addr = t.token_address
                current_qty = holdings.get(token_addr, Decimal('0'))
                if current_qty <= Decimal('0'):
                    continue
                
                # Calculate token amount sold
                token_amount = t.token_amount
                if token_amount is None or token_amount == Decimal('0'):
                    if t.price_sol and t.price_sol > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_sol)
                    elif t.price_at_trade and t.price_at_trade > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        continue
                
                # FIFO: Reduce holdings and cost basis proportionally
                ratio = min(Decimal('1.0'), safe_decimal_divide(token_amount, current_qty)) if current_qty > Decimal('0') else Decimal('0')
                holdings[token_addr] = max(Decimal('0'), current_qty - token_amount)
                cost_basis[token_addr] = cost_basis.get(token_addr, Decimal('0')) * (Decimal('1.0') - ratio)
        
        # 2. Calculate Value vs Cost for remaining holdings
        total_unrealized_loss_sol = Decimal('0')
        
        for token, qty in holdings.items():
            if qty <= Decimal('0'):
                continue
            
            remaining_cost_sol = cost_basis.get(token, Decimal('0'))
            
            # Ignore dust entries (< 0.5 SOL cost basis)
            if remaining_cost_sol < Decimal('0.5'):
                continue
            
            # Get current price (in USD) and convert to Decimal
            current_price_usd_float = current_prices.get(token, 0.0)
            current_price_usd = float_to_decimal(current_price_usd_float)
            
            if current_price_usd <= Decimal('0'):
                # If price unavailable, assume it's worthless (100% loss)
                total_unrealized_loss_sol += remaining_cost_sol
                continue
            
            # Convert token quantity to USD value
            current_val_usd = qty * current_price_usd
            remaining_cost_usd = remaining_cost_sol * sol_price_decimal
            
            # If value is < 20% of cost, it's a heavy bag
            if remaining_cost_usd > Decimal('0'):
                threshold = remaining_cost_usd * Decimal('0.20')
                if current_val_usd < threshold:
                    # Calculate loss in SOL terms
                    loss_usd = remaining_cost_usd - current_val_usd
                    loss_sol = safe_decimal_divide(loss_usd, sol_price_decimal) if sol_price_decimal > Decimal('0') else loss_usd
                    total_unrealized_loss_sol += loss_sol
        
        # Convert to float at boundary for API compatibility
        return decimal_to_float(total_unrealized_loss_sol)
    
    @staticmethod
    def calculate_paper_gains(
        trades: List[HistoricalTrade],
        current_prices: Dict[str, float],
        sol_price_usd: Optional[float] = None
    ) -> float:
        """
        Calculate unrealized gains from positions currently in profit.
        
        Returns the total unrealized gain in SOL (positive value = gain).
        Only counts positions where current value > cost basis by >20%.
        """
        holdings: Dict[str, Decimal] = {}
        cost_basis: Dict[str, Decimal] = {}
        sol_price_decimal = float_to_decimal(sol_price_usd) if sol_price_usd is not None else Decimal('1.0')
        
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                token_addr = t.token_address
                token_amount = t.token_amount
                if token_amount is None or token_amount == Decimal('0'):
                    if t.price_sol and t.price_sol > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_sol)
                    elif t.price_at_trade and t.price_at_trade > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        continue
                holdings[token_addr] = holdings.get(token_addr, Decimal('0')) + token_amount
                cost_basis[token_addr] = cost_basis.get(token_addr, Decimal('0')) + t.amount_sol
            elif t.action == TradeAction.SELL:
                token_addr = t.token_address
                current_qty = holdings.get(token_addr, Decimal('0'))
                if current_qty <= Decimal('0'):
                    continue
                token_amount = t.token_amount
                if token_amount is None or token_amount == Decimal('0'):
                    if t.price_sol and t.price_sol > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_sol)
                    elif t.price_at_trade and t.price_at_trade > Decimal('0'):
                        token_amount = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        continue
                ratio = min(Decimal('1.0'), safe_decimal_divide(token_amount, current_qty)) if current_qty > Decimal('0') else Decimal('0')
                holdings[token_addr] = max(Decimal('0'), current_qty - token_amount)
                cost_basis[token_addr] = cost_basis.get(token_addr, Decimal('0')) * (Decimal('1.0') - ratio)
        
        total_unrealized_gain_sol = Decimal('0')
        for token, qty in holdings.items():
            if qty <= Decimal('0'):
                continue
            remaining_cost_sol = cost_basis.get(token, Decimal('0'))
            if remaining_cost_sol < Decimal('0.5'):
                continue
            current_price_usd_float = current_prices.get(token, 0.0)
            current_price_usd = float_to_decimal(current_price_usd_float)
            if current_price_usd <= Decimal('0'):
                continue
            current_val_usd = qty * current_price_usd
            remaining_cost_usd = remaining_cost_sol * sol_price_decimal
            if remaining_cost_usd > Decimal('0'):
                profit_ratio = current_val_usd / remaining_cost_usd
                if profit_ratio > Decimal('1.20'):
                    gain_usd = current_val_usd - remaining_cost_usd
                    gain_sol = safe_decimal_divide(gain_usd, sol_price_decimal) if sol_price_decimal > Decimal('0') else gain_usd
                    total_unrealized_gain_sol += gain_sol
        
        return decimal_to_float(total_unrealized_gain_sol)
    
    @staticmethod
    async def fetch_bulk_prices(token_addresses: List[str]) -> Dict[str, float]:
        """
        Fetch current prices for multiple tokens from Jupiter Price API.
        
        Args:
            token_addresses: List of token mint addresses
            
        Returns:
            Dict mapping token_address -> price_usd (0.0 if not found or error)
        """
        if not token_addresses:
            return {}
        
        prices = {}
        
        # Jupiter Price API supports bulk requests via comma-separated IDs
        # Max ~100 tokens per request to avoid URL length issues
        batch_size = 100
        base_url = "https://api.jup.ag/price/v3"  # Migrated from lite-api.jup.ag/price/v2
        
        for i in range(0, len(token_addresses), batch_size):
            batch = token_addresses[i:i + batch_size]
            token_list = ",".join(batch)
            url = f"{base_url}?ids={token_list}"
            
            try:
                import aiohttp
                async with aiohttp.ClientSession() as session:
                    async with session.get(url, timeout=aiohttp.ClientTimeout(total=10)) as response:
                        response.raise_for_status()
                        data = await response.json()
                
                        # Jupiter returns: {"data": {"token_address": {"price": 0.123, ...}, ...}}
                        price_data = data.get("data", {})
                        for token_addr in batch:
                            token_info = price_data.get(token_addr, {})
                            price = token_info.get("price")
                            if price is not None:
                                try:
                                    prices[token_addr] = float(price)
                                except (ValueError, TypeError):
                                    prices[token_addr] = 0.0
                            else:
                                prices[token_addr] = 0.0
                        
            except aiohttp.ClientError as e:
                logger.warning(f"Failed to fetch prices from Jupiter: {e}")
                # Set all batch tokens to 0.0 on error
                for token_addr in batch:
                    prices[token_addr] = 0.0
            except (ValueError, KeyError, TypeError) as e:
                logger.warning(f"Failed to parse Jupiter price response: {e}")
                for token_addr in batch:
                    prices[token_addr] = 0.0
        
        return prices


class WalletAnalyzer:
    """
    Wallet analyzer for fetching and computing wallet metrics.
    
    In production, initialize with RPC/API credentials:
        analyzer = WalletAnalyzer(
            helius_api_key="...",
            rpc_url="https://mainnet.helius-rpc.com/?api-key=..."
        )
    """
    
    def __init__(
        self,
        helius_api_key: Optional[str] = None,
        rpc_url: Optional[str] = None,
        discover_wallets: bool = True,
        max_wallets: int = 50,
        budget_manager: Optional['PredictiveBudgetManager'] = None,
    ):
        """
        Initialize the wallet analyzer.

        Args:
            helius_api_key: Helius API key for transaction data
            rpc_url: Solana RPC URL for on-chain queries
            discover_wallets: Whether to discover wallets from on-chain data
            max_wallets: Maximum number of wallets to discover
            budget_manager: Optional PredictiveBudgetManager for API quota management
        """
        self.helius_api_key = helius_api_key
        self.rpc_url = rpc_url
        self._discover_wallets = discover_wallets
        self._max_wallets = max_wallets

        # Initialize budget manager if provided
        self._budget_manager = budget_manager

        # Initialize Helius client
        self.helius_client = HeliusClient(helius_api_key)
        
        # Initialize LiquidityProvider for historical liquidity collection
        db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
        self.liquidity_provider = LiquidityProvider(db_path=db_path)
        
        # Initialize RugCheck client if enabled
        self.rugcheck_client = None
        if SECURITY_AVAILABLE and ScoutConfig and ScoutConfig.get_rugcheck_enabled():
            try:
                self.rugcheck_client = RugCheckClient()
            except Exception as e:
                logger.warning(f"Failed to initialize RugCheck client: {e}")
        
        # Cache for metrics and trades (bounded to prevent memory growth)
        self._metrics_cache: OrderedDict = OrderedDict()
        self._metrics_cache_maxlen = 500
        self._trades_cache: OrderedDict = OrderedDict()
        self._trades_cache_maxlen = 500
        self._candidate_wallets: List[str] = []
        self._token_meta_cache: OrderedDict = OrderedDict()
        self._token_creation_cache: OrderedDict = OrderedDict()
        self._price_cache: OrderedDict = OrderedDict()  # Cache for token prices
        self._wallet_age_cache: Dict[str, Optional[float]] = {}
        self._sol_price_usd: Optional[float] = None  # Cached SOL price
        self._sol_price_lock = asyncio.Lock()
        self._safety_check_total: int = 0  # Cumulative token safety check count
        self._safety_check_failures: int = 0  # Cumulative safety check failures

        # Lock for thread-safe token safety cache access
        self._safety_cache_lock = asyncio.Lock()

        # Locks for thread-safe cache access
        self._metrics_cache_lock = asyncio.Lock()
        self._trades_cache_lock = asyncio.Lock()
        self._token_meta_cache_lock = asyncio.Lock()
        self._token_creation_cache_lock = asyncio.Lock()
        self._wallet_age_cache_lock = asyncio.Lock()

        # Parse cache for improved reliability - cache successful parse results by tx signature
        # Bounded with maxlen to prevent unbounded growth (worst offender for memory leaks)
        self._parse_cache: OrderedDict = OrderedDict()
        self._parse_cache_lock = asyncio.Lock()
        self._parse_cache_hits = 0
        self._parse_cache_misses = 0
        
        # Initialize Redis client for persistent caching (if available)
        self._redis_client = None
        try:
            from .redis_client import RedisClient
            if CONFIG_AVAILABLE and ScoutConfig and ScoutConfig.get_redis_enabled():
                redis_url = ScoutConfig.get_redis_url()
                self._redis_client = RedisClient(redis_url=redis_url, enabled=True)
                if self._redis_client.is_available():
                    logger.info("Redis cache enabled for token metadata and creation times")
                    # Share Redis client with HeliusClient for discovery cache & dedup
                    self.helius_client._redis = self._redis_client
                else:
                    logger.warning("Redis enabled but unavailable, using in-memory cache")
                    self._redis_client = None
        except ImportError:
            pass  # Redis not available
        
        # Known DEX program IDs for smart money detection
        self._dex_program_ids = {
            "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",  # Jupiter
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",  # Raydium
            "9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP",  # Orca
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",  # Whirlpool
            "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",  # PumpFun
        }
        self._jito_program_id = "Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb"
        self._jupiter_limit_order_program = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"  # Same as Jupiter, but check for limit order instructions

        # Max txs to pull per wallet when computing metrics/trades
        self._wallet_tx_limit = int(os.getenv("SCOUT_WALLET_TX_LIMIT", "500"))
        self._wallet_tx_limit = max(50, min(self._wallet_tx_limit, 5000))

        # Diagnostics: aggregate parse health across the entire run
        self._parse_stats = {
            "transactions_fetched": 0,
            "swaps_parsed": 0,
            "trades_valid": 0,
            "parse_failures_total": 0,
            "parse_failures_by_reason": {
                "no_primary_token": 0,
                "direction_ambiguous": 0,
                "not_involved": 0,
                "other": 0,
            },
            "token_creation_fetched": 0,
            "token_creation_success": 0,
            "token_creation_fallback_helix": 0,
            "parse_cache_hits": 0,
            "parse_cache_misses": 0,
        }
        self._discovery_stats = {
            "infrastructure_filtered": 0,
            "balance_checked": 0,
            "balance_filtered": 0,
            "wallets_with_no_trades": 0,
        }

    def can_spend_budget(self, estimated_credits: int = 100, category: Optional[str] = None) -> tuple[bool, str]:
        """
        Check if we have enough budget for an operation.

        Args:
            estimated_credits: Estimated credit cost for the operation
            category: Budget category (e.g., "discovery", "analysis", "validation")

        Returns:
            Tuple of (can_proceed: bool, reason: str)
        """
        if not self._budget_manager:
            return True, "No budget manager configured"

        try:
            from core.predictive_budget_manager import BudgetCategory

            # Map string category to BudgetCategory enum
            budget_category = BudgetCategory.ANALYSIS  # Default
            if category:
                category_map = {
                    "discovery": BudgetCategory.DISCOVERY,
                    "analysis": BudgetCategory.ANALYSIS,
                    "validation": BudgetCategory.VALIDATION,
                    "enrichment": BudgetCategory.ENRICHMENT,
                    "monitoring": BudgetCategory.MONITORING,
                }
                budget_category = category_map.get(category.lower(), BudgetCategory.ANALYSIS)

            # Get current snapshot
            snapshot = self._budget_manager.get_realtime_snapshot()

            # Check if we have enough credits remaining
            if snapshot.credits_remaining < estimated_credits:
                return False, f"Insufficient credits: {snapshot.credits_remaining:,} < {estimated_credits:,}"

            # Check alert level - block operations if critical
            if snapshot.alert_level.value in ["critical", "depleted"]:
                return False, f"Budget alert level: {snapshot.alert_level.value}"

            return True, "Budget OK"

        except Exception as e:
            logger.warning(f"Budget check failed: {e}. Proceeding with operation.")
            return True, f"Budget check failed: {e}"

    def record_credit_usage(self, credits: int, category: str = "analysis", value: float = 0.0):
        """
        Record credit usage after an operation.

        Args:
            credits: Number of credits consumed
            category: Budget category (discovery, analysis, validation, etc.)
            value: Value generated (e.g., high-WQS wallet found)
        """
        if not self._budget_manager:
            return

        try:
            from core.predictive_budget_manager import BudgetCategory

            # Map string category to BudgetCategory enum
            category_map = {
                "discovery": BudgetCategory.DISCOVERY,
                "analysis": BudgetCategory.ANALYSIS,
                "validation": BudgetCategory.VALIDATION,
                "enrichment": BudgetCategory.ENRICHMENT,
                "monitoring": BudgetCategory.MONITORING,
            }
            budget_category = category_map.get(category.lower(), BudgetCategory.ANALYSIS)

            self._budget_manager.record_category_usage(budget_category, credits, value)
            logger.debug(f"Recorded {credits} credits for {category}")

        except Exception as e:
            logger.warning(f"Failed to record credit usage: {e}")

    def get_budget_summary(self) -> dict:
        """Get current budget summary as a dict."""
        if not self._budget_manager:
            return {"status": "No budget manager configured"}

        try:
            return self._budget_manager.get_daily_summary()
        except Exception as e:
            return {"status": f"Error: {e}"}

    @classmethod
    async def create(
        cls,
        helius_api_key: Optional[str] = None,
        rpc_url: Optional[str] = None,
        discover_wallets: bool = False,
        max_wallets: int = 20,
        budget_manager: Optional['PredictiveBudgetManager'] = None,
    ):
        """
        Async factory method to create WalletAnalyzer with async initialization.

        This is the recommended way to create an analyzer when you need wallet discovery.

        Args:
            helius_api_key: API key for Helius
            rpc_url: Solana RPC URL (optional)
            discover_wallets: If True, discover wallets from on-chain data
            max_wallets: Maximum number of wallets to discover/analyze
            budget_manager: Optional PredictiveBudgetManager for API quota management

        Returns:
            Initialized WalletAnalyzer instance with wallets loaded

        Example:
            analyzer = await WalletAnalyzer.create(
                helius_api_key="your_key",
                discover_wallets=True,
                max_wallets=20
            )
        """
        # Create instance with synchronous __init__
        instance = cls(
            helius_api_key=helius_api_key,
            rpc_url=rpc_url,
            discover_wallets=discover_wallets,
            max_wallets=max_wallets,
            budget_manager=budget_manager,
        )
        
        # Perform async initialization
        await instance._async_init()
        
        return instance

    async def _async_init(self):
        """Async initialization for wallet loading and discovery."""
        # Try to load wallets from config file first
        wallet_list_file = os.getenv("SCOUT_WALLET_LIST_FILE", "/app/config/wallets.txt")
        if os.path.exists(wallet_list_file):
            try:
                with open(wallet_list_file, 'r') as f:
                    wallets = [line.strip() for line in f if line.strip() and not line.strip().startswith('#')]
                    if wallets:
                        self._candidate_wallets = wallets[:self._max_wallets]
                        print(f"[Analyzer] Loaded {len(self._candidate_wallets)} wallets from {wallet_list_file}")
                    else:
                        print("[Analyzer] Wallet list file empty, trying discovery...")
                        await self._try_discover_wallets_async()
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to load wallet list: {e}")
                await self._try_discover_wallets_async()
        else:
            # Try discovery or fall back to sample data
            await self._try_discover_wallets_async()

    async def clear_wallet_cache(self, address: str):
        """Clear cached data for a specific wallet to free memory."""
        async with self._metrics_cache_lock:
            self._metrics_cache.pop(address, None)
        async with self._trades_cache_lock:
            self._trades_cache.pop(address, None)
        # Note: We keep _token_meta_cache as that is reusable across wallets

    async def _metrics_cache_set(self, key: str, value: Any):
        async with self._metrics_cache_lock:
            self._metrics_cache[key] = value
            self._metrics_cache.move_to_end(key)
            if len(self._metrics_cache) > self._metrics_cache_maxlen:
                self._metrics_cache.popitem(last=False)

    async def _trades_cache_set(self, key: str, value: Any):
        async with self._trades_cache_lock:
            self._trades_cache[key] = value
            self._trades_cache.move_to_end(key)
            if len(self._trades_cache) > self._trades_cache_maxlen:
                self._trades_cache.popitem(last=False)

    async def clear_all_caches(self):
        """Clear all cached data to free memory."""
        async with self._metrics_cache_lock:
            self._metrics_cache.clear()
        async with self._trades_cache_lock:
            self._trades_cache.clear()
        async with self._token_meta_cache_lock:
            self._token_meta_cache.clear()
        async with self._token_creation_cache_lock:
            self._token_creation_cache.clear()
        self._price_cache.clear()
        async with self._parse_cache_lock:
            self._parse_cache.clear()

    def _parse_cache_set(self, key: str, value: Optional[Dict[str, Any]], maxlen: int = 5000):
        """Helper to set parse cache with automatic eviction (move to end on insertion)."""
        self._parse_cache[key] = value
        # If cache exceeds maxlen, evict oldest entries (FIFO)
        if len(self._parse_cache) > maxlen:
            for _ in range(len(self._parse_cache) - maxlen):
                self._parse_cache.popitem(last=False)

    def _ordered_cache_set(self, cache: OrderedDict, key: str, value: Any, maxlen: int = 500):
        """Helper to set ordered cache with automatic eviction (move to end on insertion)."""
        cache[key] = value
        cache.move_to_end(key)
        # If cache exceeds maxlen, evict oldest entries (FIFO)
        if len(cache) > maxlen:
            for _ in range(len(cache) - maxlen):
                cache.popitem(last=False)

    async def _try_discover_wallets_async(self):
        """Try to discover wallets asynchronously, fall back to sample data if fails."""
        if self._discover_wallets and self.helius_client.api_key:
            print("[Analyzer] Attempting to discover wallets from on-chain data...")
            try:
                # Check if sophisticated multi-timeframe discovery is enabled
                if CONFIG_AVAILABLE and ScoutConfig and ScoutConfig.get_multi_timeframe_enabled():
                    await self._discover_with_multi_timeframe_system()
                else:
                    # Fallback to manual sequential implementation
                    await self._discover_with_manual_implementation()
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to discover wallets: {e}")
                import traceback
                if os.getenv("SCOUT_VERBOSE", "false").lower() == "true":
                    traceback.print_exc()

    async def _discover_with_multi_timeframe_system(self):
        """Use sophisticated multi-timeframe discovery system (Sprint 4)."""
        from core.multitimeframe_discovery import get_multi_timeframe_discovery, DiscoveryTimeframe

        print("[Analyzer] Using sophisticated multi-timeframe discovery system...")

        # Initialize multi-timeframe discovery system
        mt_discovery = get_multi_timeframe_discovery(helius_client=self.helius_client)

        # Get configuration
        parallel = ScoutConfig.get_multi_timeframe_parallel()
        discovery_goal = ScoutConfig.get_multi_timeframe_goal()

        # Get max API calls budget
        max_api_calls = ScoutConfig.get_max_api_calls_per_run() if CONFIG_AVAILABLE else 500

        print(f"[Analyzer] Multi-timeframe configuration:")
        print(f"  Parallel execution: {parallel}")
        print(f"  Discovery goal: {discovery_goal}")
        print(f"  API budget: {max_api_calls} calls")

        try:
            # Execute multi-timeframe discovery with coordination
            result = await mt_discovery.discover_all_timeframes(
                budget_credits=max_api_calls,
                parallel=parallel,
                timeframes=[
                    DiscoveryTimeframe.DEEP,    # 720h historical
                    DiscoveryTimeframe.FAST,    # 24h recent
                    DiscoveryTimeframe.TRENDING # 4h trending
                ]
            )

            # Extract high-quality wallets from cross-timeframe ranking
            if result.cross_timeframe_ranking:
                # Use cross-timeframe deduplicated and ranked results
                ranked_wallets = [wallet for wallet, score in result.cross_timeframe_ranking]

                # Apply profitability pre-screen if enabled
                _profit_filter = ScoutConfig.get_discovery_profitability_filter() if CONFIG_AVAILABLE else True
                if _profit_filter and len(ranked_wallets) > self._max_wallets:
                    discovered = await self._profitability_pre_screen(ranked_wallets, self._max_wallets)
                else:
                    discovered = ranked_wallets[:self._max_wallets]

                self._candidate_wallets = discovered

                # Log sophisticated statistics
                print(f"[Multi-Timeframe] Discovery completed successfully:")
                print(f"  Total discovered: {result.total_unique_wallets} unique wallets")
                print(f"  Selected: {len(self._candidate_wallets)} for analysis")
                print(f"  Deduplication ratio: {result.deduplication_stats.get('deduplication_ratio', 0):.2%}")
                print(f"  Multi-timeframe wallets: {result.deduplication_stats.get('multi_timeframe_wallets', 0)}")
                print(f"  Total execution time: {result.total_execution_time_seconds:.1f}s")
                print(f"  Credits consumed: {result.total_credits_consumed}")

                # Log timeframe breakdown
                for tf, tf_result in result.timeframe_results.items():
                    print(f"  {tf.value}: {len(tf_result.wallets_discovered)} wallets, "
                          f"{tf_result.execution_time_seconds:.1f}s, "
                          f"{tf_result.credits_consumed} credits")

                # Save multi-timeframe discovery statistics to state persistence
                if STATE_PERSISTENCE_AVAILABLE and StatePersistence:
                    try:
                        persistence = StatePersistence()
                        persistence.save_multi_timeframe_discovery_stats(
                            result=result,
                            parallel=parallel,
                            discovery_goal=discovery_goal
                        )
                        print("[Multi-Timeframe] Statistics saved to state persistence")
                    except Exception as e:
                        print(f"[Multi-Timeframe] Failed to save statistics: {e}")

                return
            else:
                print("[Multi-Timeframe] No wallets discovered, falling back")

        except Exception as e:
            print(f"[Multi-Timeframe] Discovery failed: {e}, falling back to manual implementation")
            import traceback
            traceback.print_exc()
            # Fall through to manual implementation

    async def _discover_with_manual_implementation(self):
        """Fallback manual sequential implementation (legacy)."""
        print("[Analyzer] Using manual sequential discovery implementation...")

        # Get configuration from environment variables
        hours_back = int(os.getenv("SCOUT_DISCOVERY_HOURS", "24"))
        min_trade_count = int(os.getenv("SCOUT_MIN_TRADE_COUNT", "3"))

        # When profitability pre-screen is enabled, discover 2x wallets
        _profit_filter = os.getenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "true").lower() == "true"

        discovered_all: List[str] = []

        try:
            # Only apply 2x multiplier when max_wallets >= 50
            profit_multiplier = 2 if _profit_filter and self._max_wallets >= 50 else 1
            _discover = self._max_wallets * profit_multiplier

            print(f"[Analyzer] Running manual discovery scan ({hours_back}h window, max={_discover})...")
            manual_timeout = ScoutConfig.get_discovery_timeout_seconds() if CONFIG_AVAILABLE and ScoutConfig else 300

            # Check budget before calling Helius (estimate: 200 credits for discovery)
            estimated_credits = 200
            can_proceed, reason = self.can_spend_budget(estimated_credits, "discovery")
            if not can_proceed:
                print(f"[Analyzer] Budget check failed: {reason}")
                print(f"[Analyzer] Skipping wallet discovery due to budget constraints")
                return

            discovered = await asyncio.wait_for(
                self.helius_client.discover_wallets_from_recent_swaps(
                    limit=1000,
                    min_trade_count=min_trade_count,
                    max_wallets=_discover,
                    hours_back=hours_back,
                ),
                timeout=manual_timeout
            )

            # Record credit usage (value = number of wallets discovered)
            credits_used = estimated_credits if discovered else 50
            self.record_credit_usage(credits_used, "discovery", value=len(discovered) if discovered else 0)

            if discovered:
                print(f"[Analyzer] Manual discovery found {len(discovered)} wallets")
                discovered_all.extend(discovered)

        except asyncio.TimeoutError:
            print(f"[Analyzer] Manual discovery timeout after {manual_timeout}s, using partial results")
        except Exception as e:
            print(f"[Analyzer] Manual discovery failed: {e}")

        if discovered_all:
            # Deduplicate
            discovered = list(dict.fromkeys(discovered_all))  # Preserve order, dedup

            # Run profitability pre-screen if enabled
            if _profit_filter and len(discovered) > self._max_wallets:
                discovered = await self._profitability_pre_screen(discovered, self._max_wallets)
            else:
                discovered = discovered[:self._max_wallets]

            self._candidate_wallets = discovered
            print(f"[Analyzer] Manual discovery completed: {len(self._candidate_wallets)} candidate wallets")
            return
        else:
            print("[Analyzer] Manual discovery found no wallets, loading from database...")

        # Fallback: Try to load from existing roster database
        try:
            roster_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")

            for db_path in [roster_path]:
                from .db import _is_postgres, _is_sqlite
                db_available = _is_postgres() or (_is_sqlite() and os.path.exists(db_path))
                if db_available:
                    from .db import get_connection
                    conn = get_connection(db_path)
                    try:
                        cursor = conn.cursor()
                        cursor.execute("""
                            SELECT table_name FROM information_schema.tables
                            WHERE table_name = 'wallets'
                            UNION
                            SELECT name FROM sqlite_master
                            WHERE type='table' AND name='wallets'
                        """)
                        if cursor.fetchone():
                            cursor.execute("""
                                SELECT DISTINCT address, archetype, last_arb_check_at
                                FROM wallets
                                WHERE status IN ('ACTIVE', 'CANDIDATE')
                                AND (archetype != 'ARBITRAGE' OR last_arb_check_at IS NULL OR last_arb_check_at < NOW() - INTERVAL '24 hours')
                                ORDER BY wqs_score DESC NULLS LAST
                                LIMIT ?
                            """, (self._max_wallets,))
                            existing_wallets = [row[0] for row in cursor.fetchall()]

                            if existing_wallets:
                                self._candidate_wallets = existing_wallets[:self._max_wallets]
                                print(f"[Analyzer] Loaded {len(self._candidate_wallets)} wallets from existing database ({db_path})")
                                return
                    finally:
                        conn.close()
        except Exception as e:
            print(f"[Analyzer] Warning: Failed to load from database: {e}")
        
        # Final fallback: sample data
        if not self.helius_client.api_key:
            print("[Analyzer] No Helius API key found, using sample data")
        else:
            print("[Analyzer] No wallets discovered, using sample data")
        self._load_sample_data()
    
    def _load_sample_data(self):
        """Load sample wallet data for testing."""
        # Sample wallets for testing
        self._candidate_wallets = [
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
            "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321",
            "3jHgFdAbCdEfGhIjKlMnOpQrStUvWxYz1122334455",
            "8wQpRsAbCdEfGhIjKlMnOpQrStUvWxYz6677889900",
        ]
        
        # Sample metrics cache (in production, fetch from chain)
        self._metrics_cache = {
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU": WalletMetrics(
                address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                roi_7d=12.5,
                roi_30d=45.2,
                trade_count_30d=127,
                win_rate=0.72,
                max_drawdown_30d=8.5,
                avg_trade_size_sol=0.5,
                last_trade_at=(utcnow() - timedelta(hours=2)).isoformat(),
                win_streak_consistency=0.68,
            ),
            "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890": WalletMetrics(
                address="9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                roi_7d=8.3,
                roi_30d=32.8,
                trade_count_30d=89,
                win_rate=0.65,
                max_drawdown_30d=12.1,
                avg_trade_size_sol=0.3,
                last_trade_at=(utcnow() - timedelta(hours=6)).isoformat(),
                win_streak_consistency=0.55,
            ),
            "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321": WalletMetrics(
                address="5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321",
                roi_7d=150.0,  # Suspicious spike!
                roi_30d=25.0,
                trade_count_30d=15,  # Low trade count
                win_rate=0.80,
                max_drawdown_30d=5.0,
                avg_trade_size_sol=1.2,
                last_trade_at=(utcnow() - timedelta(hours=1)).isoformat(),
                win_streak_consistency=0.40,
            ),
            "3jHgFdAbCdEfGhIjKlMnOpQrStUvWxYz1122334455": WalletMetrics(
                address="3jHgFdAbCdEfGhIjKlMnOpQrStUvWxYz1122334455",
                roi_7d=-5.0,
                roi_30d=-15.0,
                trade_count_30d=45,
                win_rate=0.35,
                max_drawdown_30d=35.0,  # High drawdown
                avg_trade_size_sol=0.8,
                last_trade_at=(utcnow() - timedelta(days=3)).isoformat(),
                win_streak_consistency=0.20,
            ),
            "8wQpRsAbCdEfGhIjKlMnOpQrStUvWxYz6677889900": WalletMetrics(
                address="8wQpRsAbCdEfGhIjKlMnOpQrStUvWxYz6677889900",
                roi_7d=5.0,
                roi_30d=18.5,
                trade_count_30d=52,
                win_rate=0.58,
                max_drawdown_30d=10.0,
                avg_trade_size_sol=0.4,
                last_trade_at=(utcnow() - timedelta(hours=12)).isoformat(),
                win_streak_consistency=0.50,
            ),
        }
        
        # Sample historical trades for backtesting
        self._trades_cache = self._generate_sample_trades()
    
    def _generate_sample_trades(self) -> dict:
        """Generate sample historical trades for each wallet."""
        trades_cache = {}
        
        # Known tokens for sample trades
        tokens = [
            ("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263", "BONK"),
            ("EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm", "WIF"),
            ("7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr", "POPCAT"),
        ]
        
        for wallet in self._candidate_wallets:
            trades = []
            metrics = self._metrics_cache.get(wallet)
            if not metrics:
                continue
            
            # Generate trades based on metrics
            num_trades = min(metrics.trade_count_30d or 10, 30)  # Cap at 30 for sample
            
            for i in range(num_trades):
                token_addr, token_symbol = tokens[i % len(tokens)]
                days_ago = (i * 30) // num_trades  # Spread across 30 days
                
                # Alternate buy/sell
                action = TradeAction.BUY if i % 2 == 0 else TradeAction.SELL
                
                # Calculate PnL based on win rate
                import random
                is_win = random.random() < (metrics.win_rate or 0.5)
                pnl = random.uniform(0.01, 0.1) if is_win else random.uniform(-0.05, 0)
                
                trade = HistoricalTrade(
                    token_address=token_addr,
                    token_symbol=token_symbol,
                    action=action,
                    amount_sol=(metrics.avg_trade_size_sol or Decimal('0.5')),
                    price_at_trade=Decimal(str(random.uniform(0.00001, 10.0))),
                    timestamp=utcnow() - timedelta(days=days_ago, hours=random.randint(0, 23)),
                    tx_signature=f"{wallet[:8]}_{i}",
                    pnl_sol=Decimal(str(pnl)) if action == TradeAction.SELL else Decimal('0'),
                    liquidity_at_trade_usd=Decimal(str(random.uniform(50000, 500000))),
                )
                trades.append(trade)
            
            trades_cache[wallet] = sorted(trades, key=lambda t: t.timestamp, reverse=True)
        
        return trades_cache
    
    async def _profitability_pre_screen(
        self,
        wallets: List[str],
        max_wallets: int,
    ) -> List[str]:
        """
        Quick profitability filter before full wallet analysis.
        
        Fetches SOL balances for discovered candidates and ranks them by
        on-chain wealth as a proxy for profitability. This prevents Scout
        from wasting analysis time on empty/dust wallets.
        
        Args:
            wallets: Discovered wallet addresses (sorted by trade count)
            max_wallets: Maximum wallets to retain after filtering
            
        Returns:
            Filtered list of up to max_wallets, sorted by SOL balance desc
        """
        if not wallets:
            return []
        
        print(f"[Analyzer] Profitability pre-screen: checking {len(wallets)} candidates...")

        # Check budget before calling Helius (estimate: 10 credits per wallet)
        estimated_credits = len(wallets) * 10
        can_proceed, reason = self.can_spend_budget(estimated_credits, "discovery")
        if not can_proceed:
            print(f"[Analyzer] Budget check failed: {reason}")
            print(f"[Analyzer] Skipping profitability pre-screen, using first {max_wallets} wallets")
            return wallets[:max_wallets]

        try:
            balances = await self.helius_client.get_wallet_sol_balances(wallets)

            non_zero = {w: b for w, b in balances.items() if b > 0.01}

            # Record credit usage (value = number of non-zero wallets found)
            self.record_credit_usage(estimated_credits, "discovery", value=len(non_zero))
            print(f"[Analyzer] Pre-screen: {len(non_zero)}/{len(wallets)} wallets have > 0.01 SOL balance")
            
            scored = [(w, balances.get(w, 0.0)) for w in wallets]
            scored.sort(key=lambda x: x[1], reverse=True)
            
            result = [w for w, _ in scored[:max_wallets]]
            print(f"[Analyzer] Pre-screen complete: retained {len(result)} candidates")
            return result
        except Exception as e:
            print(f"[Analyzer] Pre-screen failed ({e}), falling through to all candidates")
            return wallets[:max_wallets]
    
    def get_candidate_wallets(self) -> List[str]:
        """
        Get list of candidate wallet addresses to analyze.
        
        In production, this would:
        1. Query known wallet lists/APIs
        2. Filter by activity level
        3. Return addresses for detailed analysis
        
        Returns:
            List of wallet addresses
        """
        return self._candidate_wallets
    
    async def get_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
        """
        Get metrics for a specific wallet.

        Fetches real transaction history from Helius API and calculates
        ROI, win rate, drawdown from actual trades.

        Args:
            address: Wallet address to analyze

        Returns:
            WalletMetrics object or None if wallet not found
        """
        print(f"  [{address[:8]}] Checking cache...")
        async with self._metrics_cache_lock:
            if address in self._metrics_cache:
                print(f"  [{address[:8]}] Found in cache")
                return self._metrics_cache[address]

        print(f"  [{address[:8]}] Not in cache, checking database...")
        # Try to load from database first (if wallet exists there)
        try:
            db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
            # On the PostgreSQL backend the SQLite file path is irrelevant —
            # the pool connects to DATABASE_URL regardless. Only skip the DB
            # lookup when running genuinely file-backed SQLite whose file is
            # missing.
            from .db import _is_postgres, _is_sqlite
            db_available = _is_postgres() or (_is_sqlite() and os.path.exists(db_path))
            if db_available:
                from .db import get_connection
                conn = get_connection(db_path)
                try:
                    cursor = conn.cursor()
                    cursor.execute("""
                        SELECT wqs_score, roi_7d, roi_30d, trade_count_30d, win_rate,
                               max_drawdown_30d, avg_trade_size_sol, last_trade_at
                        FROM wallets
                        WHERE address = %s
                        LIMIT 1
                    """, (address,))
                    row = cursor.fetchone()

                    if row:
                        roi_7d = float(row['roi_7d']) if row['roi_7d'] is not None else None
                        roi_30d = float(row['roi_30d']) if row['roi_30d'] is not None else None
                        trade_count_30d = row['trade_count_30d']
                        win_rate = float(row['win_rate']) if row['win_rate'] is not None else None
                        max_drawdown_30d = float(row['max_drawdown_30d']) if row['max_drawdown_30d'] is not None else None
                        avg_trade_size_sol = row['avg_trade_size_sol']
                        last_trade_at = row['last_trade_at']

                        is_stale = False
                        if last_trade_at:
                            try:
                                lt_str = str(last_trade_at).replace("Z", "+00:00")
                                lt_dt = datetime.fromisoformat(lt_str)
                                if lt_dt.tzinfo is None:
                                    lt_dt = lt_dt.replace(tzinfo=timezone.utc)
                                age = utcnow() - lt_dt
                                if age.days > 30:
                                    is_stale = True
                                    print(f"  [{address[:8]}] DB metrics stale (last trade {age.days}d ago), will re-fetch")
                            except (ValueError, TypeError):
                                pass

                        if is_stale:
                            pass
                        elif any(x is not None for x in [roi_7d, roi_30d, trade_count_30d, win_rate]):
                            metrics = WalletMetrics(
                                address=address,
                                roi_7d=roi_7d,
                                roi_30d=roi_30d,
                                trade_count_30d=trade_count_30d,
                                win_rate=win_rate,
                                max_drawdown_30d=max_drawdown_30d,
                                avg_trade_size_sol=avg_trade_size_sol,
                                last_trade_at=last_trade_at,
                                win_streak_consistency=None,
                            )
                            await self._metrics_cache_set(address, metrics)
                            return metrics
                finally:
                    conn.close()
        except Exception as e:
            logger.warning("Failed to load metrics from database for %s: %s", address[:8], e)

        # Fetch real data if Helius client is available
        if self.helius_client.api_key:
            try:
                metrics = await self._fetch_real_wallet_metrics(address)
                if metrics:
                    await self._metrics_cache_set(address, metrics)
                    return metrics
            except Exception as e:
                logger.warning("Failed to fetch metrics for %s: %s", address[:8], e)
        
        # Fall back to cached sample data
        return self._metrics_cache.get(address)
    
    async def _fetch_real_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
        """Fetch real wallet metrics from Helius API."""
        print(f"  [{address[:8]}] Fetching transactions (limit={self._wallet_tx_limit})...")

        # Check budget before calling Helius (estimate: 50 credits per wallet fetch)
        estimated_credits = 50
        can_proceed, reason = self.can_spend_budget(estimated_credits, "analysis")
        if not can_proceed:
            print(f"  [{address[:8]}] Budget check failed: {reason}")
            print(f"  [{address[:8]}] Skipping wallet analysis due to budget constraints")
            return None

        # Get transaction history
        transactions = await self.helius_client.get_wallet_transactions(
            address,
            days=30,
            limit=self._wallet_tx_limit,
        )

        # Record credit usage (value = 1 if we got transactions, 0 otherwise)
        credits_used = estimated_credits if transactions else 10  # Partial credit even if no results
        self.record_credit_usage(credits_used, "analysis", value=1 if transactions else 0)

        print(f"  [{address[:8]}] Received {len(transactions) if transactions else 0} transactions")
        
        if not transactions:
            print(f"  [{address[:8]}] No transactions found")
            return None
        
        print(f"  [{address[:8]}] Parsing {len(transactions)} transactions into trades...")
        # Parse transactions into trades
        trades = []
        parse_failures = 0
        trade_failures = 0
        self._parse_stats["transactions_fetched"] += len(transactions)
        for i, tx in enumerate(transactions):
            if i % 25 == 0 and i > 0:
                print(f"  [{address[:8]}] Progress: {i}/{len(transactions)} txs, {len(trades)} trades, {parse_failures} parse fails, {trade_failures} trade fails")
            
            # Log first transaction completely for debugging (SCOUT_DEBUG_TX_DUMP env var)
            if i == 0 and os.getenv("SCOUT_DEBUG_TX_DUMP", "false").lower() == "true":
                import json
                print(f"  [{address[:8]}] ━━━ FIRST TRANSACTION STRUCTURE ━━━")
                tx_json = json.dumps(tx, indent=2, default=str)
                # Log in chunks to avoid overwhelming output
                lines = tx_json.split('\n')
                for j in range(min(100, len(lines))):  # First 100 lines
                    print(f"  [{address[:8]}] {lines[j]}")
                if len(lines) > 100:
                    print(f"  [{address[:8]}] ... ({len(lines) - 100} more lines)")
                print(f"  [{address[:8]}] ━━━ END TRANSACTION STRUCTURE ━━━")
            
            # Check parse cache first (by transaction signature)
            tx_sig = tx.get("signature", "")
            swap = None

            # Use async lock to prevent race conditions on the OrderedDict between coroutines
            async with self._parse_cache_lock:
                cached = self._parse_cache.get(tx_sig)
            if cached is not None:
                self._parse_cache_hits += 1
                swap = cached
            else:
                self._parse_cache_misses += 1
                # Attempt standard parsing
                swap = self.helius_client.parse_swap_transaction(tx, wallet_address=address)

                # Cache the result (even if None) to avoid re-parsing
                async with self._parse_cache_lock:
                    self._parse_cache[tx_sig] = swap

            if swap:
                self._parse_stats["swaps_parsed"] += 1
                # Convert to HistoricalTrade format
                trade = await self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
                    self._parse_stats["trades_valid"] += 1
                else:
                    trade_failures += 1
            else:
                parse_failures += 1
                self._parse_stats["parse_failures_total"] += 1
                reason = self._categorize_parse_failure(tx, address)
                self._parse_stats["parse_failures_by_reason"][reason] += 1
                # Log first few failures for debugging
                if parse_failures <= 3:
                    tx_type = tx.get("type", "unknown")
                    tx_sig_short = tx.get("signature", "")[:8]
                    print(f"  [{address[:8]}] Parse fail #{parse_failures}: type={tx_type}, sig={tx_sig_short}..., reason={reason}")
                    # Log key fields
                    print(f"  [{address[:8]}]   - tokenTransfers: {len(tx.get('tokenTransfers', []))} items")
                    print(f"  [{address[:8]}]   - nativeTransfers: {len(tx.get('nativeTransfers', []))} items")
                    print(f"  [{address[:8]}]   - accountData: {len(tx.get('accountData', []))} items")
                    if tx.get('events'):
                        print(f"  [{address[:8]}]   - events: {list(tx.get('events', {}).keys())}")
                    if reason == "unknown":
                        print(f"  [{address[:8]}]   - tx keys: {list(tx.keys())}")
                        # Log type-specific fields for SWAP and non-SWAP
                        if tx.get("description"):
                            print(f"  [{address[:8]}]   - description: {tx['description'][:120]}")
                        if tx.get("instructions"):
                            print(f"  [{address[:8]}]   - instructions: {len(tx['instructions'])} items")
                        if tx.get("source"):
                            print(f"  [{address[:8]}]   - source: {tx['source']}")
        
        print(f"  [{address[:8]}] Parsed {len(trades)} trades from {len(transactions)} transactions")

        # Cache parsed trades so the backtest validator (main.py:555) can access them.
        # Without this, _trades_cache is empty and every ACTIVE-qualified wallet is
        # silently demoted to CANDIDATE at the "No trades" backtest branch.
        if trades:
            await self._trades_cache_set(address, trades)

        # Phase 5a: Telegram bot detection
        # Count swaps routed through known bot routers (programId or feePayer)
        bot_swap_count = 0
        if self.helius_client.KNOWN_BOT_ROUTERS:
            for tx in transactions:
                if tx.get("type") == "SWAP":
                    # Check if executing programId is a known bot router
                    for ix in tx.get("instructions", []):
                        if ix.get("programId") in self.helius_client.KNOWN_BOT_ROUTERS:
                            bot_swap_count += 1
                            break
                    # Also check feePayer
                    if tx.get("feePayer") in self.helius_client.KNOWN_BOT_ROUTERS:
                        bot_swap_count += 1

        # Calculate bot swap ratio
        bot_swap_ratio = 0.0
        total_swaps = len([tx for tx in transactions if tx.get("type") == "SWAP"])
        if total_swaps >= 10:  # Only flag if wallet has at least 10 swaps
            bot_swap_ratio = bot_swap_count / max(1, total_swaps)

        print(f"  [{address[:8]}] Bot detection: {bot_swap_count}/{total_swaps} swaps ({bot_swap_ratio:.1%})")

        # Compute per-wallet parse rate
        total_fetched = len(transactions)
        total_parsed = len(trades)
        parse_rate = total_parsed / max(1, total_fetched)
        # Include partial-parse failures in the denominator — if 60%+ of transactions
        # cannot be parsed, the wallet's activity is too opaque to evaluate reliably.
        is_unproven_from_parse = (total_fetched > 0 and parse_rate < 0.30)

        if not trades:
            print(f"  [{address[:8]}] No valid trades found after parsing")
            self._discovery_stats["wallets_with_no_trades"] += 1
            return None

        # Compute DEX diversity from raw Helius transactions (source field)
        dex_sources = {
            tx.get("source") for tx in transactions
            if tx.get("source") and tx.get("source") not in ("UNKNOWN", "")
        }
        dex_diversity = len(dex_sources) if dex_sources else None

        # Detect limit order and Jito MEV usage from raw transaction data.
        # Jito: any native SOL transfer to a known Jito tip account.
        # Limit orders: Helius sets source="JUPITER_LIMIT" for limit-order fills, or the
        #   Jupiter Limit Order v2 program appears in the instructions programId list.
        _jito_tip_accounts = {
            "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4",
            "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
            "Cw8CFyM9FkoMi7K918YFiz4gBC9MDiSrqwR775XZdTJ5",
            "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt13UZMCSj",
            "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
            "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
            "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
            "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
        }
        _jupiter_limit_program = "j1o2qRpjcyUwEvwtcfhEQefh773ZgjxcVRry7LDqg5X"

        uses_limit_orders = False
        uses_mev_protection = False
        for tx in transactions:
            if not uses_limit_orders:
                if tx.get("source") == "JUPITER_LIMIT":
                    uses_limit_orders = True
                else:
                    for ix in tx.get("instructions", []):
                        if ix.get("programId") == _jupiter_limit_program:
                            uses_limit_orders = True
                            break
            if not uses_mev_protection:
                for nt in tx.get("nativeTransfers", []):
                    if nt.get("toUserAccount") in _jito_tip_accounts:
                        uses_mev_protection = True
                        break

        print(f"  [{address[:8]}] Smart money: limit_orders={uses_limit_orders}, mev_protection={uses_mev_protection}")

        # Phase 5b: MEV/Sandwich Risk Detection
        # Analyze nativeTransfers for sandwich attack patterns.
        # A sandwich attack typically has: frontrun buy → victim buy → backrun sell
        # within the same block. We approximate by checking if the wallet's
        # swap transactions appear in blocks with multiple swaps to the same token.
        mev_risk_score: Optional[float] = None
        try:
            swap_txs = [
                tx for tx in transactions
                if tx.get("type") == "SWAP" and tx.get("tokenTransfers")
            ]
            if swap_txs:
                # Group swap transactions by block timestamp (hour bucket as proxy)
                sandwich_suspicious = 0
                for tx in swap_txs:
                    token_transfers = tx.get("tokenTransfers", [])
                    native_transfers = tx.get("nativeTransfers", [])
                    # Simple heuristic: if swap has >3 token transfers AND the wallet
                    # is not the only participant, it may be in a sandwich
                    if len(token_transfers) > 3 and any(
                        nt.get("toUserAccount") in _jito_tip_accounts
                        for nt in native_transfers
                    ):
                        sandwich_suspicious += 1
                mev_risk_score = sandwich_suspicious / max(1, len(swap_txs))
        except Exception as e:
            logger.warning(f"MEV risk check failed for {address}: {e}", exc_info=True)

        print(f"  [{address[:8]}] Calculating metrics from {len(trades)} trades...")
        # Calculate metrics from trades
        metrics = await self._calculate_metrics_from_trades(
            address, trades,
            dex_diversity_score=dex_diversity,
            uses_limit_orders=uses_limit_orders,
            uses_mev_protection=uses_mev_protection,
            is_unproven_from_parse=is_unproven_from_parse,
            parse_rate=parse_rate,
            mev_risk_score=mev_risk_score,
            is_tg_bot_user=(bot_swap_ratio >= 0.5 and total_swaps >= 10),
            transactions=transactions,
        )
        if metrics:
            print(f"  [{address[:8]}] ✓ Metrics calculated successfully")
        else:
            print(f"  [{address[:8]}] ✗ Metrics calculation returned None")
        return metrics
    
    async def _parse_swap_to_trade(self, swap: Dict[str, Any], wallet: str) -> Optional[HistoricalTrade]:
        """Parse a swap transaction into a HistoricalTrade."""
        try:
            # Robust swap parsing already produced wallet-relative quantities
            direction = (swap.get("direction") or "").upper()
            if direction not in ("BUY", "SELL"):
                return None

            action = TradeAction.BUY if direction == "BUY" else TradeAction.SELL
            timestamp = datetime.fromtimestamp(
                swap.get("timestamp", int(utcnow().timestamp())),
                tz=timezone.utc
            )

            token_mint = swap.get("token_mint", "") or swap.get("token_out", "")
            # Convert all financial values to Decimal immediately
            token_amount = float_to_decimal(swap.get("token_amount") or 0.0)
            sol_amount_raw = swap.get("sol_amount")
            price_sol_raw = swap.get("price_sol")
            price_usd_raw = swap.get("price_usd")
            usd_amount_raw = swap.get("usd_amount")

            sol_amount: Decimal = float_to_decimal(sol_amount_raw) if sol_amount_raw is not None else Decimal('0')
            price_sol: Decimal = float_to_decimal(price_sol_raw) if price_sol_raw is not None else Decimal('0')
            price_usd: Optional[Decimal] = float_to_decimal(price_usd_raw) if price_usd_raw is not None else None

            # If this was a token->token swap valued in USD, derive SOL notional using SOL/USD.
            if sol_amount_raw is None and usd_amount_raw is not None:
                try:
                    usd_amount = float_to_decimal(usd_amount_raw)
                    sol_price_usd_float = await self.liquidity_provider.get_sol_price_usd()
                    sol_price_usd = float_to_decimal(sol_price_usd_float)
                    if usd_amount > Decimal('0') and sol_price_usd > Decimal('0'):
                        sol_amount = safe_decimal_divide(usd_amount, sol_price_usd)
                        price_sol = safe_decimal_divide(sol_amount, token_amount) if token_amount > Decimal('0') else Decimal('0')
                except Exception as e:
                    logger.debug(f"Token price estimation failed: {e}")

            # Token metadata enrichment (symbol/decimals)
            token_symbol = swap.get("token_symbol") or None
            if not token_symbol or token_symbol == "UNKNOWN":
                token_symbol = await self._get_token_symbol_async(token_mint) or "UNKNOWN"

            trade = HistoricalTrade(
                token_address=token_mint,
                token_symbol=token_symbol,
                action=action,
                amount_sol=sol_amount,  # SOL notional (spent/received)
                price_at_trade=price_sol,  # SOL per token
                timestamp=timestamp,
                tx_signature=swap.get("signature", ""),
                pnl_sol=None,
                liquidity_at_trade_usd=None,
                token_amount=token_amount,
                sol_amount=sol_amount,
                price_sol=price_sol,
                price_usd=price_usd,
            )
            # If we didn't get USD price directly, derive it from SOL/USD.
            if trade.price_usd is None and trade.price_sol and trade.price_sol > Decimal('0'):
                sol_price_usd_float = await self.liquidity_provider.get_sol_price_usd()
                sol_price_usd = float_to_decimal(sol_price_usd_float)
                if sol_price_usd > Decimal('0'):
                    trade.price_usd = trade.price_sol * sol_price_usd

            return trade
        except Exception as e:
            print(f"[Analyzer] Error parsing swap: {e}")
            return None

    async def _get_token_symbol(self, token_mint: str) -> Optional[str]:
        """Best-effort token symbol lookup with caching."""
        if not token_mint:
            return None
        
        # Check Redis cache first (persistent across restarts)
        if self._redis_client and self._redis_client.is_available():
            try:
                import json
                cache_key = f"token_meta:{token_mint}"
                cached_json = self._redis_client.get(cache_key)
                if cached_json:
                    cached_meta = json.loads(cached_json)
                    async with self._token_meta_cache_lock:
                        self._token_meta_cache[token_mint] = cached_meta
                    return cached_meta.get("symbol")
            except Exception as e:
                logger.debug(f"Redis cache read failed for token meta: {e}")
        
        # Check in-memory cache
        async with self._token_meta_cache_lock:
            if token_mint in self._token_meta_cache:
                return self._token_meta_cache[token_mint].get("symbol")

        # 1) Known tokens map
        if hasattr(self.liquidity_provider, "KNOWN_TOKENS") and token_mint in self.liquidity_provider.KNOWN_TOKENS:
            symbol = self.liquidity_provider.KNOWN_TOKENS[token_mint][0]
            meta = {"symbol": symbol}
            async with self._token_meta_cache_lock:
                self._token_meta_cache[token_mint] = meta
            # Cache in Redis
            if self._redis_client and self._redis_client.is_available():
                try:
                    import json
                    cache_key = f"token_meta:{token_mint}"
                    self._redis_client.set(cache_key, json.dumps(meta), ttl_seconds=7 * 24 * 3600)
                except Exception:
                    pass
            return symbol

        async with self._token_meta_cache_lock:
            self._token_meta_cache[token_mint] = {}
        # Cache empty result in Redis to avoid repeated API calls
        if self._redis_client and self._redis_client.is_available():
            try:
                import json
                cache_key = f"token_meta:{token_mint}"
                self._redis_client.set(cache_key, json.dumps({}), ttl_seconds=24 * 3600)  # 1 day for empty results
            except Exception:
                pass
        return None

    async def _get_token_symbol_async(self, token_mint: str) -> Optional[str]:
        """Async variant of _get_token_symbol that also queries Birdeye when available."""
        # Check sync caches first (avoids an I/O round-trip for already-known tokens)
        symbol = await self._get_token_symbol(token_mint)
        if symbol:
            return symbol

        birdeye = getattr(self.liquidity_provider, "birdeye_client", None)
        if birdeye is None:
            return None

        try:
            meta = await birdeye.get_token_metadata(token_mint)
            if meta and meta.get("symbol"):
                enriched = {"symbol": meta["symbol"]}
                async with self._token_meta_cache_lock:
                    self._token_meta_cache[token_mint] = enriched
                if self._redis_client and self._redis_client.is_available():
                    try:
                        import json as _json
                        self._redis_client.set(
                            f"token_meta:{token_mint}",
                            _json.dumps(enriched),
                            ttl_seconds=7 * 24 * 3600,
                        )
                    except Exception:
                        pass
                return meta["symbol"]
        except Exception as exc:
            logger.debug(f"Birdeye symbol lookup failed for {token_mint[:8]}: {exc}")

        return None

    @staticmethod
    def _replay_positions(
        trades: List[HistoricalTrade],
    ) -> Tuple[Decimal, Decimal, Dict[str, Dict[str, Decimal]], Dict[int, Decimal], float]:
        """
        Replay trades chronologically with FIFO cost basis tracking.

        Returns:
            total_cost_sold: Sum of cost basis of all SELL trades
            realized_pnl: Sum of realized PnL from SELL trades
            open_positions: Dict of token -> {qty, cost_sol}
            per_trade_pnl: Dict of sorted index -> pnl_sol for each SELL trade
            replay_data_gap_ratio: Ratio of SELL events with data gaps to total SELL events
        """
        has_swap_fields = any(t.sol_amount is not None or t.token_amount is not None for t in trades)

        EPSILON = Decimal('1e-9')
        positions: Dict[str, Dict[str, Decimal]] = {}
        total_cost_sold = Decimal('0')
        realized_pnl_total = Decimal('0')
        per_trade_pnl: Dict[int, Decimal] = {}

        # Track data gap metrics
        sell_events_count = 0
        mismatched_sell_events_count = 0

        sorted_trades = sorted(trades, key=lambda t: t.timestamp)

        for idx, t in enumerate(sorted_trades):
            token = t.token_address

            if has_swap_fields:
                token_qty = t.token_amount
                sol_amt = t.sol_amount if t.sol_amount is not None else t.amount_sol

                if token_qty is None or token_qty <= Decimal('0'):
                    if t.price_at_trade and t.price_at_trade > Decimal('0') and sol_amt and sol_amt > Decimal('0'):
                        token_qty = safe_decimal_divide(sol_amt, t.price_at_trade)

                if token_qty is None or token_qty <= Decimal('0') or sol_amt is None or sol_amt <= Decimal('0'):
                    continue
            else:
                qty = float_to_decimal(t.amount_sol or Decimal('0'))
                price = float_to_decimal(t.price_at_trade or Decimal('0'))
                if qty <= EPSILON or price <= EPSILON:
                    continue

            if t.action == TradeAction.BUY:
                pos = positions.setdefault(token, {"qty": Decimal('0'), "cost_sol": Decimal('0')})
                if has_swap_fields:
                    pos["qty"] += token_qty
                    pos["cost_sol"] += sol_amt
                else:
                    pos["qty"] += qty
                    pos["cost_sol"] += qty * price

            elif t.action == TradeAction.SELL:
                sell_events_count += 1
                pos = positions.get(token)
                if not pos or pos["qty"] < EPSILON:
                    continue

                if has_swap_fields:
                    sell_qty = min(token_qty, pos["qty"])
                    
                    # Proportional scaling when data gap detected
                    if token_qty > pos["qty"]:
                        mismatched_sell_events_count += 1
                        scale = safe_decimal_divide(pos["qty"], token_qty)
                        sell_val = sol_amt * scale
                    else:
                        sell_val = sol_amt
                else:
                    sell_qty = min(qty, pos["qty"])
                    sell_val = float_to_decimal(t.pnl_sol or Decimal('0'))

                if sell_qty < EPSILON:
                    continue

                avg_cost = safe_decimal_divide(pos["cost_sol"], pos["qty"])
                cost_basis = avg_cost * sell_qty

                if has_swap_fields:
                    pnl_val = sell_val - cost_basis
                else:
                    pnl_val = sell_val

                total_cost_sold += cost_basis
                realized_pnl_total += pnl_val
                per_trade_pnl[idx] = pnl_val

                pos["qty"] -= sell_qty
                pos["cost_sol"] -= cost_basis

                if pos["qty"] < EPSILON:
                    positions.pop(token, None)
                else:
                    pos["cost_sol"] = max(Decimal('0'), pos["cost_sol"])

        # Calculate replay data gap ratio
        replay_data_gap_ratio = 0.0
        if sell_events_count > 0:
            replay_data_gap_ratio = float(mismatched_sell_events_count) / float(sell_events_count)

        return total_cost_sold, realized_pnl_total, positions, per_trade_pnl, replay_data_gap_ratio

    def _enrich_trades_with_realized_pnl(self, trades: List[HistoricalTrade]) -> List[HistoricalTrade]:
        """
        Compute realized PnL (in SOL) for SELL trades using average cost basis.

        This makes metrics like win-rate and drawdown meaningful even when the
        raw swap payload doesn't directly include PnL.
        """
        if all(t.token_amount is None and t.sol_amount is None and t.price_sol is None for t in trades):
            return trades

        _, _, _, per_trade_pnl, replay_data_gap_ratio = self._replay_positions(trades)

        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for idx, pnl in per_trade_pnl.items():
            sorted_trades[idx].pnl_sol = pnl

        return trades
    
    async def _fetch_token_creation_time(self, token_address: str) -> Optional[float]:
        """
        Fetch token creation timestamp with multi-source fallback.
        
        Sources (in order):
        1. Cache
        2. Birdeye API (best for DeFi tokens)
        3. Known tokens hardcoded list
        
        Args:
            token_address: Token mint address
            
        Returns:
            Unix timestamp of token creation, or None
        """
        if not token_address:
            return None
        
        # Check Redis cache first (persistent across restarts)
        if self._redis_client and self._redis_client.is_available():
            try:
                import json
                cache_key = f"token_creation:{token_address}"
                cached_json = self._redis_client.get(cache_key)
                if cached_json:
                    cached_data = json.loads(cached_json)
                    # Handle None values (cached as "null" string)
                    if cached_data == "null" or cached_data is None:
                        return None
                    return float(cached_data)
            except Exception as e:
                logger.debug(f"Redis cache read failed for token creation: {e}")
        
        # Check in-memory cache
        async with self._token_creation_cache_lock:
            if token_address in self._token_creation_cache:
                return self._token_creation_cache[token_address]
            
        timestamp = None
        
        # Try Birdeye API
        try:
            if getattr(self.liquidity_provider, "birdeye_client", None):
                birdeye_client = self.liquidity_provider.birdeye_client
                if birdeye_client:
                    creation_info = await birdeye_client.get_token_creation_info(token_address)
                    if creation_info:
                        # Handle both integer and string formats
                        ts = creation_info.get("blockUnixTime") or creation_info.get("txTime")
                        if ts:
                            timestamp = float(ts)
        except Exception as e:
            # Only log if verbose mode enabled
            if os.getenv("SCOUT_VERBOSE") == "true":
                print(f"[Analyzer] Birdeye creation fetch failed for {token_address[:8]}: {e}")
        
        # Fallback: Helius signatures — use the oldest known tx on this mint
        # as a lower-bound estimate of when the token began trading.
        if timestamp is None and self.helius_client.api_key:
            try:
                fallback_ts = await self.helius_client.get_token_first_tx_timestamp(token_address)
                if fallback_ts:
                    timestamp = float(fallback_ts)
                    self._parse_stats["token_creation_fallback_helix"] += 1
            except Exception as e:
                logger.debug(f"Fallback timestamp fetch failed for {token_address[:8]}: {e}")
        
        self._parse_stats["token_creation_fetched"] += 1
        if timestamp is not None:
            self._parse_stats["token_creation_success"] += 1
        
        # Cache the result (even if None) to avoid repeated API calls
        # Store in Redis for persistence across restarts
        if self._redis_client and self._redis_client.is_available():
            try:
                import json
                cache_key = f"token_creation:{token_address}"
                # Cache for 7 days (token creation time never changes)
                cache_value = json.dumps(timestamp) if timestamp is not None else "null"
                self._redis_client.set(cache_key, cache_value, ttl_seconds=7 * 24 * 3600)
            except Exception as e:
                logger.debug(f"Redis cache write failed for token creation: {e}")
        
        # Also cache in-memory for fast access
        async with self._token_creation_cache_lock:
            self._token_creation_cache[token_address] = timestamp
        return timestamp

    async def _is_token_safe(self, token_address: str) -> bool:
        """
        Check if a token is safe (not a honeypot, rug, or freeze risk).

        CRITICAL: Honeypot Filter.
        Results are cached with a TTL to avoid redundant RPC calls
        for frequently-traded tokens across multiple wallets.
        Uses asyncio.Lock() for thread-safe cache access in concurrent contexts.
        """
        if not token_address:
            return False

        _cache_key = token_address
        cache_ttl = 300  # 5-minute cache

        # Check cache with lock to prevent race conditions
        async with self._safety_cache_lock:
            if hasattr(self, '_token_safety_cache') and _cache_key in self._token_safety_cache:
                cached_result, cached_time = self._token_safety_cache[_cache_key]
                if time.time() - cached_time < cache_ttl:
                    return cached_result

        # Cache miss - perform expensive check outside the lock
        result = await self._is_token_safe_uncached(token_address)

        # Write to cache with lock
        async with self._safety_cache_lock:
            if not hasattr(self, '_token_safety_cache'):
                self._token_safety_cache: Dict[str, Tuple[bool, float]] = {}
            if not hasattr(self, '_safety_check_total'):
                self._safety_check_total = 0
                self._safety_check_failures = 0

            self._safety_check_total += 1
            if not result:
                self._safety_check_failures += 1

            self._token_safety_cache[_cache_key] = (result, time.time())
            return result

    async def _is_token_safe_uncached(self, token_address: str) -> bool:
        """
        Uncached token safety check (honeypot, rug, freeze risk).
        """
        if not token_address:
            return False

        # 1. Known Safe Tokens (USDC, USDT, SOL, etc) - always pass
        KNOWN_SAFE = [
            "So11111111111111111111111111111111111111112", # SOL
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", # USDC
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", # USDT
        ]
        if token_address in KNOWN_SAFE:
            return True
            
        # 2. Check Freeze Authority (The "Honeypot" Check)
        # Also check for Token-2022 program which has different layout
        try:
            if self.helius_client and self.helius_client.api_key:
                import aiohttp
                import base64
                
                url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "") or os.getenv("SOLANA_RPC_URL", "")
                if not url:
                    url = f"https://mainnet.helius-rpc.com/?api-key={self.helius_client.api_key}"
                payload = {
                    "jsonrpc": "2.0", 
                    "id": "scout-honeypot", 
                    "method": "getAccountInfo", 
                    "params": [token_address, {"encoding": "base64"}]
                }
                
                # Use async call (this method is called from async context)
                session = await self.helius_client._get_session()
                async with session.post(url, json=payload, timeout=aiohttp.ClientTimeout(total=3)) as resp:
                    if resp.status == 200:
                        data = await resp.json()
                        val = data.get("result", {}).get("value")
                        if val and val.get("data"):
                            raw = base64.b64decode(val["data"][0])
                            
                            TOKEN_2022_PROGRAM = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
                            account_owner = val.get("owner", "")

                            if account_owner == TOKEN_2022_PROGRAM:
                                # Token-2022 extension check: scan extension headers after
                                # the base mint layout (offset 165) for risky extension types.
                                # Extension type 1 = TransferFeeConfig (fee on every transfer)
                                # Extension type 2 = TransferFeeAmount (fee on every transfer)
                                # Extension type 4 = ConfidentialTransferMint (confidential transfer)
                                # Extension type 5 = ConfidentialTransferAccount (confidential transfer)
                                # Extension type 10 = InterestBearingConfig (value changes over time)
                                # Extension type 14 = TransferHook (arbitrary code on transfer)
                                RISKY_EXTENSIONS = {1, 2, 4, 5, 10, 14}
                                
                                # Check allowlist before rejecting for risky extensions
                                from scout.config import ScoutConfig
                                allowlist = ScoutConfig.get_token_2022_allowlist()
                                if token_address in allowlist:
                                    logger.info(f"Token-2022 token {token_address[:8]}... is in allowlist, skipping extension check")
                                elif len(raw) > 165:
                                    offset = 165
                                    while offset + 4 <= len(raw):
                                        ext_type = int.from_bytes(raw[offset:offset+2], 'little')
                                        ext_len = int.from_bytes(raw[offset+2:offset+4], 'little')
                                        if ext_type in RISKY_EXTENSIONS:
                                            import logging as _logging
                                            _logging.getLogger(__name__).warning(
                                                f"Token-2022 risky extension type={ext_type} found for {token_address}"
                                            )
                                            return False  # REJECT: TransferFee or TransferHook
                                        if ext_type == 0:  # End of extensions sentinel
                                            break
                                        offset += 4 + ext_len
                                # No risky extensions found; pass this check
                            else:
                                # Standard SPL Token layout
                                # Mint Layout: Freeze Option at offset 46 (u32)
                                # 0-3: MintAuthOption, 4-35: MintAuth, 36-43: Supply, 44: Decimals, 45: Init
                                # 46-49: FreezeAuthOption. If 1, Authority follows.
                                if len(raw) >= 50:
                                    freeze_opt = int.from_bytes(raw[46:50], 'little')
                                    if freeze_opt == 1:
                                        return False  # Has freeze authority -> REJECT

                                    # mint_authority == 0 means fixed supply (safe);
                                    # rely on RugCheck for comprehensive mint authority analysis
        except Exception as e:
            import traceback
            logger.warning(f"Token safety check failed for {token_address}: {e}\n{traceback.format_exc()}")
            # Check fail mode: "open" assumes safe on error, "closed" rejects
            fail_mode = os.getenv("SCOUT_SAFETY_FAIL_MODE", "closed").lower()
            if fail_mode == "open":
                logger.info(f"Safety fail-mode=open: assuming {token_address[:8]}... is safe")
                return True
            return False

        # Token passed basic safety checks
        return True

    def log_safety_health_summary(self) -> None:
        """Log a health warning if >20% of safety checks failed."""
        total = getattr(self, '_safety_check_total', 0)
        failures = getattr(self, '_safety_check_failures', 0)
        if total > 0:
            fail_rate = (failures / total) * 100
            if fail_rate > 20.0:
                logger.warning(
                    f"Token safety health: {failures}/{total} checks failed ({fail_rate:.0f}%) "
                    f"— RPC may be degraded"
                )

    async def _get_sol_price_usd(self) -> float:
        """
        Get current SOL price in USD.
        
        Returns:
            SOL price in USD, or 1.0 as fallback
        """
        async with self._sol_price_lock:
            if self._sol_price_usd is not None:
                return self._sol_price_usd

            try:
                sol_mint = "So11111111111111111111111111111111111111112"
                prices = await PortfolioTracker.fetch_bulk_prices([sol_mint])
                price = prices.get(sol_mint, 0.0)
                if price > 0:
                    self._sol_price_usd = price
                    if hasattr(self.liquidity_provider, 'cache_historical_sol_price'):
                        from datetime import timezone
                        self.liquidity_provider.cache_historical_sol_price(
                            datetime.now(timezone.utc), price
                        )
                    return price
            except Exception as e:
                logger.debug(f"Failed to fetch SOL price: {e}")

            return 1.0
    
    def determine_archetype(
        self, 
        metrics: WalletMetrics, 
        trades: List[HistoricalTrade]
    ) -> TraderArchetype:
        """
        Determine trader archetype based on trading behavior.
        
        Priority order:
        1. ARBITRAGE: Round-trip ratio >= threshold (bot behavior)
        2. INSIDER: Fresh wallet (created < 24h before trading)
        3. WHALE: Average trade size > 50 SOL
        4. SNIPER: Buys < 2 mins after launch on average
        5. SWING: Holds positions > 4 hours on average
        6. Default: SCALPER (many trades, small timeframe)
        
        Args:
            metrics: Wallet performance metrics
            trades: Historical trades
            
        Returns:
            TraderArchetype enum value
        """
        # Check for round-trip ratio (ARBITRAGE) if available in metrics
        # This should be computed before calling determine_archetype
        if hasattr(metrics, 'round_trip_ratio') and metrics.round_trip_ratio is not None:
            threshold = 0.60  # Default threshold
            if CONFIG_AVAILABLE and ScoutConfig:
                threshold = ScoutConfig.get_arb_round_trip_threshold_pct()
            
            if metrics.round_trip_ratio >= threshold:
                return TraderArchetype.ARBITRAGE
        
        # 2. INSIDER: Fresh wallet (created < 24h before trading)
        if metrics.is_fresh_wallet:
            return TraderArchetype.INSIDER
        
        # 3. WHALE: Average trade size > 50 SOL
        if metrics.avg_trade_size_sol and metrics.avg_trade_size_sol > Decimal(50):
            return TraderArchetype.WHALE
        
        # 4. SNIPER: Buys < 2 mins after launch on average
        if metrics.avg_entry_delay_seconds is not None:
            if metrics.avg_entry_delay_seconds < 120:  # < 2 minutes
                return TraderArchetype.SNIPER
        
        # 5. SWING: Holds positions > 4 hours on average
        avg_hold_time = self._calculate_avg_hold_time(trades)
        if avg_hold_time and avg_hold_time > 14400:  # > 4 hours (14400 seconds)
            return TraderArchetype.SWING
        
        # 6. Default: SCALPER (many trades, small timeframe)
        return TraderArchetype.SCALPER
    
    def _calculate_avg_hold_time(self, trades: List[HistoricalTrade]) -> Optional[float]:
        """
        Calculate average hold time in seconds.
        
        Args:
            trades: List of historical trades (sorted by timestamp)
            
        Returns:
            Average hold time in seconds, or None if insufficient data
        """
        if not trades:
            return None
        
        # Sort trades by timestamp
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        # Track open positions: token_address -> (buy_time, buy_amount)
        open_positions: Dict[str, List[tuple]] = {}  # token -> [(buy_time, buy_amount), ...]
        hold_times = []
        
        for t in sorted_trades:
            token_addr = t.token_address
            timestamp = t.timestamp.timestamp()
            
            if t.action == TradeAction.BUY:
                # Add to open positions
                buy_amount = t.token_amount or t.amount_sol
                if token_addr not in open_positions:
                    open_positions[token_addr] = []
                open_positions[token_addr].append((timestamp, buy_amount))
                
            elif t.action == TradeAction.SELL:
                # Match with oldest buy (FIFO)
                if token_addr in open_positions and open_positions[token_addr]:
                    sell_amount = t.token_amount or t.amount_sol
                    remaining_sell = sell_amount
                    
                    while remaining_sell > 0 and open_positions[token_addr]:
                        buy_time, buy_amount = open_positions[token_addr][0]
                        
                        # Calculate how much of this buy is being sold
                        sold_from_buy = min(remaining_sell, buy_amount)
                        hold_time = timestamp - buy_time
                        
                        if hold_time > 0:  # Sanity check
                            hold_times.append(hold_time)
                        
                        # Update positions
                        if sold_from_buy >= buy_amount:
                            # Fully sold this buy
                            open_positions[token_addr].pop(0)
                            remaining_sell -= buy_amount
                        else:
                            # Partially sold
                            open_positions[token_addr][0] = (buy_time, buy_amount - sold_from_buy)
                            remaining_sell = 0
        
        if not hold_times:
            return None
        
        return sum(hold_times) / len(hold_times)
    
    def _detect_round_trip_ratio_from_transactions(
        self, 
        transactions: List[Dict[str, Any]],
        wallet_address: str
    ) -> float:
        """
        Detect round-trip ratio from raw Helius transactions.
        
        A round-trip is defined as a transaction where the same token is both
        bought and sold in the same transaction (typical of arbitrage bots).
        
        Args:
            transactions: List of raw Helius transaction objects
            wallet_address: Wallet address to analyze
            
        Returns:
            Round-trip ratio as a float (0.0 to 1.0)
        """
        if not transactions or len(transactions) < 3:
            return 0.0
        
        round_trip_count = 0
        analyzed_count = 0
        
        for tx in transactions:
            # Only analyze SWAP transactions
            if tx.get("type") != "SWAP":
                continue
            
            # Extract token deltas from the transaction
            token_deltas: Dict[str, float] = {}
            
            # Process tokenTransfers
            for tt in tx.get("tokenTransfers", []):
                mint = tt.get("mint", "")
                if not mint:
                    continue
                
                # Handle different tokenAmount formats
                amount = tt.get("tokenAmount", 0)
                if isinstance(amount, str):
                    try:
                        amount = float(amount)
                    except ValueError:
                        continue
                elif isinstance(amount, (int, float)):
                    amount = float(amount)
                else:
                    continue
                
                # Track wallet-relative deltas
                if tt.get("fromUserAccount") == wallet_address:
                    token_deltas[mint] = token_deltas.get(mint, 0.0) - amount
                if tt.get("toUserAccount") == wallet_address:
                    token_deltas[mint] = token_deltas.get(mint, 0.0) + amount
            
            # Check if any token has both positive and negative deltas
            # (i.e., both bought and sold in the same transaction)
            has_round_trip = False
            for mint, delta in token_deltas.items():
                # Skip SOL/wSOL (it's the quote asset)
                if mint == "So11111111111111111111111111111111111111112":
                    continue
                
                # If delta is close to zero, it means we both bought and sold this token
                # Use a small threshold to account for rounding errors and slippage
                if abs(delta) < 0.01:  # Allow for small amounts from fees/slippage
                    has_round_trip = True
                    print(f"  [DEBUG] Round-trip detected for token {mint[:8]}... (delta={delta})")
                    break
            
            if has_round_trip:
                round_trip_count += 1
            analyzed_count += 1
        
        print(f"  [DEBUG] Round-trip detection: {round_trip_count}/{analyzed_count} swaps")
        
        if analyzed_count == 0:
            return 0.0
        
        return round_trip_count / analyzed_count
    
    async def _detect_insider_patterns(self, address: str, trades: List[HistoricalTrade]) -> Dict[str, Any]:
        """
        Detect insider behavior based on wallet age, funding, and token creation proximity.

        Fresh wallets (created <24h before first trade) are typically:
        - Burner wallets for insider trading
        - Bot wallets for sniping
        - Ephemeral addresses to hide identity

        Also checks token_creation_awareness: if >60% of BUYs happen within 5 min
        of token creation AND the wallet enters quickly, classify as insider.

        Returns:
            Dict with insider metrics
        """
        is_fresh_wallet = False

        if not trades:
            return {"is_fresh_wallet": False, "suspicion_score": 0.0, "token_creation_awareness_ratio": 0.0}

        # Get first trade timestamp
        first_trade_time = min(t.timestamp for t in trades)

        now = time.time()

        # Try to get wallet creation time (first transaction ever) for the authoritative check
        wallet_creation_time = await self._get_wallet_creation_time_cached(address)

        if wallet_creation_time:
            # Authoritative: wallet is fresh only if the wallet itself was created recently
            wallet_age_days = (now - wallet_creation_time) / 86400
            if wallet_age_days < 7:
                is_fresh_wallet = True

            # If wallet was created <24h before its first swap trade, it's suspicious
            # regardless of wallet age (e.g., sniper wallet spun up for one token)
            hours_to_first_trade = (first_trade_time.timestamp() - wallet_creation_time) / 3600
            if hours_to_first_trade < 24:
                is_fresh_wallet = True
        else:
            # Fallback: no creation time available — use first SWAP trade in our 30-day window
            # as a proxy. This can produce false positives for old wallets that traded
            # different tokens last month, so only flag if first trade is very recent (<3 days).
            wallet_age_seconds = now - first_trade_time.timestamp()
            wallet_age_days = wallet_age_seconds / 86400
            if wallet_age_days < 3:
                is_fresh_wallet = True

        # Token creation awareness: check how often the wallet buys within 5 min of
        # token creation. A wallet that consistently enters right after launch is
        # either a sniper or insider, regardless of wallet age.
        # Uses token creation times already cached by the caller's pre-fetch.
        buy_trades = [t for t in trades if t.action == TradeAction.BUY]
        token_creation_awareness_ratio = 0.0
        if buy_trades:
            buys_near_creation = 0
            for t in buy_trades:
                creation_ts = self._token_creation_cache.get(t.token_address)
                if creation_ts:
                    delay_seconds = t.timestamp.timestamp() - creation_ts
                    if 0 <= delay_seconds <= 300:  # Within 5 minutes
                        buys_near_creation += 1
            if len(buy_trades) > 0:
                token_creation_awareness_ratio = buys_near_creation / len(buy_trades)

        # If >60% of BUYs happen within 5 min of token creation AND the wallet
        # enters quickly overall, classify as insider regardless of wallet age.
        avg_entry = getattr(self, '_cached_avg_entry_delay', None)
        if token_creation_awareness_ratio > 0.6:
            if avg_entry is not None and avg_entry < 120:
                is_fresh_wallet = True

        return {
            "is_fresh_wallet": is_fresh_wallet,
            "suspicion_score": 100.0 if is_fresh_wallet else 0.0,
            "token_creation_awareness_ratio": token_creation_awareness_ratio,
        }

    async def _get_wallet_creation_time_cached(self, address: str) -> Optional[float]:
        """
        Get wallet creation time (first transaction) with caching.

        Returns:
            Unix timestamp of first transaction, or None
        """
        async with self._wallet_age_cache_lock:
            if address in self._wallet_age_cache:
                return self._wallet_age_cache[address]

        creation_time = None
        if self.helius_client and hasattr(self.helius_client, 'get_wallet_first_transaction'):
            try:
                creation_time = await self.helius_client.get_wallet_first_transaction(address)
            except Exception as e:
                logger.debug(f"Wallet first transaction fetch failed for {address[:8]}: {e}")

        async with self._wallet_age_cache_lock:
            self._wallet_age_cache[address] = creation_time
        return creation_time

    async def _calculate_metrics_from_trades(self, address: str, trades: List[HistoricalTrade], dex_diversity_score: Optional[int] = None, uses_limit_orders: bool = False, uses_mev_protection: bool = False, is_unproven_from_parse: bool = False, parse_rate: Optional[float] = None, mev_risk_score: Optional[float] = None, is_tg_bot_user: bool = False, transactions: Optional[List[Dict[str, Any]]] = None) -> Optional[WalletMetrics]:
        """Calculate wallet metrics from historical trades."""
        if not trades:
            return None

        print(f"  [{address[:8]}] Checking token safety with RugCheck...")
        # Filter out unsafe tokens using RugCheck if enabled
        if self.rugcheck_client:
            # Dedupe tokens to avoid redundant API calls
            unique_tokens = {t.token_address for t in trades}
            print(f"  [{address[:8]}] Checking {len(unique_tokens)} unique tokens (from {len(trades)} trades)...")
            
            safe_tokens = set()
            risky_tokens = []
            
            # Parallel RugCheck with semaphore for rate limiting
            semaphore = asyncio.Semaphore(10)
            
            async def check_token(token_addr: str) -> Tuple[str, bool]:
                async with semaphore:
                    try:
                        is_safe = await asyncio.wait_for(
                            self.rugcheck_client.is_token_safe(token_addr),
                            timeout=5.0
                        )
                        return (token_addr, is_safe)
                    except asyncio.TimeoutError:
                        print(f"  [{address[:8]}] RugCheck timeout for token {token_addr[:8]}, marking as risky")
                        return (token_addr, False)
                    except Exception as e:
                        print(f"  [{address[:8]}] RugCheck error for token {token_addr[:8]}: {e}, marking as risky")
                        return (token_addr, False)
            
            # Run all token checks in parallel
            token_list = list(unique_tokens)
            results = await asyncio.gather(*[check_token(t) for t in token_list])
            
            # Process results and check for circuit breaker condition
            for token_addr, is_safe in results:
                if is_safe:
                    safe_tokens.add(token_addr)
                else:
                    risky_tokens.append(token_addr)
            
            # Proactive circuit breaker: if >50% risky after checking tokens, abort
            risky_ratio = len(risky_tokens) / max(1, len(token_list)) if risky_tokens else 0.0
            if risky_tokens:
                fail_mode = ScoutConfig.get_rugcheck_fail_mode() if CONFIG_AVAILABLE else "closed"
                if risky_ratio > 0.5:
                    logger.warning(
                        "RugCheck degraded: %.0f%% tokens flagged risky (%d/%d).",
                        risky_ratio * 100, len(risky_tokens), len(token_list),
                    )
                    if fail_mode == "open":
                        logger.warning(
                            "RugCheck fail mode is OPEN — assuming safe to prevent roster drain (escape hatch active)"
                        )
                        print(f"  [{address[:8]}] RugCheck circuit breaker triggered ({risky_ratio*100:.0f}% risky) — keeping all trades (escape hatch)")
                    else:
                        print(f"  [{address[:8]}] RugCheck circuit breaker triggered ({risky_ratio*100:.0f}% risky) — dropping wallet (capital-protective)")
                        return None
                else:
                    print(f"  [{address[:8]}] Filtered {len(risky_tokens)} risky tokens ({risky_ratio*100:.0f}% of unique tokens)")
                    # Filter trades to only those with safe tokens
                    trades = [t for t in trades if t.token_address in safe_tokens]
                    if not trades:
                        print(f"  [{address[:8]}] All trades filtered as risky")
                        return None
            else:
                print(f"  [{address[:8]}] All {len(safe_tokens)} tokens passed RugCheck")
        else:
            print(f"  [{address[:8]}] RugCheck disabled, using all trades")
        
        print(f"  [{address[:8]}] Sorting {len(trades)} trades...")
        # Sort trades: Primary = Timestamp, Secondary = Action (BUY before SELL to allow intraday scalps)
        # Assuming TradeAction.BUY is defined such that it sorts appropriately, or use custom key
        sorted_trades = sorted(trades, key=lambda t: (
            t.timestamp, 
            0 if t.action == TradeAction.BUY else 1
        ))

        print(f"  [{address[:8]}] Enriching trades with PnL...")
        # Enrich AFTER sorting to ensure correct cost basis calculation
        try:
            self._enrich_trades_with_realized_pnl(sorted_trades)
            print(f"  [{address[:8]}] Trades enriched successfully")
        except Exception as e:
            print(f"  [{address[:8]}] ERROR enriching trades: {e}")
            import traceback
            traceback.print_exc()
            return None
        
        # ... rest of the function ...
        
        print(f"  [{address[:8]}] Computing time windows...")
        # Calculate time windows
        now = utcnow()
        cutoff_7d = now - timedelta(days=7)
        cutoff_30d = now - timedelta(days=30)
        cutoff_90d = now - timedelta(days=90)
        
        trades_7d = [t for t in sorted_trades if t.timestamp >= cutoff_7d]
        trades_30d = [t for t in sorted_trades if t.timestamp >= cutoff_30d]
        trades_90d = [t for t in sorted_trades if t.timestamp >= cutoff_90d]
        print(f"  [{address[:8]}] Trades: 7d={len(trades_7d)}, 30d={len(trades_30d)}, 90d={len(trades_90d)}")

        # IMPORTANT:
        # `trade_count_30d` is intentionally defined as the number of *realized closes*,
        # i.e. SELL trades with a computed `pnl_sol`. This makes significance tests and
        # win/loss metrics comparable and prevents “lots of buys, few sells” wallets
        # from looking statistically robust.
        close_trades_30d = [
            t for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol is not None
        ]
        print(f"  [{address[:8]}] Close trades (30d): {len(close_trades_30d)}")
        
        print(f"  [{address[:8]}] Calculating ROI...")
        # Calculate ROI from actual price changes
        roi_7d = self._calculate_roi_from_trades(trades_7d, days=7)
        roi_30d = self._calculate_roi_from_trades(trades_30d, days=30)
        roi_90d = self._calculate_roi_from_trades(trades_90d, days=90) if len(trades_90d) > len(trades_30d) else None
        print(f"  [{address[:8]}] ROI: 7d={roi_7d:.1f}%, 30d={roi_30d:.1f}%"
              + (f", 90d={roi_90d:.1f}%" if roi_90d is not None else ""))
        
        print(f"  [{address[:8]}] Calculating win rate...")
        # Calculate win rate from actual PnL data
        try:
            win_rate = self._calculate_win_rate_from_trades(trades_30d)
            print(f"  [{address[:8]}] Win rate: {win_rate:.1f}%")
        except Exception as e:
            print(f"  [{address[:8]}] ERROR calculating win rate: {e}")
            return None
        
        print(f"  [{address[:8]}] Calculating drawdown...")
        # Calculate drawdown
        try:
            max_drawdown = self._calculate_drawdown_from_trades(trades_30d)
            print(f"  [{address[:8]}] Drawdown: {max_drawdown:.1f}%")
        except Exception as e:
            print(f"  [{address[:8]}] ERROR calculating drawdown: {e}")
            return None
        
        # Calculate average trade size (use Decimal for precision)
        avg_trade_size = safe_decimal_divide(
            sum(t.amount_sol for t in trades_30d),
            Decimal(str(len(trades_30d)))
        ) if trades_30d else Decimal('0')
        
        # Get last trade timestamp
        last_trade_at = sorted_trades[-1].timestamp.isoformat() if sorted_trades else None
        
        # Calculate win streak consistency (simplified)
        win_streak_consistency = self._calculate_win_streak_consistency(trades_30d)
        
        # 1. Calculate Profit Factor (use Decimal internally, convert to float at boundary)
        # Use trades_30d (not all-time trades) so recency weighting is consistent with ROI/win-rate
        gross_profit = sum(t.pnl_sol for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol and t.pnl_sol > Decimal('0'))
        gross_loss = abs(sum(t.pnl_sol for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol and t.pnl_sol < Decimal('0')))
        
        profit_factor = 0.0
        win_count = sum(1 for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol and t.pnl_sol > Decimal('0'))
        profit_factor = self._compute_base_profit_factor(gross_profit, gross_loss, win_count)

        # Bag-holder penalty on profit_factor (Phase 2.4)
        # Reconstruct positions from all trades and penalize PF for bags held > 30 days.
        # Mirrors compute_wallet_trade_stats logic to ensure bag-aware PF reaches WQS.
        import time as _time_module
        bag_positions: dict = {}
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                pos = bag_positions.setdefault(t.token_address, {"qty": Decimal('0'), "cost": Decimal('0')})
                qty = t.token_amount
                if qty is None or qty == Decimal('0'):
                    if t.price_at_trade and t.price_at_trade > Decimal('0'):
                        qty = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        qty = Decimal('0')
                if qty > Decimal('0'):
                    pos["qty"] += qty
                    pos["cost"] += t.amount_sol
            elif t.action == TradeAction.SELL:
                pos = bag_positions.get(t.token_address)
                if pos and pos["qty"] > Decimal('0'):
                    qty = t.token_amount
                    if qty is None or qty == Decimal('0'):
                        if t.price_at_trade and t.price_at_trade > Decimal('0'):
                            qty = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                        else:
                            qty = Decimal('0')
                    if qty > Decimal('0') and pos["qty"] > Decimal('0'):
                        fraction = min(Decimal('1.0'), safe_decimal_divide(qty, pos["qty"]))
                        pos["qty"] -= qty
                        pos["cost"] -= (pos["cost"] * fraction)

        _now_ts = Decimal(str(int(_time_module.time())))
        _max_bag_age = Decimal('2592000')
        bag_count = 0
        for token, pos in bag_positions.items():
            if pos["qty"] > Decimal('0'):
                last_buy = None
                for t in sorted_trades:
                    if t.token_address == token and t.action == TradeAction.BUY:
                        last_buy = t.timestamp
                if last_buy:
                    bag_age = _now_ts - Decimal(str(int(last_buy.timestamp())))
                    if bag_age > _max_bag_age:
                        bag_count += 1

        if bag_count > 0:
            bag_penalty = min(Decimal('0.5'), Decimal(bag_count) * Decimal('0.1'))
            profit_factor = float(Decimal(str(profit_factor)) * (Decimal('1.0') - bag_penalty))

        # 2. Calculate Average Entry Delay (Sniper Check)
        avg_entry_delay = None
        entry_delays = []
        buy_trades = [t for t in trades if t.action == TradeAction.BUY]
        
        # Take the 5 most-recently-bought unique tokens for the sniper check.
        # Using a plain set gives non-deterministic ordering — always use recency order
        # so we sample the wallet's latest behaviour, not an arbitrary 5 tokens.
        seen_tokens: set = set()
        recent_unique_tokens = []
        for t in sorted(buy_trades, key=lambda x: x.timestamp, reverse=True):
            if t.token_address not in seen_tokens:
                seen_tokens.add(t.token_address)
                recent_unique_tokens.append(t.token_address)
                if len(recent_unique_tokens) == 5:
                    break
        unique_tokens = recent_unique_tokens

        # Also prefetch ALL token creation times for insider detection (B4).
        # The sniper check uses only the 5 most recent; insider detection uses all.
        all_token_addresses = list(set(t.token_address for t in buy_trades))
        
        # Pre-fetch creation times (this will cache them) — sniper tokens first, then all remaining
        print(f"  [{address[:8]}] Fetching token creation times for {len(all_token_addresses)} tokens...")
        tasks = [self._fetch_token_creation_time(token) for token in all_token_addresses]
        await asyncio.gather(*tasks, return_exceptions=True)
        print(f"  [{address[:8]}] Token creation times fetched")
            
        for token in unique_tokens:
            # _token_creation_cache is a dict, not async
            creation_ts = self._token_creation_cache.get(token)
            if creation_ts:
                # Find the FIRST buy of this token by this wallet
                first_buy = min([t.timestamp.timestamp() for t in buy_trades if t.token_address == token])
                
                # Ensure delay is non-negative
                delay = max(0.0, first_buy - creation_ts)
                entry_delays.append(delay)
        
        if entry_delays:
            avg_entry_delay = sum(entry_delays) / len(entry_delays)

        # Store on self so _detect_insider_patterns can access it for B4 token-awareness check
        self._cached_avg_entry_delay = avg_entry_delay
        
        print(f"  [{address[:8]}] Detecting insider patterns...")
        # 3. Detect Insider Patterns (Fresh Wallet Check)
        try:
            insider_metrics = await self._detect_insider_patterns(address, trades)
            is_fresh_wallet = insider_metrics.get("is_fresh_wallet", False)
            print(f"  [{address[:8]}] Insider detection complete (fresh={is_fresh_wallet})")
        except Exception as e:
            print(f"  [{address[:8]}] ERROR in insider detection: {e}")
            is_fresh_wallet = False

        # Calculate replay data gap ratio from FIFO replay
        _, _, _, _, replay_data_gap_ratio = self._replay_positions(trades)
        if replay_data_gap_ratio > 0:
            print(f"  [{address[:8]}] FIFO replay data gap ratio: {replay_data_gap_ratio:.2%}")

        # 4. Smart Money Detection (DEX diversity, limit orders, MEV protection)
        # All three values are computed from raw Helius transactions upstream and passed in.
        # uses_limit_orders / uses_mev_protection default False for callers that don't supply them.
        
        print(f"  [{address[:8]}] Calculating unrealized PnL...")
        # 5. Calculate Unrealized PnL (Bag Holder Detection)
        total_unrealized_loss_sol = None
        total_realized_profit_sol = None
        total_unrealized_gain_sol = None
        
        try:
            # Calculate realized profit from SELL trades (use Decimal)
            realized_pnls = [t.pnl_sol for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol is not None]
            total_realized_profit_sol = sum((pnl for pnl in realized_pnls if pnl > Decimal('0')), Decimal('0'))
            
            # Get unique token addresses from current holdings
            buy_trades = [t for t in sorted_trades if t.action == TradeAction.BUY]
            sell_trades = [t for t in sorted_trades if t.action == TradeAction.SELL]
            
            # Track sell amounts per token (use Decimal)
            sell_amounts = {}
            for t in sell_trades:
                token_addr = t.token_address
                token_amount = t.token_amount or Decimal('0')
                sell_amounts[token_addr] = sell_amounts.get(token_addr, Decimal('0')) + token_amount
            
            # Find tokens that might have remaining holdings
            potential_holdings = []
            buy_amounts = {}
            for t in buy_trades:
                token_addr = t.token_address
                token_amount = t.token_amount or Decimal('0')
                buy_amounts[token_addr] = buy_amounts.get(token_addr, Decimal('0')) + token_amount
                
                # If buy amount > sell amount, there might be holdings
                if buy_amounts[token_addr] > sell_amounts.get(token_addr, Decimal('0')):
                    potential_holdings.append(token_addr)
            
            # Fetch current prices for tokens with potential holdings
            if potential_holdings:
                # Get SOL price for conversion
                sol_price = await self._get_sol_price_usd()
                
                # Fetch prices in bulk
                current_prices = await PortfolioTracker.fetch_bulk_prices(potential_holdings)
                
                # Calculate unrealized PnL (losses)
                total_unrealized_loss_sol = PortfolioTracker.calculate_unrealized_pnl(
                    sorted_trades,
                    current_prices,
                    sol_price
                )
                print(f"  [{address[:8]}] Unrealized PnL calculated: {total_unrealized_loss_sol}")
                
                # Calculate paper gains (unrealized profits)
                total_unrealized_gain_sol = PortfolioTracker.calculate_paper_gains(
                    sorted_trades,
                    current_prices,
                    sol_price
                )
        except Exception as e:
            print(f"  [{address[:8]}] ERROR calculating unrealized PnL: {e}")
            logger.warning(f"Failed to calculate unrealized PnL for {address}: {e}")
            total_unrealized_loss_sol = None
        
        print(f"  [{address[:8]}] Computing Sortino ratio...")
        # Calculate Sortino ratio: excess return / downside deviation
        sortino_ratio = None
        try:
            if close_trades_30d:
                # Compute per-trade return fractions (pnl / cost_basis) so numerator and
                # denominator are in the same units (dimensionless fraction of capital invested).
                # Using raw SOL pnl / raw SOL pnl for downside std would give units of "SOL",
                # which makes the ratio dependent on position size rather than trade quality.
                sell_trades = [
                    t for t in trades_30d
                    if t.action == TradeAction.SELL
                    and t.pnl_sol is not None
                    and t.sol_amount is not None
                    and t.sol_amount > 0
                ]
                if sell_trades:
                    # per-trade return fraction = pnl / cost_basis
                    # Infer cost basis: cost_basis = sol_amount - pnl_sol (since pnl = proceeds - cost)
                    # Fall back to sol_amount if pnl is None or cost_basis would be zero
                    def _infer_return(t) -> float:
                        if t.pnl_sol is None:
                            return 0.0
                        cost_basis = t.sol_amount - t.pnl_sol
                        if cost_basis <= 0:
                            return 0.0
                        return float(t.pnl_sol / cost_basis)

                    trade_returns = [_infer_return(t) for t in sell_trades]
                    avg_return = sum(trade_returns) / len(trade_returns)
                    downside_returns = [r for r in trade_returns if r < 0]
                    if downside_returns:
                        downside_variance = sum(r**2 for r in downside_returns) / len(downside_returns)
                        downside_deviation = downside_variance ** 0.5
                        if downside_deviation > 0:
                            sortino_ratio = avg_return / downside_deviation
        except Exception as e:
            print(f"  [{address[:8]}] Warning: Could not calculate Sortino ratio: {e}")

        print(f"  [{address[:8]}] Creating WalletMetrics object...")
        # D5: Check if wallet is correlated with known scam addresses
        correlated_with_scam = False
        if os.getenv("SCOUT_ENABLE_SCAM_CHECK", "true").lower() == "true":
            try:
                if is_known_scam_address(address):
                    correlated_with_scam = True
                else:
                    funder = await self.helius_client.get_wallet_funder(address)
                    correlated_with_scam = not await check_wallet_correlation(
                        address, funder=funder
                    )
            except Exception as e:
                logger.warning(f"Scam check failed for {address}: {e}", exc_info=True)

        # Phase 2c: Token category concentration
        token_symbols = set(t.token_symbol for t in trades if t.token_symbol and t.token_symbol != "UNKNOWN")
        token_categories = set()
        for sym in token_symbols:
            cat = self._classify_token_category(sym)
            if cat:
                token_categories.add(cat)
        unique_token_categories = len(token_categories) if token_categories else None

        # Phase 2d: Pump.fun bonding-curve token concentration
        # Tokens with mint address ending in "pump" are pump.fun bonding-curve tokens
        # with $0 DEX liquidity — copy-trading them is a guaranteed loss from Jito tips.
        pumpfun_count = sum(1 for t in trades if t.token_address and t.token_address.endswith("pump"))
        pumpfun_trade_ratio = (pumpfun_count / len(trades)) if trades else None
        if pumpfun_trade_ratio is not None:
            print(f"  [{address[:8]}] Pump.fun trade ratio: {pumpfun_trade_ratio:.2%} ({pumpfun_count}/{len(trades)})")

        # Round-trip detection for arbitrage classification
        print(f"  [{address[:8]}] Detecting round-trip ratio...")
        round_trip_ratio = None
        try:
            min_trades_for_arb = 10
            if CONFIG_AVAILABLE and ScoutConfig:
                min_trades_for_arb = ScoutConfig.get_arb_min_trades_for_detection()
            
            # Only compute round-trip ratio if we have enough raw transactions
            if transactions and len(transactions) >= min_trades_for_arb:
                round_trip_ratio = self._detect_round_trip_ratio_from_transactions(transactions, address)
                print(f"  [{address[:8]}] Round-trip ratio: {round_trip_ratio:.2%}")
            else:
                _tx_count = len(transactions) if transactions else 0
                print(f"  [{address[:8]}] Skipping round-trip detection (insufficient transactions: {_tx_count} < {min_trades_for_arb})")
        except Exception as e:
            print(f"  [{address[:8]}] Warning: Could not detect round-trip ratio: {e}")

        # Determine trader archetype
        archetype = None
        try:
            temp_metrics = WalletMetrics(
                address=address,
                roi_7d=roi_7d,
                roi_30d=roi_30d,
                trade_count_30d=len(close_trades_30d),
                avg_trade_size_sol=avg_trade_size if avg_trade_size else None,
                avg_entry_delay_seconds=avg_entry_delay,
                is_fresh_wallet=is_fresh_wallet,
                round_trip_ratio=round_trip_ratio,
            )
            archetype_result = self.determine_archetype(temp_metrics, trades)
            archetype = archetype_result.value if hasattr(archetype_result, 'value') else str(archetype_result)
            print(f"  [{address[:8]}] Archetype: {archetype}")
        except Exception as e:
            print(f"  [{address[:8]}] Warning: Could not determine archetype: {e}")

        # Calculate trajectory
        trajectory = None
        try:
            from .wqs import _interpret_trajectory
            trajectory = _interpret_trajectory(roi_7d, roi_30d)
            print(f"  [{address[:8]}] Trajectory: {trajectory}")
        except Exception as e:
            print(f"  [{address[:8]}] Warning: Could not calculate trajectory: {e}")

        return WalletMetrics(
            address=address,
            roi_7d=roi_7d,
            roi_30d=roi_30d,
            roi_90d=roi_90d,
            trade_count_30d=len(close_trades_30d),
            win_rate=win_rate,
            max_drawdown_30d=max_drawdown,
            avg_trade_size_sol=avg_trade_size if avg_trade_size else None,
            last_trade_at=last_trade_at,
            win_streak_consistency=win_streak_consistency,
            avg_entry_delay_seconds=avg_entry_delay,
            profit_factor=profit_factor,
            is_fresh_wallet=is_fresh_wallet,
            is_unproven=(profit_factor is None or is_unproven_from_parse),
            sortino_ratio=sortino_ratio,
            parse_rate=parse_rate,
            total_unrealized_loss_sol=total_unrealized_loss_sol,
            total_realized_profit_sol=total_realized_profit_sol,
            total_unrealized_gain_sol=total_unrealized_gain_sol,
            dex_diversity_score=dex_diversity_score,
            uses_limit_orders=uses_limit_orders,
            uses_mev_protection=uses_mev_protection,
            correlated_with_scam=correlated_with_scam,
            unique_token_categories=unique_token_categories,
            mev_risk_score=mev_risk_score,
            archetype=archetype,
            trajectory=trajectory,
            replay_data_gap_ratio=replay_data_gap_ratio,
            is_tg_bot_user=is_tg_bot_user,
            round_trip_ratio=round_trip_ratio,
            pumpfun_trade_ratio=pumpfun_trade_ratio,
        )

    def compute_wallet_trade_stats(self, trades: List[HistoricalTrade]) -> Dict[str, Optional[Any]]:
        """
        Compute additional wallet stats from realized PnL (SOL) for persistence.
        
        Uses Decimal internally for all financial calculations to avoid floating-point errors.
        Monetary values are returned as Decimal; ratios remain float.

        Returns:
          - avg_win_sol (float)
          - avg_loss_sol (float)
          - profit_factor (float, sum_wins / sum_losses)
          - realized_pnl_30d_sol (Decimal, sum of realized pnl over SELL trades)
        """
        if not trades:
            return {
                "avg_win_sol": None,
                "avg_loss_sol": None,
                "profit_factor": None,
                "realized_pnl_30d_sol": None,
            }

        pnls = [t.pnl_sol for t in trades if t.action == TradeAction.SELL and t.pnl_sol is not None]
        if not pnls:
            return {
                "avg_win_sol": None,
                "avg_loss_sol": None,
                "profit_factor": None,
                "realized_pnl_30d_sol": Decimal(0),
            }

        wins = [p for p in pnls if p > Decimal('0')]
        losses = [abs(p) for p in pnls if p < Decimal('0')]
        sum_wins = sum(wins) if wins else Decimal('0')
        sum_losses = sum(losses) if losses else Decimal('0')
        
        # ---------------------------------------------------------
        # NEW: "Open Position" Trap Check
        # Scan for bags held (Rug Check). If value < 10% of cost, count as loss.
        # ---------------------------------------------------------
        # Quick position reconstruction using Decimal
        positions: Dict[str, Dict[str, Decimal]] = {} # token -> {qty, cost}
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                pos = positions.setdefault(t.token_address, {"qty": Decimal('0'), "cost": Decimal('0')})
                qty = t.token_amount
                if qty is None or qty == Decimal('0'):
                    if t.price_at_trade and t.price_at_trade > Decimal('0'):
                        qty = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                    else:
                        qty = Decimal('0')
                if qty > Decimal('0'):
                    pos["qty"] += qty
                    pos["cost"] += t.amount_sol
            elif t.action == TradeAction.SELL:
                pos = positions.get(t.token_address)
                if pos and pos["qty"] > Decimal('0'):
                    qty = t.token_amount
                    if qty is None or qty == Decimal('0'):
                        if t.price_at_trade and t.price_at_trade > Decimal('0'):
                            qty = safe_decimal_divide(t.amount_sol, t.price_at_trade)
                        else:
                            qty = Decimal('0')
                    # Proportional cost reduction
                    fraction = min(Decimal('1.0'), safe_decimal_divide(qty, pos["qty"]))
                    pos["qty"] -= qty
                    pos["cost"] -= (pos["cost"] * fraction)
        
        # Check remaining bags (open positions)
        # Apply penalty for positions held > 30 days without exit
        # Full unrealized PnL requires price fetches (handled in calculate_unrealized_pnl async)
        now = Decimal(str(time.time()))
        bag_count = 0
        max_bag_age_seconds = Decimal('2592000')  # 30 days
        for token, pos in positions.items():
            if pos["qty"] > Decimal('0'):
                # This token is held without exit
                last_buy = None
                for t in sorted_trades:
                    if t.token_address == token and t.action == TradeAction.BUY:
                        last_buy = t.timestamp

                if last_buy:
                    # last_buy is a datetime object — use .timestamp() to get Unix epoch
                    bag_age = now - Decimal(str(int(last_buy.timestamp())))
                    if bag_age > max_bag_age_seconds:
                        # Bag held > 30 days - apply penalty
                        bag_count += 1

        avg_win = decimal_to_float(safe_decimal_divide(sum_wins, Decimal(str(len(wins))))) if wins else None
        avg_loss = decimal_to_float(safe_decimal_divide(sum_losses, Decimal(str(len(losses))))) if losses else None

        # Profit Factor Calculation (Robust + Rug Aware)
        profit_factor = self._compute_base_profit_factor(sum_wins, sum_losses, len(wins))

        # Reduce profit_factor by 10% per held bag (capped at 50% reduction)
        if bag_count > 0:
            bag_penalty = min(Decimal('0.5'), Decimal(bag_count) * Decimal('0.1'))
            profit_factor = decimal_to_float(
                (Decimal(str(profit_factor)) * (Decimal('1.0') - bag_penalty))
            )

        # Ensure pnls sum is computed with Decimal
        total_realized_pnl = sum(pnls, Decimal('0')) if pnls else Decimal('0')
        
        return {
            "avg_win_sol": avg_win,
            "avg_loss_sol": avg_loss,
            "profit_factor": profit_factor,
            "realized_pnl_30d_sol": total_realized_pnl, # monetary value stays Decimal
        }
    
    @staticmethod
    def _compute_base_profit_factor(
        gross_profit: Decimal,
        gross_loss: Decimal,
        win_count: int,
    ) -> float:
        """
        Shared profit factor computation used by both get_wallet_metrics
        and compute_wallet_trade_stats.

        Returns capped profit factor for zero-loss wallets to prevent inflation.
        """
        if gross_loss == Decimal('0'):
            if gross_profit > Decimal('0'):
                # Zero losses have undefined/infinite PF, but we cap at 100.0
                # to prevent extreme score inflation while still rewarding perfect performance
                capped = min(Decimal(str(win_count)) * Decimal('2.0'), Decimal('100.0'))
                return decimal_to_float(capped)
            return 0.0
        return decimal_to_float(safe_decimal_divide(gross_profit, gross_loss))

    def _calculate_roi_from_trades(
        self,
        trades: List[HistoricalTrade],
        days: int = 30,
    ) -> float:
        """
        Calculate accurate ROI from historical trades.

        Uses FIFO position tracking via _replay_positions to compute
        total_cost_sold (cost basis of sold tokens) and realized PnL.
        This correctly handles DCA wallets where the denominator should
        only count the cost basis of what was actually sold.

        Uses Decimal internally for all financial calculations to avoid
        floating-point errors. Converts to float at the boundary for API
        compatibility.

        Args:
            trades: List of historical trades
            days: Time window for ROI calculation

        Returns:
            ROI as percentage
        """
        if not trades:
            return 0.0

        total_cost_sold, realized_pnl, _, _, replay_data_gap_ratio = self._replay_positions(trades)

        if total_cost_sold <= Decimal('0'):
            return 0.0

        roi_decimal = safe_decimal_divide(realized_pnl, total_cost_sold) * Decimal('100.0')
        return decimal_to_float(roi_decimal)
    
    def _calculate_win_rate_from_trades(
        self,
        trades: List[HistoricalTrade],
    ) -> float:
        """
        Calculate accurate win rate from historical trades.
        
        Uses actual PnL data to determine wins vs losses.
        
        Args:
            trades: List of historical trades
            
        Returns:
            Win rate as float (0.0 to 1.0)
        """
        if not trades:
            return 0.0

        # Only count SELL trades (closing positions) for win/loss
        closing_trades = [t for t in trades if t.action == TradeAction.SELL]
        
        if not closing_trades:
            return 0.0
        
        # Count wins and losses based on PnL
        wins = 0
        losses = 0
        
        for trade in closing_trades:
            if trade.pnl_sol is not None:
                if trade.pnl_sol > 0:
                    wins += 1
                elif trade.pnl_sol < 0:
                    losses += 1
        
        total = wins + losses
        
        if total == 0:
            return 0.0
        
        return wins / total
    
    @staticmethod
    def _calculate_alpha_decay(trades: List[HistoricalTrade]) -> Optional[float]:
        """
        Compute alpha decay: ratio of recent (last 10) win rate to all-time win rate.
        
        Returns a value in [0, infinity):
        - 1.0 = stable win rate
        - < 0.70 = significant decay (losing edge)
        - > 1.0 = improving
        
        Returns None if fewer than 3 closing trades exist.
        """
        closing = sorted(
            [t for t in trades if t.action == TradeAction.SELL and t.pnl_sol is not None],
            key=lambda t: t.timestamp,
        )
        if len(closing) < 3:
            return None
        
        all_wins = sum(1 for t in closing if t.pnl_sol > 0)
        all_losses = sum(1 for t in closing if t.pnl_sol < 0)
        all_total = all_wins + all_losses
        if all_total == 0:
            return None
        all_time_win_rate = all_wins / all_total
        
        recent = closing[-10:]
        recent_wins = sum(1 for t in recent if t.pnl_sol > 0)
        recent_losses = sum(1 for t in recent if t.pnl_sol < 0)
        recent_total = recent_wins + recent_losses
        if recent_total == 0 or all_time_win_rate == 0:
            return None
        
        recent_win_rate = recent_wins / recent_total
        return recent_win_rate / all_time_win_rate

    @staticmethod
    def _calculate_trade_size_decay(trades: List[HistoricalTrade]) -> Optional[float]:
        """
        Compute trade size decay: ratio of avg size of last N trades to first N trades.

        Returns a value in [0, 1]:
        - 1.0 = same or growing position sizes
        - < 0.5 = significantly shrinking positions (conservative behavior change)
        - None if fewer than 6 trades exist.
        """
        if len(trades) < 6:
            return None
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        # Use time-based split so clustered trades don't bias the halves
        total_span = (sorted_trades[-1].timestamp - sorted_trades[0].timestamp).total_seconds()
        if total_span >= 7 * 86400:
            midpoint = sorted_trades[0].timestamp + timedelta(seconds=total_span / 2)
            first_half = [t for t in sorted_trades if t.timestamp < midpoint]
            second_half = [t for t in sorted_trades if t.timestamp >= midpoint]
        else:
            # Fall back to count-based for short time spans
            first_half = sorted_trades[:len(sorted_trades)//2]
            second_half = sorted_trades[-(len(sorted_trades)//2):]
        avg_first = sum(t.amount_sol for t in first_half) / max(1, len(first_half))
        avg_second = sum(t.amount_sol for t in second_half) / max(1, len(second_half))
        if avg_first <= 0:
            return None
        ratio = avg_second / avg_first
        return max(0.0, min(1.0, ratio))

    @staticmethod
    def _calculate_token_rotation_decay(trades: List[HistoricalTrade]) -> Optional[float]:
        """
        Compute token rotation decay: unique tokens in recent trades vs all trades.

        Returns a value in [0, 1]:
        - 1.0 = same token diversity as historically
        - < 0.3 = stuck trading the same tokens (loss of exploratory behaviour)
        - None if fewer than 10 trades exist.
        """
        if len(trades) < 10:
            return None
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        recent = sorted_trades[-10:]
        all_unique = len(set(t.token_address for t in sorted_trades))
        recent_unique = len(set(t.token_address for t in recent))
        if all_unique <= 0:
            return None
        ratio = recent_unique / all_unique
        return max(0.0, min(1.0, ratio))

    @staticmethod
    def _calculate_composite_decay(trades: List[HistoricalTrade]) -> Optional[float]:
        """
        Compute composite alpha decay from multiple signals.

        Combines:
        - Win rate decay (weight 0.5): ratio of recent/all-time win rate
        - Trade size decay (weight 0.3): ratio of recent/first half avg size
        - Token rotation decay (weight 0.2): ratio of recent/all-time unique tokens

        Returns a score in [0, 1]:
        - 1.0 = fresh (no decay detected)
        - 0.6 = borderline decay
        - < 0.4 = significant decay
        - None if insufficient trade data for any component.
        """
        wr = WalletAnalyzer._calculate_alpha_decay(trades)
        sz = WalletAnalyzer._calculate_trade_size_decay(trades)
        rt = WalletAnalyzer._calculate_token_rotation_decay(trades)

        components = []
        if wr is not None:
            components.append(("wr", wr, 0.5))
        if sz is not None:
            components.append(("sz", sz, 0.3))
        if rt is not None:
            components.append(("rt", rt, 0.2))

        if not components:
            return None

        total_weight = sum(w for _, _, w in components)
        score = sum(v * w for _, v, w in components) / total_weight
        return max(0.0, min(1.0, score))

    @staticmethod
    def _compute_survivorship_flag(trades: List[HistoricalTrade], wallet_age_days: Optional[float] = None) -> str:
        """
        Classify wallet survivorship bias risk based on trade history span.

        Returns:
            "SURVIVED_90D" — wallet was active 30+ days ago (more cycles)
            "SURVIVED_30D" — trade span >= 30 days
            "FRESH_30D"    — trade span < 30 days (survivorship bias risk)
            "UNKNOWN"      — insufficient data
        """
        if not trades:
            return "UNKNOWN"
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        oldest = sorted_trades[0].timestamp
        newest = sorted_trades[-1].timestamp
        span_days = (newest - oldest).days
        if span_days >= 30:
            return "SURVIVED_30D"
        if wallet_age_days is not None and wallet_age_days >= 90:
            return "SURVIVED_90D"
        return "FRESH_30D"

    @staticmethod
    def _classify_token_category(token_symbol: str) -> Optional[str]:
        """Classify a token into a broad category based on symbol patterns."""
        sym = token_symbol.upper().strip()
        if not sym or sym == "UNKNOWN":
            return None
        memecoins = {"WIF", "BONK", "POPCAT", "MEW", "MYRO", "SAMO", "SLERF",
                     "BOME", "MOG", "PENGU", "PEPE", "DOGE", "SHIB", "FLOKI",
                     "MOODENG", "GOAT", "FWOG", "MICHI", "BRETT", "MOTHER"}
        infra = {"SOL", "JUP", "JTO", "RAY", "ORCA", "PYTH", "W", "DRIFT",
                 "PRCL", "ZEUS", "KMNO", "CLOUD", "TNSR"}
        defi = {"MNDE", "MUX", "UXP", "HNT", "BORG", "MPLX"}
        stable = {"USDC", "USDT", "USDS", "PYUSD", "EURC", "FDUSD"}
        gaming = {"GALA", "PORTAL", "CROWN", "NYAN"}
        if sym in memecoins:
            return "memecoin"
        if sym in infra:
            return "infrastructure"
        if sym in defi:
            return "defi"
        if sym in stable:
            return "stablecoin"
        if sym in gaming:
            return "gaming"
        if sym.endswith("COIN") or sym.endswith("DOGE"):
            return "memecoin"
        return "other"
    
    def _calculate_drawdown_from_trades(
        self,
        trades: List[HistoricalTrade],
    ) -> float:
        """
        Calculate maximum drawdown from historical trades.
        
        Tracks running PnL and identifies peak-to-trough declines.
        
        Args:
            trades: List of historical trades
            
        Returns:
            Maximum drawdown as percentage (0.0 to 100.0)
        """
        if not trades:
            return 0.0
        
        # Sort trades chronologically
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        # Build equity curve from realized PnL over SELL trades (use Decimal for precision)
        Decimal('0')
        peak = Decimal('0')
        max_dd = Decimal('0')

        cumulative_pnl = Decimal('0')

        for t in sorted_trades:
            if t.action != TradeAction.SELL or t.pnl_sol is None:
                continue
            # Ensure pnl_sol is Decimal (may be float from test data or external sources)
            pnl_decimal = t.pnl_sol if isinstance(t.pnl_sol, Decimal) else float_to_decimal(t.pnl_sol)
            cumulative_pnl += pnl_decimal

            # Reset peak if we reach a new high in cumulative PnL
            if cumulative_pnl > peak:
                peak = cumulative_pnl
            
            # Calculate drawdown from peak
            drawdown_amount = peak - cumulative_pnl
            if drawdown_amount > Decimal('0'):
                if peak > Decimal('0'):
                    current_dd = drawdown_amount / peak
                else:
                    # Peak is 0: wallet started losing immediately and never recovered.
                    # Drawdown is 100% — the wallet has never been profitable.
                    current_dd = Decimal('1.0')

                max_dd = max(max_dd, current_dd)

        # Convert to float for API compatibility
        return float(max_dd * Decimal('100'))

    
    def _calculate_win_streak_consistency(
        self,
        trades: List[HistoricalTrade],
    ) -> float:
        """
        Calculate win streak consistency from historical trades.
        
        Analyzes win/loss patterns to determine consistency.
        Higher value = more consistent winning patterns.
        
        Args:
            trades: List of historical trades
            
        Returns:
            Consistency score (0.0 to 1.0)
        """
        if not trades:
            return 0.0

        # Get closing trades with PnL
        closing_trades = [
            t for t in trades
            if t.action == TradeAction.SELL and t.pnl_sol is not None
        ]
        
        if len(closing_trades) < 5:
            return 0.0  # Need minimum trades for consistency
        
        # Determine wins/losses (1=win, 0=loss)
        outcomes = [1 if t.pnl_sol > 0 else 0 for t in closing_trades]
        n = len(outcomes)
        if n < 5:
            return 0.0

        # Streak lengths of same outcome
        current = 1
        streaks = []
        for i in range(1, n):
            if outcomes[i] == outcomes[i - 1]:
                current += 1
            else:
                streaks.append(current)
                current = 1
        streaks.append(current)

        # Longer average streak => more consistent; alternating => ~1
        mean_streak = sum(streaks) / len(streaks) if streaks else 1.0
        streak_component = mean_streak / n  # 0..1
        win_rate = sum(outcomes) / n

        consistency = (streak_component * 0.7) + (win_rate * 0.3)
        return max(0.0, min(consistency, 1.0))
    
    async def get_historical_trades(
        self,
        address: str,
        days: int = 30,
    ) -> List[HistoricalTrade]:
        """
        Get historical trades for a wallet.
        
        This method is used by the backtester to simulate trades
        under current market conditions.
        
        Fetches real transaction data from Helius API and parses
        swap transactions into structured trade data.
        
        Args:
            address: Wallet address
            days: Number of days to look back (default 30)
            
        Returns:
            List of HistoricalTrade objects
        """
        # Check cache first
        async with self._trades_cache_lock:
            if address in self._trades_cache:
                cutoff = utcnow() - timedelta(days=days)
                return [t for t in self._trades_cache[address] if t.timestamp >= cutoff]

        # Fetch real data if Helius client is available
        if self.helius_client.api_key:
            try:
                trades = await self._fetch_real_historical_trades(address, days)
                if trades:
                    await self._trades_cache_set(address, trades)
                    return trades
            except Exception as e:
                logger.warning("Failed to fetch trades for %s: %s", address[:8], e)
        
        # Fall back to cached sample data
        trades = self._trades_cache.get(address, [])
        cutoff = utcnow() - timedelta(days=days)
        return [t for t in trades if t.timestamp >= cutoff]
    
    async def _fetch_real_historical_trades(self, address: str, days: int) -> List[HistoricalTrade]:
        """
        Fetch real historical trades from Helius API.

        Also collects *current* liquidity snapshots to build a time-series liquidity
        database for future backtesting.

        IMPORTANT:
        We must never write "current liquidity" while stamping it with the *historical*
        trade timestamp. That would poison the historical liquidity table and cause
        the backtester to believe it has true historical liquidity for old timestamps.
        """
        # Check budget before calling Helius (estimate: 50 credits per wallet fetch)
        estimated_credits = 50
        can_proceed, reason = self.can_spend_budget(estimated_credits, "analysis")
        if not can_proceed:
            print(f"  [{address[:8]}] Budget check failed: {reason}")
            print(f"  [{address[:8]}] Skipping historical trades fetch due to budget constraints")
            return []

        transactions = await self.helius_client.get_wallet_transactions(
            address,
            days=days,
            limit=self._wallet_tx_limit,
        )

        # Record credit usage (value = number of trades fetched)
        credits_used = estimated_credits if transactions else 10
        self.record_credit_usage(credits_used, "analysis", value=len(transactions) if transactions else 0)
        
        trades = []
        liquidity_snapshots = []
        
        # Collect unique token addresses for batch liquidity fetching
        # This avoids redundant API calls for the same token across multiple trades
        unique_tokens = set()
        
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx, wallet_address=address)
            if swap:
                trade = await self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
                    unique_tokens.add(trade.token_address)
        
        # Offload liquidity collection to background thread pool to avoid blocking event loop
        # Collect current liquidity snapshots for unique tokens only (deduplication)
        if unique_tokens:
            try:
                # Use asyncio.to_thread to run blocking liquidity calls in thread pool
                # This prevents the synchronous get_current_liquidity calls from blocking the async loop
                liquidity_results = await asyncio.gather(
                    *[
                        asyncio.to_thread(
                            self.liquidity_provider.get_current_liquidity,
                            token_address
                        )
                        for token_address in unique_tokens
                    ],
                    return_exceptions=True  # Don't fail all if one token fetch fails
                )
                
                # Process liquidity results
                for token_address, result in zip(unique_tokens, liquidity_results):
                    if isinstance(result, Exception):
                        print(f"[Analyzer] Warning: Failed to collect liquidity for {token_address[:8]}...: {result}")
                        continue
                    
                    current_liq = result
                    if current_liq:
                        # Store snapshot at "now" (not at the trade's past timestamp).
                        historical_snapshot = LiquidityData(
                            token_address=current_liq.token_address,
                            liquidity_usd=current_liq.liquidity_usd,
                            price_usd=current_liq.price_usd,
                            volume_24h_usd=current_liq.volume_24h_usd,
                            timestamp=utcnow(),
                            source="analyzer_collection_current",
                        )
                        liquidity_snapshots.append(historical_snapshot)
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to collect liquidity snapshots: {e}")
        
        # Batch store liquidity snapshots for efficiency
        if liquidity_snapshots:
            try:
                stored_count = self.liquidity_provider.store_liquidity_batch(liquidity_snapshots)
                if stored_count > 0:
                    print(f"[Analyzer] Collected {stored_count} liquidity snapshots for {address[:8]}...")
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to store liquidity snapshots: {e}")
        
        # Enrich with realized PnL before returning/caching
        self._enrich_trades_with_realized_pnl(trades)
        return sorted(trades, key=lambda t: t.timestamp, reverse=True)
    
    async def fetch_recent_trades(self, address: str, days: int = 30) -> List[dict]:
        """
        Fetch recent trades for a wallet (legacy method).

        In production, this would query Helius API for transaction history.

        Args:
            address: Wallet address
            days: Number of days to look back

        Returns:
            List of trade dictionaries
        """
        # Convert to dict format for backwards compatibility
        trades = await self.get_historical_trades(address, days)
        return [
            {
                "token_address": t.token_address,
                "token_symbol": t.token_symbol,
                "action": t.action.value,
                "amount_sol": t.amount_sol,
                "price": t.price_at_trade,
                "timestamp": t.timestamp.isoformat(),
                "tx_signature": t.tx_signature,
                "pnl_sol": t.pnl_sol,
            }
            for t in trades
        ]

    def _categorize_parse_failure(self, tx: Dict[str, Any], wallet_address: str) -> str:
        """Categorize why parse_swap_transaction returned None for this transaction."""
        # Wallet involvement check (delegates to shared _is_wallet_involved)
        if not self.helius_client._is_wallet_involved(tx, wallet_address):
            return "not_involved"

        # Check if we have tokenTransfers at all
        token_transfers = tx.get("tokenTransfers") or []
        if not token_transfers:
            # Check if events.swap exists — if so, parser should have used Strategy 2
            events = tx.get("events", {}) or {}
            if events.get("swap"):
                # Events exist but were not sufficient for Strategy 2
                native_input = events["swap"].get("nativeInput") or events["swap"].get("nativeIn")
                native_output = events["swap"].get("nativeOutput") or events["swap"].get("nativeOut")
                token_in = events["swap"].get("tokenInputs") or events["swap"].get("tokenIn")
                token_out = events["swap"].get("tokenOutputs") or events["swap"].get("tokenOut")
                if native_input or native_output or token_in or token_out:
                    return "events_malformed"
                return "events_empty"
            return "no_token_transfers"

        # Check for primary token availability
        token_deltas: Dict[str, float] = {}
        for tr in token_transfers:
            mint = tr.get("mint", "")
            if not mint:
                continue
            amt = self.helius_client._parse_ui_token_amount(tr)
            if tr.get("fromUserAccount") == wallet_address:
                token_deltas[mint] = token_deltas.get(mint, 0.0) - amt
            if tr.get("toUserAccount") == wallet_address:
                token_deltas[mint] = token_deltas.get(mint, 0.0) + amt

        has_non_sol = any(m != "So11111111111111111111111111111111111111112" for m in token_deltas)
        if not has_non_sol:
            return "no_primary_token"

        # Check for direction ambiguities
        sol_delta = Decimal(str(token_deltas.get("So11111111111111111111111111111111111111112", 0)))
        native_transfers = tx.get("nativeTransfers") or []
        for nt in native_transfers:
            if nt.get("fromUserAccount") == wallet_address:
                sol_delta -= Decimal(str(nt.get("amount", 0)))
            if nt.get("toUserAccount") == wallet_address:
                sol_delta += Decimal(str(nt.get("amount", 0)))

        non_sol_mints = [m for m in token_deltas if m != "So11111111111111111111111111111111111111112"]
        has_positive = any(token_deltas[m] > 0 for m in non_sol_mints)
        has_negative = any(token_deltas[m] < 0 for m in non_sol_mints)
        has_sol_movement = abs(sol_delta) > Decimal('0.001')

        if not has_sol_movement and (not has_positive or not has_negative):
            return "direction_ambiguous"

        return "unknown"

    def print_parse_health_dashboard(self) -> None:
        """Print parse health diagnostics at end of run."""
        # Pull discovery stats from HeliusClient
        hstats = self.helius_client.get_discovery_stats()
        self._discovery_stats["infrastructure_filtered"] = hstats.get("infrastructure_filtered", 0)
        self._discovery_stats["balance_checked"] = hstats.get("balance_checked", 0)
        self._discovery_stats["balance_filtered"] = hstats.get("balance_filtered", 0)

        stats = self._parse_stats
        disc = self._discovery_stats
        total = stats["transactions_fetched"]
        parsed = stats["swaps_parsed"]
        valid = stats["trades_valid"]
        failures = stats["parse_failures_total"]
        by_reason = stats["parse_failures_by_reason"]

        # Update stats with instance cache counters
        stats["parse_cache_hits"] = self._parse_cache_hits
        stats["parse_cache_misses"] = self._parse_cache_misses

        print("\n[Analyzer] ════════════════════════════════════════")
        print("[Analyzer] Parse Health Dashboard")
        print("[Analyzer] ════════════════════════════════════════")
        overall_parse_pct = parsed / max(1, total) * 100
        warn_pct = float(os.getenv("SCOUT_PARSE_HEALTH_WARN_PCT", "50"))
        crit_pct = float(os.getenv("SCOUT_PARSE_HEALTH_CRIT_PCT", "30"))

        print(f"  Transactions fetched:  {total}")
        print(f"  Swaps parsed:          {parsed}  ({overall_parse_pct:.1f}%)")
        print(f"  Trades valid:          {valid}")
        print(f"  Parse failures:        {failures}")
        if failures > 0:
            pct = failures / max(1, total) * 100
            print(f"  Failure rate:          {pct:.1f}%")
            print("  Failures by reason:")
            for reason, count in sorted(by_reason.items(), key=lambda x: -x[1]):
                print(f"    {reason:<22s}: {count}")

        # Configurable health warnings
        if overall_parse_pct < crit_pct:
            print(f"  🔴 CRITICAL: Overall parse rate {overall_parse_pct:.1f}% < {crit_pct:.0f}%")
        elif overall_parse_pct < warn_pct:
            print(f"  🟡 WARNING: Overall parse rate {overall_parse_pct:.1f}% < {warn_pct:.0f}%")
        print()
        print("[Analyzer] Discovery Quality")
        print(f"  Infrastructure filtered:  {disc['infrastructure_filtered']}")
        print(f"  Balance checked:          {disc['balance_checked']}")
        print(f"  Balance filtered (0 SOL): {disc['balance_filtered']}")
        print(f"  Wallets with no trades:   {disc['wallets_with_no_trades']}")
        
        # Token creation time fetch success rate
        tcf = stats.get("token_creation_fetched", 0)
        tcs = stats.get("token_creation_success", 0)
        tcfb = stats.get("token_creation_fallback_helix", 0)
        if tcf > 0:
            success_rate = tcs / max(1, tcf) * 100
            fallback_rate = tcfb / max(1, tcf) * 100
            print()
            print("[Analyzer] Token Creation Time Quality")
            print(f"  Fetched:               {tcf}")
            print(f"  Successful:            {tcs}  ({success_rate:.1f}%)")
            print(f"  Helius fallback used:  {tcfb}  ({fallback_rate:.1f}%)")
            if success_rate < 20:
                print("  ⚠ WARNING: Token creation fetch success < 20% — sniper detection degraded!")

        # Parse cache statistics
        cache_hits = stats.get("parse_cache_hits", self._parse_cache_hits)
        cache_misses = stats.get("parse_cache_misses", self._parse_cache_misses)
        total_cache_lookups = cache_hits + cache_misses
        if total_cache_lookups > 0:
            cache_hit_rate = cache_hits / total_cache_lookups * 100
            print()
            print("[Analyzer] Parse Cache Statistics")
            print(f"  Cache hits:            {cache_hits}  ({cache_hit_rate:.1f}%)")
            print(f"  Cache misses:          {cache_misses}")
            print(f"  Total lookups:         {total_cache_lookups}")
            if cache_hit_rate < 10:
                print("  ⚠ WARNING: Low cache hit rate — consider increasing cache size or run duration")
        print("[Analyzer] ════════════════════════════════════════")

    def is_parse_rate_below_threshold(self) -> bool:
        """Check if overall parse rate is below the exit-fail threshold.

        Returns True when the scout should exit non-zero so cron can alert.
        Threshold controlled by SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT (default 40).
        """
        stats = self._parse_stats
        total = stats["transactions_fetched"]
        parsed = stats["swaps_parsed"]
        if total == 0:
            return False
        rate = parsed / max(1, total) * 100
        exit_pct = float(os.getenv("SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT", "40"))
        return rate < exit_pct

    def get_overall_parse_rate(self) -> float:
        """Return overall parse rate across all wallets (0.0-1.0)."""
        stats = self._parse_stats
        total = stats["transactions_fetched"]
        parsed = stats["swaps_parsed"]
        if total == 0:
            return 1.0
        return parsed / total

    # Phase 5: Batch Processing Optimization
    # ========================================

    async def analyze_wallets_batch(
        self,
        addresses: List[str],
        batch_size: int = 50,
        concurrency_per_batch: int = 5,
        progress_callback: Optional[callable] = None,
    ) -> Dict[str, Optional[WalletMetrics]]:
        """
        Analyze multiple wallets in optimized batches with controlled concurrency.

        Growth-optimized batch processing (Phase 5):
        - Processes wallets in batches of 50 for optimal memory usage
        - Uses controlled concurrency (default 5 parallel wallets) within each batch
        - Sequential batch processing for rate limit compliance
        - Progress tracking and error handling

        Args:
            addresses: List of wallet addresses to analyze
            batch_size: Number of wallets per batch (default 50 for 8x speedup)
            concurrency_per_batch: Parallel wallets within each batch (default 5)
            progress_callback: Optional callback(batch_num, total_batches, processed, total)

        Returns:
            Dict mapping address -> WalletMetrics (None if failed)
        """
        results = {}
        total_wallets = len(addresses)
        total_batches = (total_wallets + batch_size - 1) // batch_size

        logger.info(f"[Batch Process] Starting: {total_wallets} wallets in {total_batches} batches")
        logger.info(f"  Batch size: {batch_size}, Concurrency: {concurrency_per_batch}")

        # Get discovery concurrency from config if available
        if CONFIG_AVAILABLE and ScoutConfig:
            concurrency_per_batch = ScoutConfig.get_discovery_concurrency()

        for batch_num, i in enumerate(range(0, total_wallets, batch_size), 1):
            batch_addresses = addresses[i:i + batch_size]
            batch_results = await self._process_batch(
                batch_addresses,
                concurrency=concurrency_per_batch,
            )

            # Store results
            results.update(batch_results)

            # Progress callback
            if progress_callback:
                progress_callback(
                    batch_num,
                    total_batches,
                    len(results),
                    total_wallets,
                )

            # Log progress
            processed = len(results)
            success_count = sum(1 for m in results.values() if m is not None)
            logger.info(
                f"[Batch {batch_num}/{total_batches}] "
                f"Processed {processed}/{total_wallets} wallets "
                f"({success_count} successful)"
            )

        logger.info(f"[Batch Process] Complete: {sum(1 for m in results.values() if m is not None)}/{total_wallets} successful")
        return results

    async def _process_batch(
        self,
        addresses: List[str],
        concurrency: int = 5,
    ) -> Dict[str, Optional[WalletMetrics]]:
        """
        Process a single batch of wallets with controlled concurrency.

        Uses asyncio.Semaphore for rate limit compliance and memory control.

        Args:
            addresses: List of wallet addresses in this batch
            concurrency: Maximum parallel requests

        Returns:
            Dict mapping address -> WalletMetrics
        """
        results = {}
        semaphore = asyncio.Semaphore(concurrency)

        async def process_one(address: str) -> Tuple[str, Optional[WalletMetrics]]:
            async with semaphore:
                try:
                    metrics = await self.get_wallet_metrics(address)
                    return (address, metrics)
                except Exception as e:
                    logger.warning(f"Failed to analyze {address[:8]}...: {e}")
                    return (address, None)

        # Process all wallets in batch with controlled concurrency
        tasks = [process_one(addr) for addr in addresses]
        batch_results = await asyncio.gather(*tasks, return_exceptions=True)

        # Collect results
        for result in batch_results:
            if isinstance(result, Exception):
                continue
            if result and isinstance(result, tuple):
                address, metrics = result
                results[address] = metrics

        return results

    async def prefetch_wallet_data(
        self,
        addresses: List[str],
        prefetch_token_meta: bool = True,
        prefetch_prices: bool = True,
    ) -> None:
        """
        Prefetch common data across all wallets for batch optimization.

        Growth-aware prefetching (Phase 5):
        - Token metadata (cached for 24 hours)
        - SOL price (cached for 5 minutes)
        - Wallet ages (cached for 30 days)

        Args:
            addresses: List of wallet addresses to prefetch for
            prefetch_token_meta: Whether to prefetch token metadata
            prefetch_prices: Whether to prefetch current prices
        """
        logger.info(f"[Prefetch] Starting data prefetch for {len(addresses)} wallets")

        # Prefetch SOL price (needed for USD conversions)
        if prefetch_prices:
            try:
                await self._get_sol_price_usd()
                logger.info("[Prefetch] SOL price cached")
            except Exception as e:
                logger.warning(f"[Prefetch] Failed to fetch SOL price: {e}")

        # Prefetch wallet ages for insider detection
        # This is done in parallel for all wallets
        try:
            age_tasks = [self._get_wallet_creation_time_cached(addr) for addr in addresses]
            await asyncio.gather(*age_tasks, return_exceptions=True)
            logger.info(f"[Prefetch] Wallet ages cached for {len(addresses)} wallets")
        except Exception as e:
            logger.warning(f"[Prefetch] Failed to prefetch wallet ages: {e}")

        logger.info("[Prefetch] Complete")

    async def shutdown(self):
        """Cleanup and shutdown (close HTTP sessions)."""
        if self.helius_client:
            try:
                await self.helius_client.close()
            except Exception:
                pass  # Non-critical
        if self.rugcheck_client:
            try:
                await self.rugcheck_client.close()
            except Exception:
                pass  # Non-critical
        if self.liquidity_provider:
            try:
                await self.liquidity_provider.close()
            except Exception:
                pass  # Non-critical

# Example usage
if __name__ == "__main__":
    from .wqs import calculate_wqs, classify_wallet
    
    analyzer = WalletAnalyzer()
    
    print("Analyzing candidate wallets:")
    print("-" * 60)
    
    for address in analyzer.get_candidate_wallets():
        metrics = analyzer.get_wallet_metrics(address)
        if metrics:
            wqs = calculate_wqs(metrics)
            status = classify_wallet(wqs)
            trades = analyzer.get_historical_trades(address)
            print(f"{address[:8]}... | WQS: {wqs:5.1f} | Status: {status} | Trades: {len(trades)}")
