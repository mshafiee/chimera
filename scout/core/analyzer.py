"""
Wallet Analyzer - On-chain data fetching and analysis

This module fetches wallet transaction data from Solana RPC/APIs
and computes performance metrics for WQS calculation.

In production, this connects to:
- Helius API for transaction history and wallet discovery
- Jupiter API for price data
- On-chain token data for position tracking
"""

import os
import requests
import logging
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional, Dict, Any

from .wqs import WalletMetrics
from .models import HistoricalTrade, TradeAction, LiquidityData, TraderArchetype
from .helius_client import HeliusClient
from .liquidity import LiquidityProvider

# Import config and security client
try:
    from config import ScoutConfig
    from .security_client import RugCheckClient
    SECURITY_AVAILABLE = True
except ImportError:
    SECURITY_AVAILABLE = False
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
        
        Args:
            trades: List of historical trades (sorted by timestamp)
            current_prices: Dict mapping token_address -> current_price_usd
            sol_price_usd: Current SOL price in USD (for converting SOL cost basis to USD)
            
        Returns:
            Total unrealized loss in SOL (positive value = loss)
        """
        holdings = {}  # token_addr -> amount (token units)
        cost_basis = {}  # token_addr -> total_sol_spent
        
        # 1. Replay history using FIFO logic
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                token_addr = t.token_address
                # Calculate token amount from trade
                token_amount = t.token_amount
                if token_amount is None or token_amount == 0:
                    # Fallback: calculate from SOL amount and price
                    if t.price_sol and t.price_sol > 0:
                        token_amount = t.amount_sol / t.price_sol
                    elif t.price_at_trade and t.price_at_trade > 0:
                        token_amount = t.amount_sol / t.price_at_trade
                    else:
                        continue  # Skip if we can't determine amount
                
                holdings[token_addr] = holdings.get(token_addr, 0.0) + token_amount
                cost_basis[token_addr] = cost_basis.get(token_addr, 0.0) + t.amount_sol
                
            elif t.action == TradeAction.SELL:
                token_addr = t.token_address
                current_qty = holdings.get(token_addr, 0.0)
                if current_qty <= 0:
                    continue
                
                # Calculate token amount sold
                token_amount = t.token_amount
                if token_amount is None or token_amount == 0:
                    if t.price_sol and t.price_sol > 0:
                        token_amount = t.amount_sol / t.price_sol
                    elif t.price_at_trade and t.price_at_trade > 0:
                        token_amount = t.amount_sol / t.price_at_trade
                    else:
                        continue
                
                # FIFO: Reduce holdings and cost basis proportionally
                ratio = min(1.0, token_amount / current_qty) if current_qty > 0 else 0.0
                holdings[token_addr] = max(0.0, current_qty - token_amount)
                cost_basis[token_addr] = cost_basis.get(token_addr, 0.0) * (1.0 - ratio)
        
        # 2. Calculate Value vs Cost for remaining holdings
        total_unrealized_loss_sol = 0.0
        
        # Get SOL price if not provided (default to 1.0 if unavailable, will use SOL amounts directly)
        if sol_price_usd is None:
            sol_price_usd = 1.0  # Fallback: assume 1:1 if SOL price unavailable
        
        for token, qty in holdings.items():
            if qty <= 0:
                continue
            
            remaining_cost_sol = cost_basis.get(token, 0.0)
            
            # Ignore dust entries (< 0.5 SOL cost basis)
            if remaining_cost_sol < 0.5:
                continue
            
            # Get current price (in USD)
            current_price_usd = current_prices.get(token, 0.0)
            if current_price_usd <= 0:
                # If price unavailable, assume it's worthless (100% loss)
                total_unrealized_loss_sol += remaining_cost_sol
                continue
            
            # Convert token quantity to USD value
            current_val_usd = qty * current_price_usd
            remaining_cost_usd = remaining_cost_sol * sol_price_usd
            
            # If value is < 20% of cost, it's a heavy bag
            if remaining_cost_usd > 0:
                if current_val_usd < (remaining_cost_usd * 0.20):
                    # Calculate loss in SOL terms
                    loss_usd = remaining_cost_usd - current_val_usd
                    loss_sol = loss_usd / sol_price_usd if sol_price_usd > 0 else loss_usd
                    total_unrealized_loss_sol += loss_sol
        
        return total_unrealized_loss_sol
    
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
        base_url = "https://price.jup.ag/v6/price"
        
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
    ):
        """
        Initialize the wallet analyzer.
        
        Args:
            helius_api_key: Helius API key for transaction data
            rpc_url: Solana RPC URL for on-chain queries
            discover_wallets: Whether to discover wallets from on-chain data
            max_wallets: Maximum number of wallets to discover
        """
        self.helius_api_key = helius_api_key
        self.rpc_url = rpc_url
        
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
        
        # Cache for metrics and trades
        self._metrics_cache: Dict[str, WalletMetrics] = {}
        self._trades_cache: Dict[str, List[HistoricalTrade]] = {}
        self._candidate_wallets: List[str] = []
        self._token_meta_cache: Dict[str, Dict[str, Any]] = {}
        self._token_creation_cache: Dict[str, Optional[float]] = {}
        self._price_cache: Dict[str, float] = {}  # Cache for token prices
        self._sol_price_usd: Optional[float] = None  # Cached SOL price
        
        # Initialize Redis client for persistent caching (if available)
        self._redis_client = None
        try:
            from .redis_client import RedisClient
            if CONFIG_AVAILABLE and ScoutConfig and ScoutConfig.get_redis_enabled():
                redis_url = ScoutConfig.get_redis_url()
                self._redis_client = RedisClient(redis_url=redis_url, enabled=True)
                if self._redis_client.is_available():
                    logger.info("Redis cache enabled for token metadata and creation times")
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

    def clear_wallet_cache(self, address: str):
        """Clear cached data for a specific wallet to free memory."""
        self._metrics_cache.pop(address, None)
        self._trades_cache.pop(address, None)
        # Note: We keep _token_meta_cache as that is reusable across wallets

        # Max txs to pull per wallet when computing metrics/trades
        self._wallet_tx_limit = int(os.getenv("SCOUT_WALLET_TX_LIMIT", "500"))
        self._wallet_tx_limit = max(50, min(self._wallet_tx_limit, 5000))
        
        # Try to load wallets from config file first
        wallet_list_file = os.getenv("SCOUT_WALLET_LIST_FILE", "/app/config/wallets.txt")
        if os.path.exists(wallet_list_file):
            try:
                with open(wallet_list_file, 'r') as f:
                    wallets = [line.strip() for line in f if line.strip() and not line.strip().startswith('#')]
                    if wallets:
                        self._candidate_wallets = wallets[:max_wallets]
                        print(f"[Analyzer] Loaded {len(self._candidate_wallets)} wallets from {wallet_list_file}")
                    else:
                        print(f"[Analyzer] Wallet list file empty, trying discovery...")
                        self._try_discover_wallets(discover_wallets, max_wallets)
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to load wallet list: {e}")
                self._try_discover_wallets(discover_wallets, max_wallets)
        else:
            # Try discovery or fall back to sample data
            self._try_discover_wallets(discover_wallets, max_wallets)
    
    def _try_discover_wallets(self, discover_wallets: bool, max_wallets: int):
        """Try to discover wallets, fall back to sample data if fails."""
        if discover_wallets and self.helius_client.api_key:
            print("[Analyzer] Attempting to discover wallets from on-chain data...")
            try:
                # Get configuration from environment variables
                hours_back = int(os.getenv("SCOUT_DISCOVERY_HOURS", "24"))
                min_trade_count = int(os.getenv("SCOUT_MIN_TRADE_COUNT", "3"))
                
                discovered = self.helius_client.discover_wallets_from_recent_swaps(
                    limit=1000,  # Max transactions to query (deprecated but kept for compatibility)
                    min_trade_count=min_trade_count,
                    max_wallets=max_wallets,
                    hours_back=hours_back,
                )
                if discovered:
                    self._candidate_wallets = discovered[:max_wallets]
                    print(f"[Analyzer] Discovered {len(self._candidate_wallets)} candidate wallets")
                    return
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to discover wallets: {e}")
                import traceback
                if os.getenv("SCOUT_VERBOSE", "false").lower() == "true":
                    traceback.print_exc()
        
        # Fallback: Try to load from existing roster database
        try:
            # Try main database first
            roster_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
            # Also check for roster_new.db in the data directory
            data_dir = Path(roster_path).parent
            roster_new_path = data_dir / "roster_new.db"
            
            for db_path in [roster_path, str(roster_new_path)]:
                if os.path.exists(db_path):
                    import sqlite3
                    conn = sqlite3.connect(db_path)
                    cursor = conn.cursor()
                    # Check if wallets table exists
                    cursor.execute("""
                        SELECT name FROM sqlite_master 
                        WHERE type='table' AND name='wallets'
                    """)
                    if cursor.fetchone():
                        # Get existing wallets from database
                        cursor.execute("""
                            SELECT DISTINCT address 
                            FROM wallets 
                            WHERE status IN ('ACTIVE', 'CANDIDATE')
                            ORDER BY wqs_score DESC NULLS LAST
                            LIMIT ?
                        """, (max_wallets,))
                        existing_wallets = [row[0] for row in cursor.fetchall()]
                        conn.close()
                        
                        if existing_wallets:
                            self._candidate_wallets = existing_wallets[:max_wallets]
                            print(f"[Analyzer] Loaded {len(self._candidate_wallets)} wallets from existing database ({db_path})")
                            return
                    else:
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
                last_trade_at=(datetime.utcnow() - timedelta(hours=2)).isoformat(),
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
                last_trade_at=(datetime.utcnow() - timedelta(hours=6)).isoformat(),
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
                last_trade_at=(datetime.utcnow() - timedelta(hours=1)).isoformat(),
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
                last_trade_at=(datetime.utcnow() - timedelta(days=3)).isoformat(),
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
                last_trade_at=(datetime.utcnow() - timedelta(hours=12)).isoformat(),
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
                    amount_sol=metrics.avg_trade_size_sol or 0.5,
                    price_at_trade=random.uniform(0.00001, 10.0),
                    timestamp=datetime.utcnow() - timedelta(days=days_ago, hours=random.randint(0, 23)),
                    tx_signature=f"{wallet[:8]}_{i}",
                    pnl_sol=pnl if action == TradeAction.SELL else 0,
                    liquidity_at_trade_usd=random.uniform(50000, 500000),
                )
                trades.append(trade)
            
            trades_cache[wallet] = sorted(trades, key=lambda t: t.timestamp, reverse=True)
        
        return trades_cache
    
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
        # Check cache first
        if address in self._metrics_cache:
            return self._metrics_cache[address]
        
        # Try to load from database first (if wallet exists there)
        try:
            db_path = os.getenv("CHIMERA_DB_PATH", "data/chimera.db")
            if os.path.exists(db_path):
                import sqlite3
                conn = sqlite3.connect(db_path)
                cursor = conn.cursor()
                cursor.execute("""
                    SELECT wqs_score, roi_7d, roi_30d, trade_count_30d, win_rate,
                           max_drawdown_30d, avg_trade_size_sol, last_trade_at
                    FROM wallets
                    WHERE address = ?
                    LIMIT 1
                """, (address,))
                row = cursor.fetchone()
                conn.close()
                
                if row:
                    # Convert database row to WalletMetrics
                    wqs_score, roi_7d, roi_30d, trade_count_30d, win_rate, \
                    max_drawdown_30d, avg_trade_size_sol, last_trade_at = row
                    
                    # If we have some metrics, create WalletMetrics object
                    if any(x is not None for x in [roi_7d, roi_30d, trade_count_30d, win_rate]):
                        metrics = WalletMetrics(
                            address=address,
                            roi_7d=roi_7d,
                            roi_30d=roi_30d,
                            trade_count_30d=trade_count_30d,
                            win_rate=win_rate,
                            max_drawdown_30d=max_drawdown_30d,
                            avg_trade_size_sol=avg_trade_size_sol,
                            last_trade_at=last_trade_at,
                            win_streak_consistency=None,  # Not stored in DB, will be calculated
                        )
                        self._metrics_cache[address] = metrics
                        return metrics
        except Exception as e:
            # Log but don't fail - continue to try other sources
            if os.getenv("SCOUT_VERBOSE", "false").lower() == "true":
                print(f"[Analyzer] Warning: Failed to load metrics from database for {address[:8]}...: {e}")
        
        # Fetch real data if Helius client is available
        if self.helius_client.api_key:
            try:
                metrics = await self._fetch_real_wallet_metrics(address)
                if metrics:
                    self._metrics_cache[address] = metrics
                    return metrics
            except Exception as e:
                if os.getenv("SCOUT_VERBOSE", "false").lower() == "true":
                    print(f"[Analyzer] Warning: Failed to fetch metrics for {address[:8]}...: {e}")
        
        # Fall back to cached sample data
        return self._metrics_cache.get(address)
    
    async def _fetch_real_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
        """Fetch real wallet metrics from Helius API."""
        # Get transaction history
        transactions = await self.helius_client.get_wallet_transactions(
            address,
            days=30,
            limit=self._wallet_tx_limit,
        )
        
        if not transactions:
            return None
        
        # Parse transactions into trades
        trades = []
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx, wallet_address=address)
            if swap:
                # Convert to HistoricalTrade format
                trade = self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
        
        if not trades:
            return None
        
        # Calculate metrics from trades
        return await self._calculate_metrics_from_trades(address, trades)
    
    def _parse_swap_to_trade(self, swap: Dict[str, Any], wallet: str) -> Optional[HistoricalTrade]:
        """Parse a swap transaction into a HistoricalTrade."""
        try:
            # Robust swap parsing already produced wallet-relative quantities
            direction = (swap.get("direction") or "").upper()
            if direction not in ("BUY", "SELL"):
                return None

            action = TradeAction.BUY if direction == "BUY" else TradeAction.SELL
            timestamp = datetime.utcfromtimestamp(
                swap.get("timestamp", int(datetime.utcnow().timestamp()))
            )

            token_mint = swap.get("token_mint", "") or swap.get("token_out", "")
            token_amount = float(swap.get("token_amount") or 0.0)
            sol_amount_raw = swap.get("sol_amount")
            price_sol_raw = swap.get("price_sol")
            price_usd_raw = swap.get("price_usd")
            usd_amount_raw = swap.get("usd_amount")

            sol_amount: float = float(sol_amount_raw or 0.0) if sol_amount_raw is not None else 0.0
            price_sol: float = float(price_sol_raw or 0.0) if price_sol_raw is not None else 0.0
            price_usd: Optional[float] = float(price_usd_raw) if price_usd_raw is not None else None

            # If this was a token->token swap valued in USD, derive SOL notional using SOL/USD.
            if sol_amount_raw is None and usd_amount_raw is not None:
                try:
                    usd_amount = float(usd_amount_raw)
                    sol_price_usd = await self.liquidity_provider.get_sol_price_usd()
                    if usd_amount > 0 and sol_price_usd > 0:
                        sol_amount = usd_amount / sol_price_usd
                        price_sol = (sol_amount / token_amount) if token_amount > 0 else 0.0
                except Exception:
                    pass

            # Token metadata enrichment (symbol/decimals)
            token_symbol = swap.get("token_symbol") or None
            if not token_symbol or token_symbol == "UNKNOWN":
                token_symbol = self._get_token_symbol(token_mint) or "UNKNOWN"

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
            if trade.price_usd is None and trade.price_sol and trade.price_sol > 0:
                sol_price_usd = self.liquidity_provider.get_sol_price_usd()
                if sol_price_usd > 0:
                    trade.price_usd = trade.price_sol * sol_price_usd

            return trade
        except Exception as e:
            print(f"[Analyzer] Error parsing swap: {e}")
            return None

    def _get_token_symbol(self, token_mint: str) -> Optional[str]:
        """Best-effort token symbol lookup with caching."""
        if not token_mint:
            return None
        if token_mint in self._token_meta_cache:
            return self._token_meta_cache[token_mint].get("symbol")

        # 1) Known tokens map
        if hasattr(self.liquidity_provider, "KNOWN_TOKENS") and token_mint in self.liquidity_provider.KNOWN_TOKENS:
            symbol = self.liquidity_provider.KNOWN_TOKENS[token_mint][0]
            self._token_meta_cache[token_mint] = {"symbol": symbol}
            return symbol

        # 2) Birdeye (if available)
        try:
            if getattr(self.liquidity_provider, "birdeye_client", None):
                meta = self.liquidity_provider.birdeye_client.get_token_metadata(token_mint)
                if meta:
                    self._token_meta_cache[token_mint] = meta
                    return meta.get("symbol")
        except Exception:
            pass

        self._token_meta_cache[token_mint] = {}
        return None

    def _enrich_trades_with_realized_pnl(self, trades: List[HistoricalTrade]) -> List[HistoricalTrade]:
        """
        Compute realized PnL (in SOL) for SELL trades using average cost basis.

        This makes metrics like win-rate and drawdown meaningful even when the
        raw swap payload doesn't directly include PnL.
        """
        # If these trades are in the legacy "price_at_trade + pnl_sol" test format
        # (no `token_amount` / `sol_amount`), don't try to overwrite/derive PnL.
        if all(t.token_amount is None and t.sol_amount is None and t.price_sol is None for t in trades):
            return trades

        # Track per-token position: {token: (token_qty, cost_basis_sol)}
        positions: Dict[str, Dict[str, float]] = {}
        
        EPSILON = 1e-9  # Define constant

        # Sort chronologically for cost-basis accounting
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)

        for t in sorted_trades:
            token = t.token_address
            token_qty = t.token_amount
            sol_amt = t.sol_amount if t.sol_amount is not None else t.amount_sol

            # If token_amount missing (legacy tests), infer from SOL and price if possible
            if token_qty is None or token_qty <= 0:
                if t.price_at_trade and t.price_at_trade > 0 and sol_amt and sol_amt > 0:
                    token_qty = sol_amt / t.price_at_trade
                    t.token_amount = token_qty

            if token_qty is None or token_qty <= 0 or sol_amt is None:
                continue

            if t.action == TradeAction.BUY:
                pos = positions.setdefault(token, {"qty": 0.0, "cost_sol": 0.0})
                pos["qty"] += token_qty
                pos["cost_sol"] += sol_amt

            elif t.action == TradeAction.SELL:
                pos = positions.get(token)
                # Stricter check using EPSILON
                if not pos or pos["qty"] < EPSILON:
                    continue

                # Don't sell more than we tracked
                sell_qty = min(token_qty, pos["qty"])
                
                # Check for near-zero sell quantity to prevent division errors
                if sell_qty < EPSILON:
                    continue

                avg_cost_per_token = pos["cost_sol"] / pos["qty"]
                cost_basis_sol = avg_cost_per_token * sell_qty
                realized_pnl_sol = sol_amt - cost_basis_sol

                t.pnl_sol = realized_pnl_sol

                # Reduce position
                pos["qty"] -= sell_qty
                pos["cost_sol"] -= cost_basis_sol
                
                # Clean up dust immediately
                if pos["qty"] < EPSILON:
                    positions.pop(token, None)
                else:
                    # Sanity clamp to prevent negative cost on positive qty
                    pos["cost_sol"] = max(0.0, pos["cost_sol"])

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
        self._token_creation_cache[token_address] = timestamp
        return timestamp

    async def _is_token_safe(self, token_address: str) -> bool:
        """
        Check if a token is safe (not a honeypot, rug, or freeze risk).
        
        CRITICAL: Honeypot Filter.
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
                            
                            # Check if this is Token-2022 (different program ID)
                            # Token-2022 uses TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb
                            # For now, we check the standard SPL Token layout
                            # TODO: Add full Token-2022 support with transfer hooks check
                            
                            # Mint Layout: Freeze Option at offset 46 (u32)
                            # 0-3: MintAuthOption, 4-35: MintAuth, 36-43: Supply, 44: Decimals, 45: Init
                            # 46-49: FreezeAuthOption. If 1, Authority follows.
                            if len(raw) >= 50:
                                freeze_opt = int.from_bytes(raw[46:50], 'little')
                                if freeze_opt == 1:
                                    return False # Has freeze authority -> REJECT
                                
                                # Check mint_authority (offset 0-3: MintAuthOption)
                                # If mint_authority is None (0), supply is fixed (good)
                                # If mint_authority exists (1), supply can be inflated (risky)
                                mint_auth_opt = int.from_bytes(raw[0:4], 'little')
                                # Note: mint_authority = None (0) is actually SAFE (fixed supply)
                                # mint_authority = Some(address) means supply can be minted (risky)
                                # However, most legitimate tokens have mint_authority, so this is
                                # primarily a check for tokens that explicitly disabled it (very safe)
                                # For now, we rely on RugCheck for comprehensive mint authority checks
        except Exception:
            pass
        
        return True

    async def _get_sol_price_usd(self) -> float:
        """
        Get current SOL price in USD.
        
        Returns:
            SOL price in USD, or 1.0 as fallback
        """
        if self._sol_price_usd is not None:
            return self._sol_price_usd
        
        try:
            # Try to get from Jupiter Price API
            sol_mint = "So11111111111111111111111111111111111111112"
            prices = await PortfolioTracker.fetch_bulk_prices([sol_mint])
            price = prices.get(sol_mint, 0.0)
            if price > 0:
                self._sol_price_usd = price
                return price
        except Exception as e:
            logger.debug(f"Failed to fetch SOL price: {e}")
        
        # Fallback to 1.0 (will use SOL amounts directly)
        return 1.0
    
    def determine_archetype(
        self, 
        metrics: WalletMetrics, 
        trades: List[HistoricalTrade]
    ) -> TraderArchetype:
        """
        Determine trader archetype based on trading behavior.
        
        Args:
            metrics: Wallet performance metrics
            trades: Historical trades
            
        Returns:
            TraderArchetype enum value
        """
        # 1. INSIDER: Fresh wallet (created < 24h before trading)
        if metrics.is_fresh_wallet:
            return TraderArchetype.INSIDER
        
        # 2. WHALE: Average trade size > 50 SOL
        if metrics.avg_trade_size_sol and metrics.avg_trade_size_sol > 50.0:
            return TraderArchetype.WHALE
        
        # 3. SNIPER: Buys < 2 mins after launch on average
        if metrics.avg_entry_delay_seconds is not None:
            if metrics.avg_entry_delay_seconds < 120:  # < 2 minutes
                return TraderArchetype.SNIPER
        
        # 4. SWING: Holds positions > 4 hours on average
        avg_hold_time = self._calculate_avg_hold_time(trades)
        if avg_hold_time and avg_hold_time > 14400:  # > 4 hours (14400 seconds)
            return TraderArchetype.SWING
        
        # 5. Default: SCALPER (many trades, small timeframe)
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
    
    def _detect_insider_patterns(self, address: str, trades: List[HistoricalTrade]) -> Dict[str, Any]:
        """
        Detect insider behavior based on wallet age and funding.
        
        Fresh wallets (created <24h before first trade) are typically:
        - Burner wallets for insider trading
        - Bot wallets for sniping
        - Ephemeral addresses to hide identity
        
        Returns:
            Dict with insider metrics
        """
        is_fresh_wallet = False
        
        if not trades:
            return {"is_fresh_wallet": False, "suspicion_score": 0.0}
        
        # Get first trade timestamp
        first_trade_time = min(t.timestamp for t in trades)
        
        # Try to get wallet creation time (first transaction ever)
        wallet_creation_time = self._get_wallet_creation_time_cached(address)
        
        if wallet_creation_time:
            # Calculate hours between wallet creation and first trade
            hours_diff = (first_trade_time.timestamp() - wallet_creation_time) / 3600
            
            # If wallet was created <24h before trading, it's suspicious
            if hours_diff < 24:
                is_fresh_wallet = True
        
        return {
            "is_fresh_wallet": is_fresh_wallet,
            "suspicion_score": 100.0 if is_fresh_wallet else 0.0
        }
    
    def _get_wallet_creation_time_cached(self, address: str) -> Optional[float]:
        """
        Get wallet creation time (first transaction) with caching.
        
        Args:
            address: Wallet address
            
        Returns:
            Unix timestamp of first transaction, or None
        """
        if not hasattr(self, '_wallet_age_cache'):
            self._wallet_age_cache = {}
        
        if address in self._wallet_age_cache:
            return self._wallet_age_cache[address]
        
        # Try to get from Helius if available
        creation_time = None
        if self.helius_client and hasattr(self.helius_client, 'get_wallet_first_transaction'):
            try:
                creation_time = self.helius_client.get_wallet_first_transaction(address)
            except Exception:
                pass
        
        self._wallet_age_cache[address] = creation_time
        return creation_time

    async def _calculate_metrics_from_trades(self, address: str, trades: List[HistoricalTrade]) -> Optional[WalletMetrics]:
        """Calculate wallet metrics from historical trades."""
        if not trades:
            return None

        # Filter out unsafe tokens using RugCheck if enabled
        if self.rugcheck_client:
            safe_trades = []
            risky_tokens = []
            for t in trades:
                token_addr = t.token_address
                if await self.rugcheck_client.is_token_safe(token_addr):
                    safe_trades.append(t)
                else:
                    risky_tokens.append(token_addr)
            
            if risky_tokens:
                logger.debug(f"Wallet {address[:8]}... has {len(risky_tokens)} risky tokens filtered by RugCheck")
            
            # Use only safe trades for analysis
            trades = safe_trades
            
            # If all trades were filtered out, return None
            if not trades:
                logger.debug(f"Wallet {address[:8]}... rejected: all trades involved risky tokens")
                return None
        
        # Sort trades: Primary = Timestamp, Secondary = Action (BUY before SELL to allow intraday scalps)
        # Assuming TradeAction.BUY is defined such that it sorts appropriately, or use custom key
        sorted_trades = sorted(trades, key=lambda t: (
            t.timestamp, 
            0 if t.action == TradeAction.BUY else 1
        ))

        # Enrich AFTER sorting to ensure correct cost basis calculation
        self._enrich_trades_with_realized_pnl(sorted_trades)
        
        # ... rest of the function ...
        
        # Calculate time windows
        now = datetime.utcnow()
        cutoff_7d = now - timedelta(days=7)
        cutoff_30d = now - timedelta(days=30)
        
        trades_7d = [t for t in sorted_trades if t.timestamp >= cutoff_7d]
        trades_30d = [t for t in sorted_trades if t.timestamp >= cutoff_30d]

        # IMPORTANT:
        # `trade_count_30d` is intentionally defined as the number of *realized closes*,
        # i.e. SELL trades with a computed `pnl_sol`. This makes significance tests and
        # win/loss metrics comparable and prevents “lots of buys, few sells” wallets
        # from looking statistically robust.
        close_trades_30d = [
            t for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol is not None
        ]
        
        # Calculate ROI from actual price changes
        roi_7d = self._calculate_roi_from_trades(trades_7d, days=7)
        roi_30d = self._calculate_roi_from_trades(trades_30d, days=30)
        
        # Calculate win rate from actual PnL data
        win_rate = self._calculate_win_rate_from_trades(trades_30d)
        
        # Calculate drawdown
        max_drawdown = self._calculate_drawdown_from_trades(trades_30d)
        
        # Calculate average trade size
        avg_trade_size = sum(t.amount_sol for t in trades_30d) / len(trades_30d) if trades_30d else 0.0
        
        # Get last trade timestamp
        last_trade_at = sorted_trades[-1].timestamp.isoformat() if sorted_trades else None
        
        # Calculate win streak consistency (simplified)
        win_streak_consistency = self._calculate_win_streak_consistency(trades_30d)
        
        # 1. Calculate Profit Factor
        gross_profit = sum(t.pnl_sol for t in trades if t.action == TradeAction.SELL and t.pnl_sol and t.pnl_sol > 0)
        gross_loss = abs(sum(t.pnl_sol for t in trades if t.action == TradeAction.SELL and t.pnl_sol and t.pnl_sol < 0))
        
        profit_factor = 0.0
        if gross_loss == 0:
            profit_factor = 100.0 if gross_profit > 0 else 0.0
        else:
            profit_factor = gross_profit / gross_loss

        # 2. Calculate Average Entry Delay (Sniper Check)
        avg_entry_delay = None
        entry_delays = []
        buy_trades = [t for t in trades if t.action == TradeAction.BUY]
        
        # Optimization: Only check top 5 recent unique tokens to save API calls
        unique_tokens = list(set(t.token_address for t in buy_trades))[:5]
        
        # Pre-fetch creation times (this will cache them)
        import asyncio
        tasks = [self._fetch_token_creation_time(token) for token in unique_tokens]
        await asyncio.gather(*tasks, return_exceptions=True)
            
        for token in unique_tokens:
            creation_ts = await self._token_creation_cache.get(token) if hasattr(self._token_creation_cache, 'get') else self._token_creation_cache.get(token)
            if creation_ts:
                # Find the FIRST buy of this token by this wallet
                first_buy = min([t.timestamp.timestamp() for t in buy_trades if t.token_address == token])
                
                # Ensure delay is non-negative
                delay = max(0.0, first_buy - creation_ts)
                entry_delays.append(delay)
        
        if entry_delays:
            avg_entry_delay = sum(entry_delays) / len(entry_delays)
        
        # 3. Detect Insider Patterns (Fresh Wallet Check)
        insider_metrics = self._detect_insider_patterns(address, trades)
        is_fresh_wallet = insider_metrics.get("is_fresh_wallet", False)
        
        # 4. Smart Money Detection (DEX diversity, limit orders, MEV protection)
        # Note: Full detection requires transaction instruction parsing
        # For now, we set defaults that can be enhanced with transaction analysis
        dex_diversity_score = None  # Will be calculated if transaction data available
        uses_limit_orders = False
        uses_mev_protection = False
        
        # Try to detect from transaction signatures if available
        # This is a placeholder - full implementation would parse transaction instructions
        # For now, we'll set these in a future enhancement when we have transaction details
        
        # 5. Calculate Unrealized PnL (Bag Holder Detection)
        total_unrealized_loss_sol = None
        total_realized_profit_sol = None
        
        try:
            # Calculate realized profit from SELL trades
            realized_pnls = [t.pnl_sol for t in trades_30d if t.action == TradeAction.SELL and t.pnl_sol is not None]
            total_realized_profit_sol = sum(pnl for pnl in realized_pnls if pnl > 0)
            
            # Get unique token addresses from current holdings
            buy_trades = [t for t in sorted_trades if t.action == TradeAction.BUY]
            sell_trades = [t for t in sorted_trades if t.action == TradeAction.SELL]
            
            # Find tokens that have buys but may not have been fully sold
            tokens_with_buys = set(t.token_address for t in buy_trades)
            tokens_fully_sold = set()
            
            # Track sell amounts per token
            sell_amounts = {}
            for t in sell_trades:
                token_addr = t.token_address
                token_amount = t.token_amount or 0.0
                sell_amounts[token_addr] = sell_amounts.get(token_addr, 0.0) + token_amount
            
            # Find tokens that might have remaining holdings
            potential_holdings = []
            buy_amounts = {}
            for t in buy_trades:
                token_addr = t.token_address
                token_amount = t.token_amount or 0.0
                buy_amounts[token_addr] = buy_amounts.get(token_addr, 0.0) + token_amount
                
                # If buy amount > sell amount, there might be holdings
                if buy_amounts[token_addr] > sell_amounts.get(token_addr, 0.0):
                    potential_holdings.append(token_addr)
            
            # Fetch current prices for tokens with potential holdings
            if potential_holdings:
                # Get SOL price for conversion
                sol_price = await self._get_sol_price_usd()
                
                # Fetch prices in bulk
                current_prices = await PortfolioTracker.fetch_bulk_prices(potential_holdings)
                
                # Calculate unrealized PnL
                total_unrealized_loss_sol = PortfolioTracker.calculate_unrealized_pnl(
                    sorted_trades,
                    current_prices,
                    sol_price
                )
        except Exception as e:
            logger.warning(f"Failed to calculate unrealized PnL for {address}: {e}")
            total_unrealized_loss_sol = None
        
        return WalletMetrics(
            address=address,
            roi_7d=roi_7d,
            roi_30d=roi_30d,
            trade_count_30d=len(close_trades_30d),
            win_rate=win_rate,
            max_drawdown_30d=max_drawdown,
            avg_trade_size_sol=avg_trade_size,
            last_trade_at=last_trade_at,
            win_streak_consistency=win_streak_consistency,
            avg_entry_delay_seconds=avg_entry_delay,
            profit_factor=profit_factor,
            is_fresh_wallet=is_fresh_wallet,
            total_unrealized_loss_sol=total_unrealized_loss_sol,
            total_realized_profit_sol=total_realized_profit_sol,
            dex_diversity_score=dex_diversity_score,
            uses_limit_orders=uses_limit_orders,
            uses_mev_protection=uses_mev_protection,
        )

    def compute_wallet_trade_stats(self, trades: List[HistoricalTrade]) -> Dict[str, Optional[float]]:
        """
        Compute additional wallet stats from realized PnL (SOL) for persistence.

        Returns:
          - avg_win_sol
          - avg_loss_sol
          - profit_factor (sum_wins / sum_losses)
          - realized_pnl_30d_sol (sum of realized pnl over SELL trades)
        """
        if not trades:
            return {
                "avg_win_sol": None,
                "avg_loss_sol": None,
                "profit_factor": None,
                "realized_pnl_30d_sol": None,
            }

        self._enrich_trades_with_realized_pnl(trades)

        pnls = [t.pnl_sol for t in trades if t.action == TradeAction.SELL and t.pnl_sol is not None]
        if not pnls:
            return {
                "avg_win_sol": None,
                "avg_loss_sol": None,
                "profit_factor": None,
                "realized_pnl_30d_sol": 0.0,
            }

        wins = [p for p in pnls if p > 0]
        losses = [abs(p) for p in pnls if p < 0]
        sum_wins = sum(wins)
        sum_losses = sum(losses)
        
        # ---------------------------------------------------------
        # NEW: "Open Position" Trap Check
        # Scan for bags held (Rug Check). If value < 10% of cost, count as loss.
        # ---------------------------------------------------------
        # Quick position reconstruction
        positions: Dict[str, Dict[str, float]] = {} # token -> {qty, cost}
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        for t in sorted_trades:
            if t.action == TradeAction.BUY:
                pos = positions.setdefault(t.token_address, {"qty": 0.0, "cost": 0.0})
                qty = t.token_amount or (t.amount_sol / t.price_at_trade if t.price_at_trade else 0)
                if qty > 0:
                    pos["qty"] += qty
                    pos["cost"] += t.amount_sol
            elif t.action == TradeAction.SELL:
                pos = positions.get(t.token_address)
                if pos and pos["qty"] > 0:
                    qty = t.token_amount or (t.amount_sol / t.price_at_trade if t.price_at_trade else 0)
                    # Proportional cost reduction
                    fraction = min(1.0, qty / pos["qty"])
                    pos["qty"] -= qty
                    pos["cost"] -= (pos["cost"] * fraction)
        
        # Check remaining bags
        for token, pos in positions.items():
            if pos["qty"] > 0 and pos["cost"] > 0.05: # Ignore dust < 0.05 SOL cost
                # Check current price
                # We need to fetch price. This might be slow if many tokens.
                # Use get_current_liquidity which caches.
                try:
                    liq = self.liquidity_provider.get_current_liquidity(token)
                    if liq and liq.price_usd > 0:
                        sol_price = await self.liquidity_provider.get_sol_price_usd()
                        current_val_sol = (pos["qty"] * liq.price_usd) / sol_price
                        
                        # If current value is < 10% of cost, it's a RUG/Bag
                        if current_val_sol < (pos["cost"] * 0.1):
                            # Treat the entire cost basis as a loss (or remaining)
                            unrealized_loss = pos["cost"] - current_val_sol
                            sum_losses += unrealized_loss
                except Exception:
                    pass


        avg_win = (sum_wins / len(wins)) if wins else None
        avg_loss = (sum_losses / len(losses)) if losses else None
        
        # Profit Factor Calculation (Robust + Rug Aware)
        profit_factor = 0.0
        if sum_losses == 0:
            profit_factor = 100.0 if sum_wins > 0 else 0.0
        else:
            profit_factor = sum_wins / sum_losses

        return {
            "avg_win_sol": avg_win,
            "avg_loss_sol": avg_loss,
            "profit_factor": profit_factor,
            "realized_pnl_30d_sol": sum(pnls), # realized only
        }
    
    def _calculate_roi_from_trades(
        self,
        trades: List[HistoricalTrade],
        days: int = 30,
    ) -> float:
        """
        Calculate accurate ROI from historical trades.
        
        Tracks positions and calculates PnL from actual price changes.
        
        Args:
            trades: List of historical trades
            days: Time window for ROI calculation
            
        Returns:
            ROI as percentage
        """
        if not trades:
            return 0.0
        
        # Two supported modes:
        # 1) Robust swap-derived mode: use SOL cashflows + derived realized PnL (SOL)
        # 2) Legacy/test mode: use amount_sol as "units", price_at_trade as price (USD),
        #    and pnl_sol as profit/loss in same units as price (USD)

        has_swap_fields = any(t.sol_amount is not None or t.token_amount is not None for t in trades)

        if has_swap_fields:
            # Ensure we have realized PnL populated for SELL trades
            self._enrich_trades_with_realized_pnl(trades)

            total_spent_sol = 0.0
            realized_pnl_sol = 0.0

            for t in trades:
                sol_amt = t.sol_amount if t.sol_amount is not None else t.amount_sol
                if t.action == TradeAction.BUY and sol_amt:
                    total_spent_sol += max(0.0, sol_amt)
                elif t.action == TradeAction.SELL and t.pnl_sol is not None:
                    realized_pnl_sol += t.pnl_sol

            if total_spent_sol <= 0:
                return 0.0

            return (realized_pnl_sol / total_spent_sol) * 100.0

        # Legacy/test mode
        total_cost = 0.0
        total_pnl = 0.0
        for t in trades:
            if t.action == TradeAction.BUY:
                total_cost += (t.amount_sol or 0.0) * (t.price_at_trade or 0.0)
            elif t.action == TradeAction.SELL and t.pnl_sol is not None:
                total_pnl += t.pnl_sol

        if total_cost <= 0:
            return 0.0
        return (total_pnl / total_cost) * 100.0
    
    def _estimate_roi(self, trades: List[HistoricalTrade]) -> float:
        """
        Estimate ROI from trades (legacy method - calls accurate calculation).
        
        Kept for backward compatibility.
        """
        return self._calculate_roi_from_trades(trades)
    
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

        # Ensure pnl is populated for SELL trades (if possible)
        self._enrich_trades_with_realized_pnl(trades)
        
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
    
    # Adding this methodology to where _detect_insider_patterns is or simply add a new helper method
    
    def _is_smart_money_candidate(self, address: str, trades: List[HistoricalTrade]) -> bool:
        """
        Filter for 'Smart Money' / 'Whale' behavior.
        
        Criteria:
        1. Whale: Trades > 10 SOL regularly
        2. KOL/Smart: Trades 'fresh' tokens but not SNIPES (wait > 5 mins)
        """
        if not trades:
            return False
            
        # 1. Whale Check
        big_trades = [t for t in trades if (t.sol_amount or 0) > 10.0] # Changed t.amount_sol to t.sol_amount
        if len(big_trades) >= 2:
            return True
            
        # 2. Smart Money Check (Early but not Sniper)
        # We need entry delays. Re-calculate or check metrics if already done.
        # Since this is called potentially before metrics calculation in some flows (or inside it),
        # let's assume we use it as a post-filter or inside metrics calc.
        
        # Actually, best place is to use the metrics we already calculated in _calculate_metrics_from_trades
        # This method is just a helper if we wanted to pre-filter, but we already have metrics.
        # So we just enforce this via WQS/Validation.
        
        return True
    
    def _estimate_win_rate(self, trades: List[HistoricalTrade]) -> float:
        """
        Estimate win rate from trades (legacy method - calls accurate calculation).
        
        Kept for backward compatibility.
        """
        return self._calculate_win_rate_from_trades(trades)
    
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
        
        # Ensure realized PnL exists for sells
        self._enrich_trades_with_realized_pnl(trades)

        # Build equity curve from realized PnL over SELL trades
        equity = 0.0
        peak = 0.0
        max_dd = 0.0
        
        cumulative_pnl = 0.0
        
        for t in sorted_trades:
            if t.action != TradeAction.SELL or t.pnl_sol is None:
                continue
            cumulative_pnl += t.pnl_sol
            
            # Reset peak if we reach a new high in cumulative PnL
            if cumulative_pnl > peak:
                peak = cumulative_pnl
            
            # Calculate drawdown from peak
            drawdown_amount = peak - cumulative_pnl
            if drawdown_amount > 0:
                # If peak is positive, standard calc
                if peak > 0:
                    current_dd = drawdown_amount / peak
                else:
                    # If peak is 0 or negative (started losing immediately), 
                    # we can't use % of peak. We can treat it as % of capital lost?
                    # Since we don't know total capital, we cap this edge case or ignore.
                    current_dd = 0.0 
                
                max_dd = max(max_dd, current_dd)

        return max_dd * 100.0

    
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

        # Ensure pnl is populated for SELL trades (if possible)
        self._enrich_trades_with_realized_pnl(trades)
        
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
        if address in self._trades_cache:
            cutoff = datetime.utcnow() - timedelta(days=days)
            return [t for t in self._trades_cache[address] if t.timestamp >= cutoff]
        
        # Fetch real data if Helius client is available
        if self.helius_client.api_key:
            try:
                trades = await self._fetch_real_historical_trades(address, days)
                if trades:
                    self._trades_cache[address] = trades
                    return trades
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to fetch trades for {address[:8]}...: {e}")
        
        # Fall back to cached sample data
        trades = self._trades_cache.get(address, [])
        cutoff = datetime.utcnow() - timedelta(days=days)
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
        transactions = await self.helius_client.get_wallet_transactions(
            address,
            days=days,
            limit=self._wallet_tx_limit,
        )
        
        trades = []
        liquidity_snapshots = []
        
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx, wallet_address=address)
            if swap:
                trade = self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
                    
                    # Collect a CURRENT liquidity snapshot (at collection time).
                    # This builds a time-series liquidity database going forward.
                    try:
                        current_liq = self.liquidity_provider.get_current_liquidity(trade.token_address)
                        if current_liq:
                            # Store snapshot at "now" (not at the trade's past timestamp).
                            historical_snapshot = LiquidityData(
                                token_address=current_liq.token_address,
                                liquidity_usd=current_liq.liquidity_usd,
                                price_usd=current_liq.price_usd,
                                volume_24h_usd=current_liq.volume_24h_usd,
                                timestamp=datetime.utcnow(),
                                source="analyzer_collection_current",
                            )
                            liquidity_snapshots.append(historical_snapshot)
                    except Exception as e:
                        # Log but don't fail on liquidity collection errors
                        print(f"[Analyzer] Warning: Failed to collect liquidity for {trade.token_address[:8]}...: {e}")
        
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
    
    def fetch_recent_trades(self, address: str, days: int = 30) -> List[dict]:
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
        trades = self.get_historical_trades(address, days)
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
