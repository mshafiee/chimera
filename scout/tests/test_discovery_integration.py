"""
Integration tests for wallet discovery enhancements.

Covers:
  - Parallel strategy execution (Item 2)
  - Redis discovery cache (Item 3)
  - Circuit breaker logging (Item 4)
  - Balance validation fail modes (Item 5)
  - API key error handling (Item 1)
  - Persistent wallet dedup (Item 7)
  - Batch activity validation (Item 8)
"""

import json
import time
import logging
import asyncio
from unittest.mock import patch, AsyncMock, MagicMock

import pytest

from core.helius_client import HeliusClient, DiscoveryError


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

WALLET_A = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
WALLET_B = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AB"
WALLET_C = "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321CD"


@pytest.fixture
def helius_client():
    return HeliusClient(api_key="test-api-key")


class FakeRedis:
    """Minimal Redis mock that supports get/set/smembers/sadd/expire/pipeline."""

    def __init__(self):
        self._data = {}
        self._sets = {}
        self._ttls = {}
        self.enabled = True
        self.redis_client = self

    def is_available(self):
        return True

    def get(self, key):
        return self._data.get(key)

    def set(self, key, value, ttl_seconds=None):
        self._data[key] = value
        if ttl_seconds:
            self._ttls[key] = ttl_seconds

    def smembers(self, key):
        return list(self._sets.get(key, set()))

    def sadd(self, key, *values):
        if key not in self._sets:
            self._sets[key] = set()
        self._sets[key].update(values)

    def expire(self, key, ttl):
        self._ttls[key] = ttl

    def pipeline(self):
        outer = self

        class _Pipe:
            def __init__(self):
                self._cmds = []

            def sadd(self, key, *vals):
                self._cmds.append(("sadd", key, vals))
                return self

            def expire(self, key, ttl):
                self._cmds.append(("expire", key, ttl))
                return self

            def execute(self):
                for cmd in self._cmds:
                    if cmd[0] == "sadd":
                        outer.sadd(cmd[1], *cmd[2])
                    elif cmd[0] == "expire":
                        outer.expire(cmd[1], cmd[2])

        return _Pipe()


# ---------------------------------------------------------------------------
# Item 1 — API key error handling
# ---------------------------------------------------------------------------

class TestAPIKeyHandling:

    async def test_missing_api_key_non_strict_returns_empty(self):
        """Non-strict mode returns [] when API key is missing."""
        client = HeliusClient(api_key=None)
        with patch.dict("os.environ", {}, clear=False):
            # Ensure no env fallback
            with patch.object(client, "api_key", None):
                wallets = await client.discover_wallets_from_recent_swaps()
        assert wallets == []

    async def test_missing_api_key_strict_raises(self):
        """Strict mode raises DiscoveryError when API key is missing."""
        client = HeliusClient(api_key=None)
        with patch.object(client, "api_key", None):
            with pytest.raises(DiscoveryError):
                await client.discover_wallets_from_recent_swaps(strict=True)


# ---------------------------------------------------------------------------
# Item 2 — Parallel strategy execution
# ---------------------------------------------------------------------------

class TestParallelStrategies:

    async def test_strategies_2_4_run_in_parallel(self, helius_client):
        """When strategy 1 yields < threshold, strategies 2-4 run concurrently."""
        call_times = {}

        async def mock_blocks(**kw):
            call_times["blocks_start"] = time.time()
            await asyncio.sleep(0.05)
            call_times["blocks_end"] = time.time()
            return {WALLET_B: 4}

        async def mock_programs(**kw):
            call_times["programs_start"] = time.time()
            await asyncio.sleep(0.05)
            call_times["programs_end"] = time.time()
            return {WALLET_C: 3}

        async def mock_seeds(**kw):
            call_times["seeds_start"] = time.time()
            await asyncio.sleep(0.05)
            call_times["seeds_end"] = time.time()
            return {}

        with patch.object(helius_client, "_discover_from_active_tokens", new_callable=AsyncMock) as m1, \
             patch.object(helius_client, "_discover_from_recent_blocks", new_callable=AsyncMock) as m2, \
             patch.object(helius_client, "_discover_from_dex_programs", new_callable=AsyncMock) as m3, \
             patch.object(helius_client, "_discover_from_seed_wallets", new_callable=AsyncMock) as m4, \
             patch.object(helius_client, "discover_from_top_performing_tokens", new_callable=AsyncMock) as m5:

            m1.return_value = {WALLET_A: 5}
            m2.side_effect = mock_blocks
            m3.side_effect = mock_programs
            m4.side_effect = mock_seeds
            m5.return_value = []

            # max_wallets=200 means threshold=100, strategy 1 finds only 1 → triggers 2-4
            await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=2, max_wallets=200
            )

        # If parallel: programs_start < blocks_end (they overlap)
        assert call_times["programs_start"] < call_times["blocks_end"]

    async def test_strategy_1_enough_skips_parallel(self, helius_client):
        """When strategy 1 yields >= threshold, strategies 2-4 don't run."""
        with patch.object(helius_client, "_discover_from_active_tokens", new_callable=AsyncMock) as m1, \
             patch.object(helius_client, "_discover_from_recent_blocks", new_callable=AsyncMock) as m2, \
             patch.object(helius_client, "_discover_from_dex_programs", new_callable=AsyncMock) as m3, \
             patch.object(helius_client, "_discover_from_seed_wallets", new_callable=AsyncMock) as m4, \
             patch.object(helius_client, "discover_from_top_performing_tokens", new_callable=AsyncMock) as m5:

            # Return enough wallets from strategy 1
            m1.return_value = {f"wallet_{i}": 5 for i in range(20)}
            m2.return_value = {}
            m3.return_value = {}
            m4.return_value = {}
            m5.return_value = []

            await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=2, max_wallets=20
            )

        m1.assert_called_once()
        m2.assert_not_called()
        m3.assert_not_called()
        m4.assert_not_called()


# ---------------------------------------------------------------------------
# Item 3 — Redis discovery cache
# ---------------------------------------------------------------------------

class TestRedisCache:

    async def test_redis_cache_hit(self, helius_client):
        """Redis cache hit returns cached wallets without running strategies."""
        fake_redis = FakeRedis()
        fake_redis.set(
            "scout:discovery:24:50",
            json.dumps([WALLET_A, WALLET_B]),
            ttl_seconds=3600,
        )
        helius_client._redis = fake_redis

        with patch.object(helius_client, "_discover_from_active_tokens") as m:
            result = await helius_client.discover_wallets_from_recent_swaps(
                max_wallets=50, hours_back=24
            )

        m.assert_not_called()
        assert result == [WALLET_A, WALLET_B]

    async def test_redis_cache_miss_writes_to_redis(self, helius_client):
        """On cache miss, discovery runs and result is stored in Redis."""
        fake_redis = FakeRedis()
        helius_client._redis = fake_redis

        with patch.object(helius_client, "_discover_from_active_tokens", new_callable=AsyncMock) as m1, \
             patch.object(helius_client, "_discover_from_recent_blocks", new_callable=AsyncMock) as m2, \
             patch.object(helius_client, "_discover_from_dex_programs", new_callable=AsyncMock) as m3, \
             patch.object(helius_client, "_discover_from_seed_wallets", new_callable=AsyncMock) as m4, \
             patch.object(helius_client, "discover_from_top_performing_tokens", new_callable=AsyncMock) as m5:

            m1.return_value = {WALLET_A: 5}
            m2.return_value = {}
            m3.return_value = {}
            m4.return_value = {}
            m5.return_value = []

            result = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=2, max_wallets=50, hours_back=24
            )

        # Redis should now have the cache
        cached = fake_redis.get("scout:discovery:24:50")
        assert cached is not None
        assert json.loads(cached) == result


# ---------------------------------------------------------------------------
# Item 4 — Circuit breaker logging
# ---------------------------------------------------------------------------

class TestCircuitBreakerLogging:

    def test_logs_when_breaker_opens(self, helius_client, caplog):
        """Opening the circuit breaker emits a WARNING log."""
        with caplog.at_level(logging.WARNING):
            for _ in range(helius_client._circuit_breaker_threshold):
                helius_client._record_failure_sync()
        assert any("Circuit Breaker" in r.message and "OPENED" in r.message
                      for r in caplog.records)

    def test_logs_when_breaker_resets(self, helius_client, caplog):
        """Resetting the circuit breaker after cooldown emits an INFO log."""
        # Open the breaker
        for _ in range(helius_client._circuit_breaker_threshold):
            helius_client._record_failure_sync()

        # Set reset time to past
        helius_client._circuit_breaker_reset_time = time.time() - 1

        with caplog.at_level(logging.INFO):
            result = helius_client._check_circuit_breaker()

        assert result is True
        assert any("Circuit Breaker" in r.message and "Resetting" in r.message
                      for r in caplog.records)

    async def test_stats_not_stale_after_reset(self, helius_client):
        """get_rate_limit_stats doesn't report stale 'open' after reset."""
        for _ in range(helius_client._circuit_breaker_threshold):
            helius_client._record_failure_sync()

        helius_client._circuit_breaker_reset_time = time.time() - 1

        stats = await helius_client.get_rate_limit_stats()
        assert stats["circuit_breaker_open"] is False


# ---------------------------------------------------------------------------
# Item 5 — Balance validation fail modes
# ---------------------------------------------------------------------------

class TestBalanceFailModes:

    async def test_balance_fail_open_includes_batch(self, helius_client):
        """SCOUT_BALANCE_FAIL_MODE=open includes wallets on RPC failure."""
        wallets = [WALLET_A, WALLET_B]
        with patch.dict("os.environ", {
            "SCOUT_BALANCE_FAIL_MODE": "open",
            "CHIMERA_RPC__PRIMARY_URL": "http://fake-rpc.test",
        }):
            with patch.object(helius_client, "_get_session", new_callable=AsyncMock) as ms:
                mock_session = AsyncMock()
                mock_session.post.side_effect = Exception("RPC error")
                ms.return_value = mock_session
                result = await helius_client._filter_by_sol_balance(wallets)
        assert set(result) == set(wallets)

    async def test_balance_fail_closed_excludes_batch(self, helius_client):
        """SCOUT_BALANCE_FAIL_MODE=closed excludes wallets on RPC failure."""
        wallets = [WALLET_A, WALLET_B]
        with patch.dict("os.environ", {
            "SCOUT_BALANCE_FAIL_MODE": "closed",
            "CHIMERA_RPC__PRIMARY_URL": "http://fake-rpc.test",
        }):
            with patch.object(helius_client, "_get_session", new_callable=AsyncMock) as ms:
                mock_session = AsyncMock()
                mock_session.post.side_effect = Exception("RPC error")
                ms.return_value = mock_session
                result = await helius_client._filter_by_sol_balance(wallets)
        assert result == []


# ---------------------------------------------------------------------------
# Item 7 — Persistent wallet dedup
# ---------------------------------------------------------------------------

class TestPersistentDedup:

    async def test_dedup_filters_seen_wallets(self, helius_client):
        """Wallets in the Redis dedup set are filtered from results."""
        fake_redis = FakeRedis()
        fake_redis.sadd(helius_client._DEDUP_KEY, WALLET_A)
        helius_client._redis = fake_redis

        with patch.object(helius_client, "_discover_from_active_tokens", new_callable=AsyncMock) as m1, \
             patch.object(helius_client, "_discover_from_recent_blocks", new_callable=AsyncMock) as m2, \
             patch.object(helius_client, "_discover_from_dex_programs", new_callable=AsyncMock) as m3, \
             patch.object(helius_client, "_discover_from_seed_wallets", new_callable=AsyncMock) as m4, \
             patch.object(helius_client, "discover_from_top_performing_tokens", new_callable=AsyncMock) as m5:

            m1.return_value = {WALLET_A: 10, WALLET_B: 5}
            m2.return_value = {}
            m3.return_value = {}
            m4.return_value = {}
            m5.return_value = []

            result = await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=2, max_wallets=50
            )

        # WALLET_A was seen → filtered, WALLET_B remains
        assert WALLET_B in result
        assert WALLET_A not in result

    async def test_dedup_marks_wallets_seen(self, helius_client):
        """After discovery, wallets are added to the Redis dedup set."""
        fake_redis = FakeRedis()
        helius_client._redis = fake_redis

        with patch.object(helius_client, "_discover_from_active_tokens", new_callable=AsyncMock) as m1, \
             patch.object(helius_client, "_discover_from_recent_blocks", new_callable=AsyncMock) as m2, \
             patch.object(helius_client, "_discover_from_dex_programs", new_callable=AsyncMock) as m3, \
             patch.object(helius_client, "_discover_from_seed_wallets", new_callable=AsyncMock) as m4, \
             patch.object(helius_client, "discover_from_top_performing_tokens", new_callable=AsyncMock) as m5:

            m1.return_value = {WALLET_A: 5, WALLET_B: 3}
            m2.return_value = {}
            m3.return_value = {}
            m4.return_value = {}
            m5.return_value = []

            await helius_client.discover_wallets_from_recent_swaps(
                min_trade_count=2, max_wallets=50
            )

        seen = fake_redis.smembers(helius_client._DEDUP_KEY)
        assert WALLET_A in seen
        assert WALLET_B in seen


# ---------------------------------------------------------------------------
# Item 8 — Batch activity validation
# ---------------------------------------------------------------------------

class TestBatchActivityValidation:

    async def test_batch_validate_returns_only_valid(self, helius_client):
        """_batch_validate_activity returns only wallets that pass validation."""
        wallets = [WALLET_A, WALLET_B, WALLET_C]

        async def mock_validate(wallet, **kw):
            return wallet == WALLET_A  # Only A passes

        with patch.object(helius_client, "_validate_wallet_activity", side_effect=mock_validate):
            result = await helius_client._batch_validate_activity(
                wallets, min_trades=2, days_back=7
            )

        assert result == [WALLET_A]

    async def test_batch_validate_respects_max_wallets(self, helius_client):
        """_batch_validate_activity stops once max_wallets accepted."""
        wallets = [WALLET_A, WALLET_B, WALLET_C]

        async def mock_validate(wallet, **kw):
            return True

        with patch.object(helius_client, "_validate_wallet_activity", side_effect=mock_validate):
            result = await helius_client._batch_validate_activity(
                wallets, min_trades=1, days_back=1, max_wallets=2
            )

        assert len(result) <= 2

    async def test_batch_validate_empty_input(self, helius_client):
        """Empty input returns empty list."""
        result = await helius_client._batch_validate_activity([], min_trades=1)
        assert result == []


# ---------------------------------------------------------------------------
# Item 9 — Profitability pre-screen (analyzer)
# ---------------------------------------------------------------------------

class TestProfitabilityPreScreen:
    """Test the analyzer's _profitability_pre_screen method."""

    async def test_pre_screen_ranks_by_balance(self):
        from core.analyzer import WalletAnalyzer

        analyzer = WalletAnalyzer.__new__(WalletAnalyzer)
        analyzer.helius_client = MagicMock()
        analyzer.helius_client.get_wallet_sol_balances = AsyncMock(
            return_value={WALLET_A: 5.0, WALLET_B: 0.5, WALLET_C: 10.0}
        )
        analyzer._budget_manager = None  # Initialize budget manager attribute

        result = await analyzer._profitability_pre_screen(
            [WALLET_A, WALLET_B, WALLET_C], max_wallets=2
        )

        # Should keep top-2 by balance: C (10), A (5)
        assert WALLET_C in result
        assert WALLET_A in result
        assert WALLET_B not in result
        assert len(result) == 2

    async def test_pre_screen_fallback_on_error(self):
        from core.analyzer import WalletAnalyzer

        analyzer = WalletAnalyzer.__new__(WalletAnalyzer)
        analyzer.helius_client = MagicMock()
        analyzer.helius_client.get_wallet_sol_balances = AsyncMock(
            side_effect=Exception("API error")
        )
        analyzer._budget_manager = None  # Initialize budget manager attribute

        result = await analyzer._profitability_pre_screen(
            [WALLET_A, WALLET_B], max_wallets=1
        )

        # Should fall back to first N wallets
        assert result == [WALLET_A]
