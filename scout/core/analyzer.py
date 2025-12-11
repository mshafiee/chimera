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
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional, Dict, Any

from .wqs import WalletMetrics
from .models import HistoricalTrade, TradeAction, LiquidityData
from .helius_client import HeliusClient
from .liquidity import LiquidityProvider


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
        
        # Cache for metrics and trades
        self._metrics_cache: Dict[str, WalletMetrics] = {}
        self._trades_cache: Dict[str, List[HistoricalTrade]] = {}
        self._candidate_wallets: List[str] = []
        
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
    
    def get_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
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
                metrics = self._fetch_real_wallet_metrics(address)
                if metrics:
                    self._metrics_cache[address] = metrics
                    return metrics
            except Exception as e:
                if os.getenv("SCOUT_VERBOSE", "false").lower() == "true":
                    print(f"[Analyzer] Warning: Failed to fetch metrics for {address[:8]}...: {e}")
        
        # Fall back to cached sample data
        return self._metrics_cache.get(address)
    
    def _fetch_real_wallet_metrics(self, address: str) -> Optional[WalletMetrics]:
        """Fetch real wallet metrics from Helius API."""
        # Get transaction history
        transactions = self.helius_client.get_wallet_transactions(
            address,
            days=30,
            limit=100,
        )
        
        if not transactions:
            return None
        
        # Parse transactions into trades
        trades = []
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx)
            if swap:
                # Convert to HistoricalTrade format
                # Note: We need to determine token symbol and calculate PnL
                trade = self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
        
        if not trades:
            return None
        
        # Calculate metrics from trades
        return self._calculate_metrics_from_trades(address, trades)
    
    def _parse_swap_to_trade(self, swap: Dict[str, Any], wallet: str) -> Optional[HistoricalTrade]:
        """Parse a swap transaction into a HistoricalTrade."""
        try:
            # Determine action (simplified: if SOL is input, it's a BUY)
            action = TradeAction.BUY if swap.get("token_in") == "So11111111111111111111111111111111111111112" else TradeAction.SELL
            
            # Get timestamp
            timestamp = datetime.utcfromtimestamp(swap.get("timestamp", int(datetime.utcnow().timestamp())))
            
            # Calculate amount in SOL
            amount_sol = swap.get("amount_in", 0) if action == TradeAction.BUY else swap.get("amount_out", 0)
            
            # For now, we'll use placeholder values that will be refined
            # In production, you'd fetch token metadata and prices
            trade = HistoricalTrade(
                token_address=swap.get("token_out", "") if action == TradeAction.BUY else swap.get("token_in", ""),
                token_symbol="UNKNOWN",  # Would fetch from token metadata
                action=action,
                amount_sol=float(amount_sol) / 1e9 if amount_sol else 0.0,  # Convert lamports to SOL
                price_at_trade=0.0,  # Would calculate from swap amounts
                timestamp=timestamp,
                tx_signature=swap.get("signature", ""),
                pnl_sol=None,  # Would calculate from price changes
                liquidity_at_trade_usd=None,
            )
            return trade
        except Exception as e:
            print(f"[Analyzer] Error parsing swap: {e}")
            return None
    
    def _calculate_metrics_from_trades(self, address: str, trades: List[HistoricalTrade]) -> Optional[WalletMetrics]:
        """Calculate wallet metrics from historical trades."""
        if not trades:
            return None
        
        # Sort trades by timestamp
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        # Calculate time windows
        now = datetime.utcnow()
        cutoff_7d = now - timedelta(days=7)
        cutoff_30d = now - timedelta(days=30)
        
        trades_7d = [t for t in sorted_trades if t.timestamp >= cutoff_7d]
        trades_30d = [t for t in sorted_trades if t.timestamp >= cutoff_30d]
        
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
        
        return WalletMetrics(
            address=address,
            roi_7d=roi_7d,
            roi_30d=roi_30d,
            trade_count_30d=len(trades_30d),
            win_rate=win_rate,
            max_drawdown_30d=max_drawdown,
            avg_trade_size_sol=avg_trade_size,
            last_trade_at=last_trade_at,
            win_streak_consistency=win_streak_consistency,
        )
    
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
        
        # Track positions: {token_address: {entry_price, entry_amount, total_cost}}
        positions = {}
        total_capital = 0.0
        total_pnl = 0.0
        
        # Sort trades chronologically
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        for trade in sorted_trades:
            if trade.action == TradeAction.BUY:
                # Open or add to position
                if trade.token_address not in positions:
                    positions[trade.token_address] = {
                        'entry_price': trade.price_at_trade,
                        'entry_amount': trade.amount_sol,
                        'total_cost': trade.amount_sol * trade.price_at_trade,
                    }
                else:
                    # Average entry price (weighted by amount)
                    pos = positions[trade.token_address]
                    additional_cost = trade.amount_sol * trade.price_at_trade
                    total_cost = pos['total_cost'] + additional_cost
                    total_amount = pos['entry_amount'] + trade.amount_sol
                    pos['entry_price'] = total_cost / total_amount if total_amount > 0 else trade.price_at_trade
                    pos['entry_amount'] = total_amount
                    pos['total_cost'] = total_cost
                
                total_capital += trade.amount_sol * trade.price_at_trade
                
            elif trade.action == TradeAction.SELL:
                # Close position and calculate PnL
                if trade.token_address in positions:
                    pos = positions[trade.token_address]
                    entry_price = pos['entry_price']
                    exit_price = trade.price_at_trade
                    
                    # Calculate PnL for the amount sold
                    sell_amount = min(trade.amount_sol, pos['entry_amount'])
                    if sell_amount > 0 and entry_price > 0:
                        pnl = (exit_price - entry_price) * sell_amount
                        total_pnl += pnl
                    
                    # Update position
                    pos['entry_amount'] -= sell_amount
                    if pos['entry_amount'] <= 0:
                        del positions[trade.token_address]
                else:
                    # Selling without a position - use trade PnL if available
                    if trade.pnl_sol is not None:
                        total_pnl += trade.pnl_sol
        
        # Calculate ROI
        if total_capital <= 0:
            # If no capital deployed, try to estimate from PnL data
            total_pnl_from_trades = sum(t.pnl_sol or 0.0 for t in sorted_trades if t.pnl_sol is not None)
            if total_pnl_from_trades != 0:
                # Estimate capital from trade amounts
                estimated_capital = sum(t.amount_sol * (t.price_at_trade or 1.0) for t in sorted_trades if t.action == TradeAction.BUY)
                if estimated_capital > 0:
                    return (total_pnl_from_trades / estimated_capital) * 100
            return 0.0
        
        roi_percent = (total_pnl / total_capital) * 100
        return roi_percent
    
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
        
        # Track running PnL
        running_pnl = 0.0
        peak_pnl = 0.0
        max_drawdown = 0.0
        
        for trade in sorted_trades:
            # Update running PnL
            if trade.pnl_sol is not None:
                running_pnl += trade.pnl_sol
            else:
                # If no PnL data, estimate from price changes for SELL trades
                if trade.action == TradeAction.SELL:
                    # Try to find corresponding BUY trade
                    buy_trades = [t for t in sorted_trades 
                                if t.token_address == trade.token_address 
                                and t.action == TradeAction.BUY 
                                and t.timestamp < trade.timestamp]
                    if buy_trades:
                        # Use most recent buy price
                        last_buy = buy_trades[-1]
                        if last_buy.price_at_trade > 0 and trade.price_at_trade > 0:
                            estimated_pnl = (trade.price_at_trade - last_buy.price_at_trade) * trade.amount_sol
                            running_pnl += estimated_pnl
            
            # Update peak
            if running_pnl > peak_pnl:
                peak_pnl = running_pnl
            
            # Calculate drawdown from peak
            if peak_pnl > 0:
                drawdown = (peak_pnl - running_pnl) / peak_pnl
                max_drawdown = max(max_drawdown, drawdown)
            elif peak_pnl < 0 and running_pnl < peak_pnl:
                # Handle negative PnL case
                drawdown = abs((running_pnl - peak_pnl) / abs(peak_pnl)) if peak_pnl != 0 else 0.0
                max_drawdown = max(max_drawdown, drawdown)
        
        return max_drawdown * 100  # Convert to percentage
    
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
        
        # Determine wins/losses
        outcomes = [1 if t.pnl_sol > 0 else 0 for t in closing_trades]
        
        # Calculate streak consistency
        # Method: Analyze streak lengths and calculate variance
        current_streak = 1
        streaks = []
        
        for i in range(1, len(outcomes)):
            if outcomes[i] == outcomes[i-1]:
                current_streak += 1
            else:
                streaks.append(current_streak)
                current_streak = 1
        streaks.append(current_streak)
        
        if not streaks or len(streaks) < 2:
            return 0.0
        
        # Calculate variance of streak lengths
        # Lower variance = more consistent
        avg_streak = sum(streaks) / len(streaks)
        variance = sum((s - avg_streak) ** 2 for s in streaks) / len(streaks)
        
        # Normalize to 0-1 range (inverse relationship)
        # Lower variance = higher consistency
        max_variance = len(outcomes)  # Theoretical maximum
        consistency = 1.0 - min(variance / max_variance, 1.0)
        
        # Also factor in win rate - higher win rate = higher consistency
        win_rate = sum(outcomes) / len(outcomes)
        consistency = (consistency * 0.7) + (win_rate * 0.3)  # Weighted combination
        
        return max(0.0, min(consistency, 1.0))  # Clamp to 0-1
    
    def get_historical_trades(
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
                trades = self._fetch_real_historical_trades(address, days)
                if trades:
                    self._trades_cache[address] = trades
                    return trades
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to fetch trades for {address[:8]}...: {e}")
        
        # Fall back to cached sample data
        trades = self._trades_cache.get(address, [])
        cutoff = datetime.utcnow() - timedelta(days=days)
        return [t for t in trades if t.timestamp >= cutoff]
    
    def _fetch_real_historical_trades(self, address: str, days: int) -> List[HistoricalTrade]:
        """
        Fetch real historical trades from Helius API.
        
        Also collects liquidity snapshots for each trade to build historical
        liquidity database for future backtesting.
        """
        transactions = self.helius_client.get_wallet_transactions(
            address,
            days=days,
            limit=100,
        )
        
        trades = []
        liquidity_snapshots = []
        
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx)
            if swap:
                trade = self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
                    
                    # Collect liquidity snapshot at trade time
                    # This builds the historical liquidity database
                    try:
                        current_liq = self.liquidity_provider.get_current_liquidity(trade.token_address)
                        if current_liq:
                            # Create historical snapshot with trade timestamp
                            historical_snapshot = LiquidityData(
                                token_address=current_liq.token_address,
                                liquidity_usd=current_liq.liquidity_usd,
                                price_usd=current_liq.price_usd,
                                volume_24h_usd=current_liq.volume_24h_usd,
                                timestamp=trade.timestamp,  # Use trade timestamp
                                source="analyzer_collection",
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
    
    def calculate_roi(self, trades: List[dict]) -> float:
        """
        Calculate ROI from a list of trades.
        
        Args:
            trades: List of trade dictionaries
            
        Returns:
            ROI as percentage
        """
        if not trades:
            return 0.0
        
        total_pnl = sum(t.get("pnl_sol", 0) or 0 for t in trades)
        total_cost = sum(t.get("amount_sol", 0) for t in trades if t.get("action") == "BUY")
        
        if total_cost <= 0:
            return 0.0
        
        return (total_pnl / total_cost) * 100
    
    def calculate_drawdown(self, trades: List[dict]) -> float:
        """
        Calculate maximum drawdown from a list of trades.
        
        Args:
            trades: List of trade dictionaries
            
        Returns:
            Maximum drawdown as percentage
        """
        if not trades:
            return 0.0
        
        # Sort by timestamp
        sorted_trades = sorted(trades, key=lambda t: t.get("timestamp", ""))
        
        peak = 0.0
        max_drawdown = 0.0
        running_pnl = 0.0
        
        for trade in sorted_trades:
            pnl = trade.get("pnl_sol", 0) or 0
            running_pnl += pnl
            peak = max(peak, running_pnl)
            
            if peak > 0:
                drawdown = (peak - running_pnl) / peak
                max_drawdown = max(max_drawdown, drawdown)
        
        return max_drawdown * 100


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
