"""Coverage completion tests for core/liquidity.py (multi-source liquidity provider)."""

import concurrent.futures
import importlib
import sys
import time
from datetime import datetime, timedelta, timezone
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

import core.liquidity as liquidity_module
from core.liquidity import LiquidityProvider
from core.models import LiquidityData
from core.utils import utcnow


def make_liq_data(token="token_1", liquidity=100000.0, price=1.0, volume=50000.0,
                  ts=None, source="birdeye"):
    return LiquidityData(
        token_address=token,
        liquidity_usd=liquidity,
        price_usd=price,
        volume_24h_usd=volume,
        timestamp=ts or utcnow(),
        source=source,
    )


class FakeBirdeye:
    """Async Birdeye stand-in with a session the provider may clean up."""

    def __init__(self, current=None, historical=None, error=None):
        self._session = MagicMock()
        self._own_session = True
        self.current = current
        self.historical = historical
        self.error = error
        self.closed = False

    async def get_current_liquidity(self, token_address):
        if self.error:
            raise self.error
        return self.current

    async def get_historical_liquidity(self, token_address, timestamp):
        if self.error:
            raise self.error
        return self.historical

    async def close(self):
        self.closed = True

    async def get_sol_price_usd(self):
        return 150.0


class FakeJupiter:
    def __init__(self, current=None, sol_price=None, error=None):
        self._session = MagicMock()
        self._own_session = True
        self.current = current
        self.sol_price = sol_price
        self.error = error
        self.closed = False

    async def get_current_liquidity(self, token_address):
        if self.error:
            raise self.error
        return self.current

    async def get_sol_price_usd(self):
        if self.error:
            raise self.error
        return self.sol_price

    async def close(self):
        self.closed = True


class FakeDexScreener:
    def __init__(self, current=None, error=None):
        self.current = current
        self.error = error

    def get_current_liquidity(self, token_address):
        if self.error:
            raise self.error
        return self.current


class FakeRedis:
    def __init__(self, data=None, available=True, error=None):
        self._data = data or {}
        self.available = available
        self.error = error
        self._keys = set(self._data.keys())

    def is_available(self):
        return self.available

    def get(self, key):
        if self.error:
            raise self.error
        return self._data.get(key)

    def set(self, key, value, ttl_seconds=None):
        if self.error:
            raise self.error
        self._data[key] = value
        self._keys.add(key)

    def delete(self, key):
        self._data.pop(key, None)
        self._keys.discard(key)


def make_simulated_provider():
    return LiquidityProvider(mode="simulated")


class FakeCursor:
    def __init__(self, row=None, fail_on_execute=False, fail_after=None):
        self.row = row
        self.fail_on_execute = fail_on_execute
        self.fail_after = fail_after
        self.executed = []
        self.rowcount = 1

    def execute(self, sql, params=None):
        self.executed.append(sql)
        if self.fail_on_execute:
            raise RuntimeError("exec failed")
        if self.fail_after is not None and len(self.executed) > self.fail_after:
            raise RuntimeError("row insert failed")
        return self

    def fetchone(self):
        return self.row


class FakeConn:
    """DB connection stand-in that returns a FakeCursor."""

    def __init__(self, cursor=None, fail_on_execute=False, fail_on_cursor=False, fail_on_commit=False):
        self._cursor = cursor or FakeCursor()
        self.fail_on_execute = fail_on_execute
        self.fail_on_cursor = fail_on_cursor
        self.fail_on_commit = fail_on_commit
        self.committed = False

    def cursor(self):
        if self.fail_on_cursor:
            raise RuntimeError("cursor failed")
        return self._cursor

    def execute(self, sql, params=None):
        if self.fail_on_execute:
            raise RuntimeError("execute failed")
        return self._cursor

    def commit(self):
        if self.fail_on_commit:
            raise RuntimeError("commit failed")
        self.committed = True

    def close(self):
        pass


class TestConstructorModes:
    def test_simulated_mode(self):
        provider = LiquidityProvider(mode="simulated")
        assert provider.mode == "simulated"
        assert provider.birdeye_client is None

    def test_real_mode_no_sources_falls_back(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "BIRDEYE_AVAILABLE", False)
        monkeypatch.setattr(liquidity_module, "DEXSCREENER_AVAILABLE", False)
        monkeypatch.setattr(liquidity_module, "JUPITER_AVAILABLE", False)
        monkeypatch.setenv("SCOUT_LIQUIDITY_STRICT_MODE", "false")
        provider = LiquidityProvider(mode="real")
        assert provider.mode == "simulated"

    def test_real_mode_strict_raises(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "BIRDEYE_AVAILABLE", False)
        monkeypatch.setattr(liquidity_module, "DEXSCREENER_AVAILABLE", False)
        monkeypatch.setattr(liquidity_module, "JUPITER_AVAILABLE", False)
        monkeypatch.setenv("SCOUT_LIQUIDITY_STRICT_MODE", "true")
        with pytest.raises(RuntimeError, match="STRICT_MODE"):
            LiquidityProvider(mode="real")

    def test_real_mode_no_api_key_warns(self, monkeypatch):
        monkeypatch.delenv("BIRDEYE_API_KEY", raising=False)
        provider = LiquidityProvider(mode="real")
        assert provider.birdeye_client is None

    def test_real_mode_creates_birdeye_client(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "BIRDEYE_AVAILABLE", True)
        fake_birdeye = MagicMock()
        monkeypatch.setattr(liquidity_module, "BirdeyeClient", lambda key: fake_birdeye)
        provider = LiquidityProvider(mode="real", birdeye_api_key="key123")
        assert provider.birdeye_client is fake_birdeye

    def test_redis_initialized_from_config(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "REDIS_AVAILABLE", True)

        class FakeRedisClient:
            def __init__(self, redis_url=None, enabled=True):
                self._redis = FakeRedis()

            def is_available(self):
                return True

        monkeypatch.setattr(liquidity_module, "RedisClient", FakeRedisClient)

        class FakeScoutConfig:
            @staticmethod
            def get_redis_enabled():
                return True

            @staticmethod
            def get_redis_url():
                return "redis://x"

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = LiquidityProvider(mode="simulated")
        assert provider.redis_client is not None

    def test_redis_unavailable_falls_back(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "REDIS_AVAILABLE", True)

        class FakeRedisClient:
            def __init__(self, redis_url=None, enabled=True):
                pass

            def is_available(self):
                return False

        monkeypatch.setattr(liquidity_module, "RedisClient", FakeRedisClient)

        class FakeScoutConfig:
            @staticmethod
            def get_redis_enabled():
                return True

            @staticmethod
            def get_redis_url():
                return "redis://x"

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = LiquidityProvider(mode="simulated")
        assert provider.redis_client is None

    def test_redis_init_exception(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "REDIS_AVAILABLE", True)

        class FakeRedisClient:
            def __init__(self, redis_url=None, enabled=True):
                raise RuntimeError("redis broke")

        monkeypatch.setattr(liquidity_module, "RedisClient", FakeRedisClient)

        class FakeScoutConfig:
            @staticmethod
            def get_redis_enabled():
                return True

            @staticmethod
            def get_redis_url():
                return "redis://x"

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = LiquidityProvider(mode="simulated")
        assert provider.redis_client is None


class TestRateLimit:
    def test_sleeps_between_requests(self):
        provider = make_simulated_provider()
        provider._rate_limit_delay = 0.001
        provider._last_request_time = time.time() - 5
        with patch("time.sleep") as mock_sleep:
            provider._rate_limit()
            mock_sleep.assert_not_called()
            provider._last_request_time = time.time()
            provider._rate_limit()
            mock_sleep.assert_called_once()

    async def test_async_rate_limit(self):
        provider = make_simulated_provider()
        provider._rate_limit_delay = 0.001
        provider._last_request_time = 0.0
        await provider._rate_limit_async()
        await provider._rate_limit_async()


class TestCleanupAsyncClient:
    def test_none_client(self):
        make_simulated_provider()._cleanup_async_client_session(None)

    def test_drops_session(self):
        client = MagicMock()
        client._session = MagicMock()
        client._own_session = True
        make_simulated_provider()._cleanup_async_client_session(client)
        assert client._session is None
        assert client._own_session is False

    def test_no_session(self):
        client = MagicMock()
        del client._session
        make_simulated_provider()._cleanup_async_client_session(client)


class TestRunAsyncCoro:
    def test_success(self):
        async def coro():
            return 42

        assert make_simulated_provider()._run_async_coro(coro()) == 42

    def test_runtime_error_fallback_to_executor(self):
        async def coro():
            return 7

        with patch.object(liquidity_module.asyncio, "run", side_effect=[RuntimeError("loop"), 7]):
            assert make_simulated_provider()._run_async_coro(coro()) == 7

    def test_executor_timeout(self):
        async def coro():
            return 1

        fake_future = MagicMock()
        fake_future.result.side_effect = concurrent.futures.TimeoutError("slow")
        fake_executor = MagicMock()
        fake_executor.submit.return_value = fake_future

        with patch.object(liquidity_module.asyncio, "run", side_effect=RuntimeError("loop")), patch.object(
            liquidity_module.concurrent.futures, "ThreadPoolExecutor", return_value=fake_executor
        ):
            result = make_simulated_provider()._run_async_coro(coro())
        assert result is None
        fake_executor.shutdown.assert_called_with(wait=False)


class TestGetCurrentLiquidity:
    def test_cache_hit(self):
        provider = make_simulated_provider()
        data = make_liq_data()
        provider._add_to_cache("token_1", data)
        with patch.object(provider, "_simulate_current_liquidity") as mock_sim:
            result = provider.get_current_liquidity("token_1")
        mock_sim.assert_not_called()
        assert result is data

    def test_simulated_mode(self):
        provider = make_simulated_provider()
        result = provider.get_current_liquidity("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263")
        assert result is not None
        assert result.source == "simulated"
        assert "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263" in provider._cache

    def test_simulated_unknown_token(self):
        provider = make_simulated_provider()
        result = provider.get_current_liquidity("unknown_token_abc")
        assert result is not None

    def test_real_mode_birdeye_only(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.birdeye_client = FakeBirdeye(current=make_liq_data(liquidity=50000.0, source="birdeye"))
        result = provider.get_current_liquidity("token_1")
        assert result.source == "birdeye"
        assert result.liquidity_usd == 50000.0

    def test_real_mode_birdeye_raises(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.birdeye_client = FakeBirdeye(error=RuntimeError("api down"))
        result = provider.get_current_liquidity("token_1")
        assert result is None

    def test_real_mode_dexscreener(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.dexscreener_client = FakeDexScreener(current=make_liq_data(liquidity=40000.0, source="dexscreener"))
        result = provider.get_current_liquidity("token_1")
        assert result.source == "dexscreener"

    def test_real_mode_dexscreener_raises(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.dexscreener_client = FakeDexScreener(error=RuntimeError("down"))
        result = provider.get_current_liquidity("token_1")
        assert result is None

    def test_real_mode_jupiter(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.jupiter_client = FakeJupiter(current=make_liq_data(liquidity=0.0, price=2.0, source="jupiter"))
        result = provider.get_current_liquidity("token_1")
        assert result is not None

    def test_real_mode_jupiter_raises(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.jupiter_client = FakeJupiter(error=RuntimeError("down"))
        result = provider.get_current_liquidity("token_1")
        assert result is None

    def test_ranking_picks_highest_liquidity(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.birdeye_client = FakeBirdeye(current=make_liq_data(liquidity=1000.0, source="birdeye"))
        provider.dexscreener_client = FakeDexScreener(current=make_liq_data(liquidity=9000.0, source="dexscreener"))
        result = provider.get_current_liquidity("token_1")
        assert result.source == "dexscreener"


class TestRankLiquiditySources:
    def test_empty(self):
        assert make_simulated_provider()._rank_liquidity_sources([], "t") is None

    def test_filters_zero_liquidity(self):
        zero = make_liq_data(liquidity=0.0, source="jupiter")
        positive = make_liq_data(liquidity=5000.0, source="dexscreener")
        best = make_simulated_provider()._rank_liquidity_sources([zero, positive], "t")
        assert best is positive

    def test_timestamp_fallback(self):
        naive = make_liq_data(liquidity=10.0, source="a")
        naive.timestamp = "not-a-datetime"
        best = make_simulated_provider()._rank_liquidity_sources([naive], "t")
        assert best is naive


class TestHistoricalLiquidity:
    def test_from_database(self, fake_db_layer, monkeypatch):
        monkeypatch.setattr(liquidity_module, "translate_ddl", lambda s: s)
        provider = make_simulated_provider()
        conn = fake_db_layer
        conn.execute("""
            CREATE TABLE historical_liquidity (
                token_address TEXT,
                liquidity_usd REAL,
                price_usd REAL,
                volume_24h_usd REAL,
                timestamp TEXT,
                source TEXT
            )
        """)
        provider._get_database_connection = lambda: MagicMock()
        with patch.object(provider, "_get_from_database", return_value=make_liq_data()):
            result = provider.get_historical_liquidity("token_1", utcnow())
        assert result is not None

    def test_db_miss_then_birdeye(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.birdeye_client = FakeBirdeye(historical=make_liq_data(ts=utcnow(), source="birdeye"))
        with patch.object(provider, "_get_from_database", return_value=None), patch.object(
            provider, "_store_in_database", return_value=True
        ) as mock_store:
            result = provider.get_historical_liquidity("token_1", utcnow(), tolerance_hours=6)
        assert result.source == "birdeye"
        mock_store.assert_called_once()

    def test_db_miss_birdeye_out_of_tolerance(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        old = utcnow() - timedelta(hours=50)
        provider.birdeye_client = FakeBirdeye(historical=make_liq_data(ts=old, source="birdeye"))
        with patch.object(provider, "_get_from_database", return_value=None):
            result = provider.get_historical_liquidity("token_1", utcnow(), tolerance_hours=6)
        assert result is None

    def test_db_miss_birdeye_raises(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.birdeye_client = FakeBirdeye(error=RuntimeError("down"))
        with patch.object(provider, "_get_from_database", return_value=None):
            result = provider.get_historical_liquidity("token_1", utcnow())
        assert result is None

    def test_strict_mode_blocks_fallback(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true")
        provider = make_simulated_provider()
        with patch.object(provider, "_get_from_database", return_value=None):
            result = provider.get_historical_liquidity("token_1", utcnow())
        assert result is None

    def test_strict_mode_env_fallback_on_import_error(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true")
        provider = make_simulated_provider()
        with patch.object(liquidity_module, "ScoutConfig", None), patch.object(
            provider, "_get_from_database", return_value=None
        ):
            result = provider.get_historical_liquidity("token_1", utcnow())
        assert result is None


class TestHistoricalOrCurrent:
    def test_historical_returns_with_confidence(self):
        provider = make_simulated_provider()
        data = make_liq_data()
        with patch.object(provider, "get_historical_liquidity", return_value=data):
            result = provider.get_historical_liquidity_or_current("token_1", utcnow())
        assert result.source.endswith("confidence_1.0")

    def test_grace_period_naive_timestamp(self):
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=10000.0)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None):
            naive_ts = datetime.now() - timedelta(days=1)
            result = provider.get_historical_liquidity_or_current("token_1", naive_ts)
        assert result is not None
        assert "grace_period" in result.source
        assert result.liquidity_usd == pytest.approx(7000.0)

    def test_grace_period_aware_timestamp(self):
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=10000.0)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None):
            aware_ts = utcnow() - timedelta(days=1)
            result = provider.get_historical_liquidity_or_current("token_1", aware_ts)
        assert result is not None

    def test_grace_period_no_current(self):
        provider = make_simulated_provider()
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=None
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current("token_1", utcnow() - timedelta(days=2))
        assert result is None

    def test_strict_mode_rejects(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "true")
        provider = make_simulated_provider()
        old_ts = utcnow() - timedelta(days=60)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=make_liq_data()
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current("token_1", old_ts)
        assert result is None

    def test_flexible_mode_allows_fallback(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true")
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=20000.0)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current(
                "token_1", utcnow() - timedelta(days=60), strategy="SHIELD"
            )
        assert result is not None
        assert "confidence_weighted" in result.source
        assert result.liquidity_usd <= 5000.0

    def test_spear_strategy_higher_cap(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true")
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=100000.0)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current(
                "token_1", utcnow() - timedelta(days=60), strategy="SPEAR"
            )
        assert result.liquidity_usd <= 10000.0

    def test_fallback_disabled(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "false")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "false")
        provider = make_simulated_provider()
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=make_liq_data()
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current(
                "token_1", utcnow() - timedelta(days=60)
            )
        assert result is None

    def test_simulated_last_resort(self, monkeypatch):
        monkeypatch.setenv("SCOUT_LIQUIDITY_MODE", "simulated")
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        provider = make_simulated_provider()
        provider._get_simulated_liquidity = lambda token, ts: make_liq_data(source="simulated")
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=None
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current(
                "token_1", utcnow() - timedelta(days=60)
            )
        assert result is not None
        assert result.source == "simulated"

    def test_all_sources_failed(self, monkeypatch):
        monkeypatch.setenv("SCOUT_LIQUIDITY_MODE", "real")
        provider = make_simulated_provider()
        provider.mode = "simulated"
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=None
        ), patch.object(liquidity_module, "ScoutConfig", None):
            result = provider.get_historical_liquidity_or_current(
                "token_1", utcnow() - timedelta(days=60)
            )
        assert result is None

    def test_token_creation_time_factors(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true")
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=10000.0)
        old_ts = utcnow() - timedelta(days=60)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None), patch(
            "core.advanced_cache.get_token_creation_time", return_value=(utcnow() - timedelta(days=1)).timestamp()
        ):
            result = provider.get_historical_liquidity_or_current("token_1", old_ts)
        assert result is not None

    def test_token_creation_time_exception(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true")
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=10000.0)
        old_ts = utcnow() - timedelta(days=60)
        with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
            provider, "get_current_liquidity", return_value=current
        ), patch.object(liquidity_module, "ScoutConfig", None), patch(
            "core.advanced_cache.get_token_creation_time", side_effect=RuntimeError("cache down")
        ):
            result = provider.get_historical_liquidity_or_current("token_1", old_ts)
        assert result is not None

    def test_token_age_bands(self, monkeypatch):
        monkeypatch.setenv("SCOUT_STRICT_HISTORICAL_LIQUIDITY", "flexible")
        monkeypatch.setenv("SCOUT_LIQUIDITY_ALLOW_FALLBACK", "true")
        provider = make_simulated_provider()
        current = make_liq_data(liquidity=10000.0)
        old_ts = utcnow() - timedelta(days=60)
        for days_old in (1, 10, 60):
            with patch.object(provider, "get_historical_liquidity", return_value=None), patch.object(
                provider, "get_current_liquidity", return_value=current
            ), patch.object(liquidity_module, "ScoutConfig", None), patch(
                "core.advanced_cache.get_token_creation_time",
                return_value=(utcnow() - timedelta(days=days_old)).timestamp(),
            ):
                result = provider.get_historical_liquidity_or_current("token_1", old_ts)
            assert result is not None


class TestDatabaseHelpers:
    def test_get_database_connection(self, fake_db_layer):
        provider = make_simulated_provider()
        conn = provider._get_database_connection()
        assert conn is not None

    def test_get_from_database_no_path(self):
        provider = make_simulated_provider()
        provider.db_path = None
        assert provider._get_from_database("t", utcnow()) is None

    def test_get_from_database_row_found(self):
        provider = make_simulated_provider()
        row = {
            "liquidity_usd": 5000.0,
            "price_usd": 1.0,
            "volume_24h_usd": 2500.0,
            "timestamp": "2026-01-01T00:00:00+00:00",
            "source": "database",
        }
        fake_conn = FakeConn(cursor=FakeCursor(row=row))
        provider._get_database_connection = lambda: fake_conn
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        result = provider._get_from_database("t", ts, tolerance_hours=6)
        assert result is not None
        assert result.source == "database"

    def test_get_from_database_naive_row_timestamp(self):
        provider = make_simulated_provider()
        row = {
            "liquidity_usd": 5000.0,
            "price_usd": 1.0,
            "volume_24h_usd": 2500.0,
            "timestamp": datetime(2026, 1, 1),  # naive datetime
            "source": "db",
        }
        fake_conn = FakeConn(cursor=FakeCursor(row=row))
        provider._get_database_connection = lambda: fake_conn
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        result = provider._get_from_database("t", ts, tolerance_hours=6)
        assert result is not None

    def test_get_from_database_out_of_tolerance(self):
        provider = make_simulated_provider()
        row = {
            "liquidity_usd": 5000.0,
            "price_usd": 1.0,
            "volume_24h_usd": 2500.0,
            "timestamp": datetime(2025, 1, 1, tzinfo=timezone.utc),
            "source": "db",
        }
        fake_conn = FakeConn(cursor=FakeCursor(row=row))
        provider._get_database_connection = lambda: fake_conn
        ts = datetime(2026, 1, 1, tzinfo=timezone.utc)
        assert provider._get_from_database("t", ts, tolerance_hours=6) is None

    def test_get_from_database_exception(self):
        provider = make_simulated_provider()
        provider._get_database_connection = lambda: FakeConn(
            cursor=FakeCursor(fail_on_execute=True)
        )
        assert provider._get_from_database("t", utcnow()) is None

    def test_store_in_database_no_path(self):
        provider = make_simulated_provider()
        provider.db_path = None
        assert provider._store_in_database(make_liq_data()) is False

    def test_store_in_database_success(self):
        provider = make_simulated_provider()
        fake_conn = FakeConn()
        provider._get_database_connection = lambda: fake_conn
        assert provider._store_in_database(make_liq_data()) is True
        assert fake_conn.committed is True

    def test_store_in_database_exception(self):
        provider = make_simulated_provider()
        provider._get_database_connection = lambda: FakeConn(fail_on_cursor=True)
        assert provider._store_in_database(make_liq_data()) is False

    def test_store_batch_empty(self):
        provider = make_simulated_provider()
        assert provider.store_liquidity_batch([]) == 0

    def test_store_batch_no_path(self):
        provider = make_simulated_provider()
        provider.db_path = None
        assert provider.store_liquidity_batch([make_liq_data()]) == 0

    def test_store_batch_success(self):
        provider = make_simulated_provider()
        fake_conn = FakeConn()
        provider._get_database_connection = lambda: fake_conn
        count = provider.store_liquidity_batch([make_liq_data(), make_liq_data(token="t2")])
        assert count == 2

    def test_store_batch_row_failure_continues(self):
        provider = make_simulated_provider()
        cursor = FakeCursor(fail_after=1)
        fake_conn = FakeConn(cursor=cursor)
        provider._get_database_connection = lambda: fake_conn
        count = provider.store_liquidity_batch([make_liq_data(), make_liq_data(token="t2")])
        assert count == 0

    def test_store_batch_outer_exception(self):
        provider = make_simulated_provider()
        provider._get_database_connection = lambda: FakeConn(fail_on_cursor=True)
        assert provider.store_liquidity_batch([make_liq_data()]) == 0


class TestEstimateSlippage:
    def test_zero_liquidity(self):
        provider = make_simulated_provider()
        assert provider.estimate_slippage("t", 1.0, 0) == 1.0

    def test_legacy_sqrt_model(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", False)
        provider = make_simulated_provider()
        result = provider.estimate_slippage("t", 1.0, 100000, sol_price_usd=150.0)
        assert 0.0 < result < 1.0

    def test_cpmm_model(self, monkeypatch):
        class FakeScoutConfig:
            @staticmethod
            def get_use_cpmm_slippage():
                return True

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = make_simulated_provider()
        result = provider.estimate_slippage("t", 1.0, 100000, sol_price_usd=150.0)
        assert 0.0 < result < 1.0

    def test_cpmm_turnover_high(self, monkeypatch):
        class FakeScoutConfig:
            @staticmethod
            def get_use_cpmm_slippage():
                return True

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = make_simulated_provider()
        assert provider.estimate_slippage("t", 1.0, 10000, sol_price_usd=150.0, volume_24h_usd=200000.0) > provider.estimate_slippage("t", 1.0, 10000, sol_price_usd=150.0, volume_24h_usd=1000.0)

    def test_cpmm_turnover_medium(self, monkeypatch):
        class FakeScoutConfig:
            @staticmethod
            def get_use_cpmm_slippage():
                return True

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = make_simulated_provider()
        result = provider.estimate_slippage("t", 1.0, 10000, sol_price_usd=150.0, volume_24h_usd=40000.0)
        assert 0.0 < result < 1.0

    def test_cpmm_turnover_low(self, monkeypatch):
        class FakeScoutConfig:
            @staticmethod
            def get_use_cpmm_slippage():
                return True

        monkeypatch.setattr(liquidity_module, "ScoutConfig", FakeScoutConfig)
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", True)
        provider = make_simulated_provider()
        result = provider.estimate_slippage("t", 1.0, 10000, sol_price_usd=150.0, volume_24h_usd=15000.0)
        assert 0.0 < result < 1.0

    def test_legacy_turnover_bands(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", False)
        provider = make_simulated_provider()
        for volume in (500000.0, 150000.0, 60000.0, 20000.0):
            result = provider.estimate_slippage(
                "t", 1.0, 10000, sol_price_usd=150.0, volume_24h_usd=volume
            )
            assert 0.0 < result < 1.0

    def test_age_additive(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", False)
        provider = make_simulated_provider()
        for age in (0.1, 0.5, 5.0, 20.0, 60.0, 200.0, 400.0, None):
            result = provider.estimate_slippage(
                "t", 1.0, 100000, sol_price_usd=150.0, token_age_days=age
            )
            assert result >= 0.0

    def test_slippage_capped_at_1(self, monkeypatch):
        monkeypatch.setattr(liquidity_module, "CONFIG_AVAILABLE", False)
        provider = make_simulated_provider()
        result = provider.estimate_slippage("t", 500.0, 100.0, sol_price_usd=150.0)
        assert result == 1.0


class FakePriceResponse:
    def __init__(self, data=None, status=200, error=None):
        self._data = data or {}
        self._status = status
        self._error = error

    def raise_for_status(self):
        if self._status != 200:
            raise RuntimeError(f"HTTP {self._status}")

    async def json(self):
        if self._error:
            raise self._error
        return self._data

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False


class FakePriceSession:
    def __init__(self, response):
        self._response = response

    def get(self, url, params=None, timeout=None):
        return self._response

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False

    async def close(self):
        pass


class TestSolPrice:
    async def test_cached_price(self):
        provider = make_simulated_provider()
        provider._sol_price_cache = (150.0, utcnow())
        assert await provider.get_sol_price_usd() == 150.0

    async def test_jupiter_client_price(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.jupiter_client = FakeJupiter(sol_price=170.0)
        assert await provider.get_sol_price_usd() == 170.0
        assert provider._sol_price_cache[0] == 170.0

    async def test_jupiter_client_fails(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        provider.jupiter_client = FakeJupiter(error=RuntimeError("jupiter down"))
        session = FakePriceSession(FakePriceResponse(status=500))
        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.get_sol_price_usd()
        assert result == provider._sol_fallback_price

    async def test_direct_api_call(self):
        provider = make_simulated_provider()
        response = FakePriceResponse(data={"So11111111111111111111111111111111111111112": {"usdPrice": 180.0}})
        session = FakePriceSession(response)
        with patch("aiohttp.ClientSession", return_value=session):
            assert await provider.get_sol_price_usd() == 180.0
        assert provider._last_known_sol_price == 180.0

    async def test_direct_api_call_invalid_price(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        response = FakePriceResponse(data={"So11111111111111111111111111111111111111112": {"usdPrice": "x"}})
        session = FakePriceSession(response)
        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.get_sol_price_usd()
        assert result == provider._sol_fallback_price

    async def test_direct_api_call_raises(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        response = FakePriceResponse(status=500)
        session = FakePriceSession(response)
        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.get_sol_price_usd()
        assert result == provider._sol_fallback_price

    async def test_from_liquidity_sources(self):
        provider = make_simulated_provider()
        session = FakePriceSession(FakePriceResponse(status=500))
        with patch("aiohttp.ClientSession", return_value=session), patch.object(
            provider, "get_current_liquidity", return_value=make_liq_data(price=160.0)
        ):
            assert await provider.get_sol_price_usd() == 160.0

    async def test_from_liquidity_sources_exception(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        session = FakePriceSession(FakePriceResponse(status=500))
        with patch("aiohttp.ClientSession", return_value=session), patch.object(
            provider, "get_current_liquidity", side_effect=RuntimeError("down")
        ):
            result = await provider.get_sol_price_usd()
        assert result == provider._sol_fallback_price

    async def test_last_known_price_fallback(self):
        provider = make_simulated_provider()
        provider._last_known_sol_price = 155.0
        session = FakePriceSession(FakePriceResponse(status=500))
        with patch("aiohttp.ClientSession", return_value=session), patch.object(
            provider, "get_current_liquidity", return_value=None
        ):
            assert await provider.get_sol_price_usd() == 155.0

    def test_get_sol_price_usd_sync_cached(self):
        provider = make_simulated_provider()
        provider._sol_price_cache = (150.0, utcnow())
        assert provider.get_sol_price_usd_sync() == 150.0

    def test_get_sol_price_usd_sync_last_known(self):
        provider = make_simulated_provider()
        provider._last_known_sol_price = 145.0
        assert provider.get_sol_price_usd_sync() == 145.0

    def test_get_sol_price_usd_sync_fallback(self):
        provider = make_simulated_provider()
        assert provider.get_sol_price_usd_sync() == provider._sol_fallback_price


class TestPriceHistoryAndRegime:
    def test_cache_historical_price(self):
        provider = make_simulated_provider()
        for i in range(250):
            provider.cache_historical_sol_price(utcnow() - timedelta(hours=i), 100.0 + i)
        assert len(provider._sol_price_history) == 200

    def test_classify_short_span(self):
        provider = make_simulated_provider()
        assert provider.classify_market_regime(utcnow(), utcnow() + timedelta(days=1)) is None

    def test_classify_from_history_bull(self):
        provider = make_simulated_provider()
        start = utcnow() - timedelta(days=30)
        provider.cache_historical_sol_price(start, 100.0)
        provider.cache_historical_sol_price(start + timedelta(days=29), 150.0)
        assert provider.classify_market_regime(start, start + timedelta(days=30)) == "BULL"

    def test_classify_from_history_bear(self):
        provider = make_simulated_provider()
        start = utcnow() - timedelta(days=30)
        provider.cache_historical_sol_price(start, 150.0)
        provider.cache_historical_sol_price(start + timedelta(days=29), 100.0)
        assert provider.classify_market_regime(start, start + timedelta(days=30)) == "BEAR"

    def test_classify_from_history_sideways(self):
        provider = make_simulated_provider()
        start = utcnow() - timedelta(days=30)
        provider.cache_historical_sol_price(start, 150.0)
        provider.cache_historical_sol_price(start + timedelta(days=29), 155.0)
        assert provider.classify_market_regime(start, start + timedelta(days=30)) == "SIDEWAYS"

    def test_classify_heuristic_long_span(self):
        provider = make_simulated_provider()
        start = utcnow() - timedelta(days=60)
        with patch.object(provider, "get_sol_price_usd_sync", return_value=100.0):
            assert provider.classify_market_regime(start, utcnow()) == "BEAR"

    def test_classify_heuristic_short_span_zero(self):
        provider = make_simulated_provider()
        start = utcnow() - timedelta(days=10)
        with patch.object(provider, "get_sol_price_usd_sync", return_value=200.0):
            assert provider.classify_market_regime(start, utcnow()) == "SIDEWAYS"


class TestCacheOperations:
    def test_redis_cache_get(self):
        provider = make_simulated_provider()
        redis = FakeRedis(data={"liquidity:token_1": (
            '{"token_address": "token_1", "liquidity_usd": 100.0, "price_usd": 1.0, '
            '"volume_24h_usd": 50.0, "timestamp": "%s", "source": "cache"}' % utcnow().isoformat()
        )})
        provider.redis_client = redis
        result = provider._get_from_cache("token_1")
        assert result.source == "cache"
        assert result.liquidity_usd == 100.0

    def test_redis_cache_get_error(self):
        provider = make_simulated_provider()
        provider.redis_client = FakeRedis(error=RuntimeError("down"))
        provider._cache["token_1"] = (make_liq_data(), utcnow())
        result = provider._get_from_cache("token_1")
        assert result is not None

    def test_in_memory_cache_valid(self):
        provider = make_simulated_provider()
        data = make_liq_data()
        provider._cache["token_1"] = (data, utcnow())
        assert provider._get_from_cache("token_1") is data

    def test_in_memory_cache_expired(self):
        provider = make_simulated_provider()
        provider._cache["token_1"] = (make_liq_data(), utcnow() - timedelta(hours=2))
        assert provider._get_from_cache("token_1") is None
        assert "token_1" not in provider._cache

    def test_redis_cache_add(self):
        provider = make_simulated_provider()
        redis = FakeRedis()
        provider.redis_client = redis
        provider._add_to_cache("token_1", make_liq_data())
        assert "liquidity:token_1" in redis._data
        assert "token_1" not in provider._cache

    def test_redis_cache_add_error_falls_back(self):
        provider = make_simulated_provider()
        provider.redis_client = FakeRedis(error=RuntimeError("down"))
        provider._add_to_cache("token_1", make_liq_data())
        assert "token_1" in provider._cache

    def test_clear_cache_redis_keys(self):
        provider = make_simulated_provider()
        redis = FakeRedis(data={"liquidity:a": "x", "other:b": "y"})
        provider.redis_client = redis
        provider._cache["token_1"] = (make_liq_data(), utcnow())
        provider.clear_cache()
        assert "liquidity:a" not in redis._data
        assert "other:b" in redis._data
        assert provider._cache == {}

    def test_clear_cache_redis_error(self):
        provider = make_simulated_provider()
        redis = FakeRedis(data={"liquidity:a": "x"})
        redis.delete = MagicMock(side_effect=RuntimeError("down"))
        provider.redis_client = redis
        provider.clear_cache()

    def test_close_clients(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        birdeye = FakeBirdeye()
        jupiter = FakeJupiter()
        provider.birdeye_client = birdeye
        provider.jupiter_client = jupiter
        import asyncio as aio

        async def run():
            await provider.close()

        aio.run(run())
        assert birdeye.closed
        assert jupiter.closed

    def test_close_clients_error(self):
        provider = make_simulated_provider()
        provider.mode = "real"
        birdeye = FakeBirdeye()
        birdeye.close = AsyncMock(side_effect=RuntimeError("close failed"))
        provider.birdeye_client = birdeye
        jupiter = FakeJupiter()
        jupiter.close = AsyncMock(side_effect=RuntimeError("jupiter close failed"))
        provider.jupiter_client = jupiter
        import asyncio as aio
        aio.run(provider.close())


class TestImportFallbacks:
    """Poison optional imports and reload the module — must run last."""

    def test_import_fallbacks(self, monkeypatch):
        for mod_name in (
            "core.birdeye_client",
            "core.liquidity_sources.dexscreener_client",
            "core.liquidity_sources.jupiter_client",
            "core.redis_client",
            "config",
        ):
            monkeypatch.setitem(sys.modules, mod_name, None)
        importlib.reload(liquidity_module)
        try:
            assert liquidity_module.BIRDEYE_AVAILABLE is False
            assert liquidity_module.DEXSCREENER_AVAILABLE is False
            assert liquidity_module.JUPITER_AVAILABLE is False
            assert liquidity_module.REDIS_AVAILABLE is False
            assert liquidity_module.CONFIG_AVAILABLE is False
            # config import fails now: exercise the env-var fallback branches
            provider = liquidity_module.LiquidityProvider(mode="simulated")
            provider._get_simulated_liquidity = lambda token, ts: make_liq_data(source="simulated")
            with patch.object(provider, "_get_from_database", return_value=None), patch.object(
                provider, "get_current_liquidity", return_value=None
            ):
                assert provider.get_historical_liquidity("token_1", utcnow()) is None
                result = provider.get_historical_liquidity_or_current(
                    "token_1", utcnow() - timedelta(days=60)
                )
                assert result is None
        finally:
            monkeypatch.undo()
            importlib.reload(liquidity_module)
            assert liquidity_module.BIRDEYE_AVAILABLE is True
