"""Tests for historical liquidity functionality."""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
import tempfile
import os
import sqlite3
from datetime import datetime, timedelta
from core.liquidity import LiquidityProvider
from core.models import LiquidityData


class TestHistoricalLiquidity:
    """Test historical liquidity lookup and storage."""
    
    @pytest.fixture
    def temp_db(self):
        """Create a temporary database for testing."""
        fd, path = tempfile.mkstemp(suffix='.db')
        os.close(fd)
        
        # Create table
        conn = sqlite3.connect(path)
        cursor = conn.cursor()
        cursor.execute("""
            CREATE TABLE IF NOT EXISTS historical_liquidity (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                liquidity_usd REAL NOT NULL,
                price_usd REAL,
                volume_24h_usd REAL,
                timestamp TIMESTAMP NOT NULL,
                source TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(token_address, timestamp)
            )
        """)
        conn.commit()
        conn.close()
        
        yield path
        
        # Cleanup
        if os.path.exists(path):
            os.unlink(path)
    
    @pytest.fixture
    def provider(self, temp_db):
        """Create LiquidityProvider with temp database."""
        return LiquidityProvider(db_path=temp_db)
    
    def test_get_historical_liquidity_exact_match(self, provider, temp_db):
        """Test getting historical liquidity with exact timestamp match."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        timestamp = datetime.utcnow() - timedelta(days=5)
        
        # Store historical liquidity
        liq_data = LiquidityData(
            token_address=token,
            liquidity_usd=100000.0,
            price_usd=0.001,
            volume_24h_usd=50000.0,
            timestamp=timestamp,
            source="test",
        )
        provider._store_in_database(liq_data)
        
        # Retrieve it
        result = provider.get_historical_liquidity(token, timestamp, tolerance_hours=6)
        
        assert result is not None
        assert result.token_address == token
        assert result.liquidity_usd == 100000.0
        assert abs((result.timestamp - timestamp).total_seconds()) < 3600  # Within 1 hour
    
    def test_get_historical_liquidity_within_tolerance(self, provider, temp_db):
        """Test getting historical liquidity within tolerance."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        stored_timestamp = datetime.utcnow() - timedelta(days=5)
        query_timestamp = stored_timestamp + timedelta(hours=3)  # 3 hours later
        
        # Store historical liquidity
        liq_data = LiquidityData(
            token_address=token,
            liquidity_usd=100000.0,
            price_usd=0.001,
            volume_24h_usd=50000.0,
            timestamp=stored_timestamp,
            source="test",
        )
        provider._store_in_database(liq_data)
        
        # Retrieve it with 6-hour tolerance
        result = provider.get_historical_liquidity(token, query_timestamp, tolerance_hours=6)
        
        assert result is not None
        assert result.liquidity_usd == 100000.0
    
    def test_get_historical_liquidity_outside_tolerance(self, provider, temp_db):
        """Test that liquidity outside tolerance is not returned."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        stored_timestamp = datetime.utcnow() - timedelta(days=5)
        query_timestamp = stored_timestamp + timedelta(hours=8)  # 8 hours later (outside 6-hour tolerance)
        
        # Store historical liquidity
        liq_data = LiquidityData(
            token_address=token,
            liquidity_usd=100000.0,
            price_usd=0.001,
            volume_24h_usd=50000.0,
            timestamp=stored_timestamp,
            source="test",
        )
        provider._store_in_database(liq_data)
        
        # Should not retrieve it with 6-hour tolerance
        result = provider.get_historical_liquidity(token, query_timestamp, tolerance_hours=6)
        
        assert result is None
    
    def test_get_historical_liquidity_fallback_to_current(self, provider):
        """Test fallback to current liquidity when historical unavailable."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        timestamp = datetime.utcnow() - timedelta(days=30)
        
        # No historical data stored, should fallback to current
        result = provider.get_historical_liquidity_or_current(token, timestamp)
        
        assert result is not None
        assert result.token_address == token
        assert result.timestamp == timestamp  # Timestamp should be set to historical
        assert "_fallback" in result.source or "simulated" in result.source
    
    def test_store_liquidity_batch(self, provider, temp_db):
        """Test batch storage of liquidity data."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        
        # Create multiple liquidity snapshots
        snapshots = []
        for i in range(5):
            snapshots.append(LiquidityData(
                token_address=token,
                liquidity_usd=100000.0 + (i * 1000),
                price_usd=0.001,
                volume_24h_usd=50000.0,
                timestamp=datetime.utcnow() - timedelta(days=i),
                source="test_batch",
            ))
        
        # Store batch
        stored_count = provider.store_liquidity_batch(snapshots)
        
        assert stored_count == 5
        
        # Verify all stored
        for snapshot in snapshots:
            result = provider.get_historical_liquidity(
                snapshot.token_address,
                snapshot.timestamp,
                tolerance_hours=24
            )
            assert result is not None
            assert result.liquidity_usd == snapshot.liquidity_usd
    
    def test_get_historical_liquidity_or_current_with_historical(self, provider, temp_db):
        """Test that historical liquidity is preferred over current."""
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        timestamp = datetime.utcnow() - timedelta(days=5)
        
        # Store historical liquidity
        liq_data = LiquidityData(
            token_address=token,
            liquidity_usd=50000.0,  # Lower than current
            price_usd=0.001,
            volume_24h_usd=25000.0,
            timestamp=timestamp,
            source="test_historical",
        )
        provider._store_in_database(liq_data)
        
        # Should return historical, not current
        result = provider.get_historical_liquidity_or_current(token, timestamp)
        
        assert result is not None
        assert result.liquidity_usd == 50000.0
        assert result.source == "test_historical"
        assert "_fallback" not in result.source


class TestLiquidityProviderIntegration:
    """Integration tests for LiquidityProvider with database."""
    
    def test_historical_liquidity_workflow(self, tmp_path):
        """Test complete workflow of storing and retrieving historical liquidity."""
        db_path = str(tmp_path / "test.db")
        provider = LiquidityProvider(db_path=db_path)
        
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        timestamp = datetime.utcnow() - timedelta(days=7)
        
        # Store historical liquidity
        liq_data = LiquidityData(
            token_address=token,
            liquidity_usd=75000.0,
            price_usd=0.001,
            volume_24h_usd=37500.0,
            timestamp=timestamp,
            source="integration_test",
        )
        
        assert provider._store_in_database(liq_data) is True
        
        # Retrieve it
        result = provider.get_historical_liquidity(token, timestamp, tolerance_hours=24)
        
        assert result is not None
        assert result.liquidity_usd == 75000.0
        assert result.source == "integration_test"




