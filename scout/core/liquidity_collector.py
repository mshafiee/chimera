"""Service to collect and store current liquidity data periodically."""

import os
import sqlite3
from datetime import datetime, timedelta
from typing import Optional
from core.birdeye_client import BirdeyeClient
from core.liquidity import LiquidityProvider


class LiquidityCollector:
    """Collects current liquidity data and stores it in the database."""

    def __init__(
        self,
        db_path: str,
        birdeye_client: Optional[BirdeyeClient] = None,
        liquidity_provider: Optional[LiquidityProvider] = None,
    ):
        """
        Initialize liquidity collector.

        Args:
            db_path: Path to SQLite database
            birdeye_client: Birdeye API client (optional)
            liquidity_provider: Liquidity provider for fetching current data (optional)
        """
        self.db_path = db_path
        self.birdeye_client = birdeye_client or BirdeyeClient()
        self.liquidity_provider = liquidity_provider or LiquidityProvider()

    def collect_current_liquidity(self, token_address: str) -> bool:
        """
        Collect current liquidity for a token and store in database.

        Args:
            token_address: Token mint address

        Returns:
            True if successfully collected and stored
        """
        # Try to get current liquidity
        liquidity_data = None

        # Try Birdeye first
        if self.birdeye_client and self.birdeye_client.api_key:
            liquidity_data = self.birdeye_client.get_current_liquidity(token_address)

        # Fallback to liquidity provider
        if not liquidity_data:
            liquidity_data = self.liquidity_provider.get_current_liquidity(token_address)

        if not liquidity_data:
            return False

        # Store in database
        try:
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            cursor.execute(
                """
                INSERT OR REPLACE INTO historical_liquidity 
                (token_address, liquidity_usd, price_usd, volume_24h_usd, timestamp, source)
                VALUES (?, ?, ?, ?, ?, ?)
                """,
                (
                    token_address,
                    liquidity_data.liquidity_usd,
                    liquidity_data.price_usd,
                    liquidity_data.volume_24h_usd,
                    liquidity_data.timestamp,
                    liquidity_data.source,
                ),
            )

            conn.commit()
            conn.close()
            return True
        except sqlite3.Error as e:
            print(f"Failed to store liquidity data: {e}")
            return False

    def collect_for_tracked_tokens(self, token_addresses: list[str]) -> int:
        """
        Collect liquidity for multiple tokens.

        Args:
            token_addresses: List of token addresses to collect

        Returns:
            Number of successfully collected tokens
        """
        success_count = 0
        for token_address in token_addresses:
            if self.collect_current_liquidity(token_address):
                success_count += 1
        return success_count

    def cleanup_old_data(self, retention_days: int = 90) -> int:
        """
        Remove historical liquidity data older than retention period.

        Args:
            retention_days: Number of days to retain data

        Returns:
            Number of rows deleted
        """
        try:
            conn = sqlite3.connect(self.db_path)
            cursor = conn.cursor()

            cutoff_date = datetime.utcnow() - timedelta(days=retention_days)
            cursor.execute(
                "DELETE FROM historical_liquidity WHERE timestamp < ?",
                (cutoff_date,),
            )

            deleted_count = cursor.rowcount
            conn.commit()
            conn.close()

            return deleted_count
        except sqlite3.Error as e:
            print(f"Failed to cleanup old data: {e}")
            return 0

