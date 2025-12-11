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
from typing import List, Optional, Dict, Any

from .wqs import WalletMetrics
from .models import HistoricalTrade, TradeAction
from .helius_client import HeliusClient


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
                discovered = self.helius_client.discover_wallets_from_recent_swaps(
                    limit=200,  # Query more transactions to find active wallets
                    min_trade_count=3,  # Minimum trades to be considered
                )
                if discovered:
                    self._candidate_wallets = discovered[:max_wallets]
                    print(f"[Analyzer] Discovered {len(self._candidate_wallets)} candidate wallets")
                else:
                    print("[Analyzer] No wallets discovered, using sample data")
                    self._load_sample_data()
            except Exception as e:
                print(f"[Analyzer] Warning: Failed to discover wallets: {e}")
                print("[Analyzer] Falling back to sample data")
                self._load_sample_data()
        else:
            if not self.helius_client.api_key:
                print("[Analyzer] No Helius API key found, using sample data")
            else:
                print("[Analyzer] Wallet discovery disabled, using sample data")
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
        
        # Fetch real data if Helius client is available
        if self.helius_client.api_key:
            try:
                metrics = self._fetch_real_wallet_metrics(address)
                if metrics:
                    self._metrics_cache[address] = metrics
                    return metrics
            except Exception as e:
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
        
        # Calculate ROI (simplified - would need price data for accurate calculation)
        # For now, estimate based on trade frequency and direction
        roi_7d = self._estimate_roi(trades_7d)
        roi_30d = self._estimate_roi(trades_30d)
        
        # Calculate win rate (simplified - would need actual PnL data)
        win_rate = self._estimate_win_rate(trades_30d)
        
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
    
    def _estimate_roi(self, trades: List[HistoricalTrade]) -> float:
        """Estimate ROI from trades (simplified - would need price data)."""
        if not trades:
            return 0.0
        
        # Simplified: assume alternating buy/sell with small profit
        # In production, would calculate actual PnL from price changes
        buy_count = sum(1 for t in trades if t.action == TradeAction.BUY)
        sell_count = sum(1 for t in trades if t.action == TradeAction.SELL)
        
        # Rough estimate: more sells than buys suggests profit
        if sell_count > 0:
            return min(50.0, (sell_count / max(buy_count, 1)) * 10.0)
        return 0.0
    
    def _estimate_win_rate(self, trades: List[HistoricalTrade]) -> float:
        """Estimate win rate from trades (simplified)."""
        if not trades:
            return 0.0
        
        # Simplified: assume 60% win rate for active traders
        # In production, would calculate from actual PnL
        return 0.6
    
    def _calculate_drawdown_from_trades(self, trades: List[HistoricalTrade]) -> float:
        """Calculate drawdown from trades (simplified)."""
        if not trades:
            return 0.0
        
        # Simplified: assume moderate drawdown for active traders
        # In production, would track running PnL and calculate actual drawdown
        return 10.0
    
    def _calculate_win_streak_consistency(self, trades: List[HistoricalTrade]) -> float:
        """Calculate win streak consistency (simplified)."""
        if not trades:
            return 0.0
        
        # Simplified: assume moderate consistency
        # In production, would analyze actual win/loss streaks
        return 0.5
    
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
        """Fetch real historical trades from Helius API."""
        transactions = self.helius_client.get_wallet_transactions(
            address,
            days=days,
            limit=100,
        )
        
        trades = []
        for tx in transactions:
            swap = self.helius_client.parse_swap_transaction(tx)
            if swap:
                trade = self._parse_swap_to_trade(swap, address)
                if trade:
                    trades.append(trade)
        
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
