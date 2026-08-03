"""Tests for historical liquidity functionality."""

import sys
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

import pytest
from datetime import datetime, timedelta
from core.liquidity import LiquidityProvider
from core.models import LiquidityData


class FakeHistoricalLiquidityDB:
    """In-memory stand-in for core.db.get_connection (PostgreSQL-only backend).

    Emulates exactly the statements LiquidityProvider issues against the
    historical_liquidity table so tests are isolated from the real database.
    """

    def __init__(self):
        self.rows = []

    def cursor(self):
        return FakeCursor(self)

    def commit(self):
        pass

    def close(self):
        pass


class FakeCursor:
    def __init__(self, db):
        self.db = db
        self._last = None

    @staticmethod
    def _parse(ts):
        if isinstance(ts, str):
            return datetime.fromisoformat(ts.replace('Z', '+00:00'))
        return ts

    def execute(self, sql, params=None):
        sql = " ".join(sql.split())
        self._last = None
        if sql.startswith("CREATE TABLE"):
            return
        if sql.startswith("INSERT INTO historical_liquidity"):
            token, liq, price, vol, ts, source = params
            self.db.rows.append({
                "token_address": token,
                "liquidity_usd": liq,
                "price_usd": price,
                "volume_24h_usd": vol,
                "timestamp": ts,
                "source": source,
            })
            return
        if sql.startswith("SELECT liquidity_usd"):
            token, t_start, t_end, t_query = params
            t_start = self._parse(t_start)
            t_end = self._parse(t_end)
            t_query = self._parse(t_query)
            best = None
            for r in self.db.rows:
                if r["token_address"] != token:
                    continue
                rts = self._parse(r["timestamp"])
                if not (t_start <= rts <= t_end):
                    continue
                if best is None or abs(rts - t_query) < best[0]:
                    best = (abs(rts - t_query), r)
            self._last = best[1] if best else None

    def fetchone(self):
        return self._last

    def fetchall(self):
        return []


class TestHistoricalLiquidity:
    """Test historical liquidity lookup and storage."""

    @pytest.fixture
    def provider(self, monkeypatch):
        """Create LiquidityProvider backed by an in-memory fake DB."""
        fake_db = FakeHistoricalLiquidityDB()

        def fake_get_connection(db_path=None, force_sqlite=False):
            return fake_db

        from core import db as core_db
        monkeypatch.setattr(core_db, "get_connection", fake_get_connection)
        return LiquidityProvider(db_path=":memory:")

    def test_get_historical_liquidity_exact_match(self, provider):
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

    def test_get_historical_liquidity_within_tolerance(self, provider):
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

    def test_get_historical_liquidity_outside_tolerance(self, provider):
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

    def test_get_historical_liquidity_fallback_to_current(self, provider, monkeypatch):
        """Test fallback to current liquidity when historical unavailable."""
        # Disable strict mode so the fallback path is exercised
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "false")
        token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        timestamp = datetime.utcnow() - timedelta(days=30)

        # Mock get_current_liquidity with a DISTINCT timestamp/source so the
        # fallback's adjustments are observable (not trivially true)
        simulated = LiquidityData(
            token_address=token,
            liquidity_usd=30000.0,
            price_usd=0.001,
            volume_24h_usd=5000.0,
            timestamp=datetime.utcnow(),  # current time, not the query timestamp
            source="live_api",
        )
        monkeypatch.setattr(provider, "get_current_liquidity", lambda addr: simulated)

        # No historical data stored, should fallback to current
        result = provider.get_historical_liquidity_or_current(token, timestamp)

        assert result is not None
        assert result.token_address == token
        # The fallback rewrites the source and stamps the query timestamp
        assert result.timestamp == timestamp
        assert result.source != simulated.source
        assert "confidence_weighted" in result.source

    def test_store_liquidity_batch(self, provider):
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

    def test_get_historical_liquidity_or_current_with_historical(self, provider):
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
        # Historical data gets confidence suffix added
        assert result.source == "test_historical_confidence_1.0"
        assert "_fallback" not in result.source


class TestLiquidityProviderIntegration:
    """Integration tests for LiquidityProvider with database."""

    def test_historical_liquidity_workflow(self, monkeypatch):
        """Test complete workflow of storing and retrieving historical liquidity."""
        fake_db = FakeHistoricalLiquidityDB()

        def fake_get_connection(db_path=None, force_sqlite=False):
            return fake_db

        from core import db as core_db
        monkeypatch.setattr(core_db, "get_connection", fake_get_connection)
        provider = LiquidityProvider(db_path=":memory:")

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






