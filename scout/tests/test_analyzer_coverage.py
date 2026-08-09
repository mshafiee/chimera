"""
Coverage-completion tests for core/analyzer.py.

Exercises every public method and error path of PortfolioTracker and
WalletAnalyzer with mocked network/DB boundaries.
"""

import asyncio
import base64
import builtins
import time
import importlib
import json
import os
import shutil
import struct
import sys
import types
from collections import OrderedDict
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from unittest.mock import AsyncMock, Mock, patch

import pytest

import core.analyzer as analyzer_mod
from core.analyzer import WalletAnalyzer, PortfolioTracker, _PARSE_CACHE_FAILURE
from core.wqs import WalletMetrics
from core.models import HistoricalTrade, TradeAction, LiquidityData, TraderArchetype
from core.decimal_utils import float_to_decimal


SOL_MINT = "So11111111111111111111111111111111111111112"
USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"


@pytest.fixture
def analyzer():
    """Real WalletAnalyzer with real HeliusClient (no network at init)."""
    return WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)


@pytest.fixture(autouse=True)
def _fake_to_thread(monkeypatch):
    """Run asyncio.to_thread bodies in-process.

    The real ThreadPoolExecutor trampoline makes the coverage C tracer stop
    recording the calling coroutine frame after the await (coverage.py bug),
    which silently drops lines. Running the callback directly keeps the trace
    intact; behavior is equivalent for the mocked boundaries used here.
    """

    async def fake_to_thread(fn, *args, **kwargs):
        return fn(*args, **kwargs)

    monkeypatch.setattr(analyzer_mod.asyncio, "to_thread", fake_to_thread)


def _make_trade(i, is_sell=False, token=None, days=None, **kw):
    base = dict(
        token_address=token or f"tok{i % 3}",
        token_symbol=f"TOK{i % 3}",
        action=TradeAction.SELL if is_sell else TradeAction.BUY,
        amount_sol=Decimal("1.0"),
        price_at_trade=Decimal("0.5"),
        timestamp=datetime.now(timezone.utc) - timedelta(days=i if days is None else days),
        tx_signature=f"tx{i}",
    )
    base.update(kw)
    return HistoricalTrade(**base)


def _swap_dict(sig="sig1", direction="BUY", token_mint="tokA",
               token_amount="100", sol_amount="1.0", **kw):
    d = {
        "signature": sig,
        "direction": direction,
        "token_mint": token_mint,
        "token_amount": token_amount,
        "sol_amount": sol_amount,
        "timestamp": int(datetime.now(timezone.utc).timestamp()),
    }
    d.update(kw)
    return d


def _tx(sig="sig1", tx_type="SWAP", source="JUPITER", **kw):
    d = {
        "signature": sig,
        "type": tx_type,
        "source": source,
        "instructions": [],
        "nativeTransfers": [],
        "tokenTransfers": [],
    }
    d.update(kw)
    return d


# ---------------------------------------------------------------------------
# PortfolioTracker static methods
# ---------------------------------------------------------------------------

class TestPortfolioTrackerPnl:
    def test_calculate_unrealized_pnl_basic(self):
        trades = [
            _make_trade(0, token="AAA", token_amount=Decimal("1000"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, token="AAA",
                        token_amount=Decimal("400"), sol_amount=Decimal("0.5"),
                        amount_sol=Decimal("0.5")),
        ]
        loss = PortfolioTracker.calculate_unrealized_pnl(
            trades, {"AAA": 0.0001}, sol_price_usd=100.0
        )
        assert loss >= 0.0

    def test_calculate_unrealized_pnl_token_amount_fallback(self):
        # token_amount missing -> derive from amount_sol / price_sol
        trades = [
            _make_trade(0, token="BBB", token_amount=None,
                        price_sol=Decimal("0.001"), amount_sol=Decimal("1.0")),
        ]
        loss = PortfolioTracker.calculate_unrealized_pnl(
            trades, {"BBB": 0.0}, sol_price_usd=100.0
        )
        # price unavailable -> assume worthless (100% loss of cost basis)
        assert loss == 1.0

    def test_calculate_unrealized_pnl_price_at_trade_fallback(self):
        trades = [
            _make_trade(0, token="CCC", token_amount=Decimal("0"),
                        price_at_trade=Decimal("0.01"), amount_sol=Decimal("2.0"),
                        price_sol=None),
        ]
        loss = PortfolioTracker.calculate_unrealized_pnl(trades, {"CCC": 0.0})
        assert loss == 2.0

    def test_calculate_unrealized_pnl_skips_unknowable_amount(self):
        trades = [
            _make_trade(0, token="DDD", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0"),
                        amount_sol=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_unrealized_pnl(trades, {"DDD": 1.0}) == 0.0

    def test_calculate_unrealized_pnl_dust_ignored(self):
        trades = [
            _make_trade(0, token="EEE", token_amount=Decimal("10"),
                        sol_amount=Decimal("0.01"), amount_sol=Decimal("0.01")),
        ]
        assert PortfolioTracker.calculate_unrealized_pnl(trades, {"EEE": 0.0}) == 0.0

    def test_calculate_unrealized_pnl_sell_without_position(self):
        trades = [
            _make_trade(0, is_sell=True, token="FFF", token_amount=Decimal("5"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_unrealized_pnl(trades, {"FFF": 1.0}) == 0.0

    def test_calculate_unrealized_pnl_heavy_bag(self):
        trades = [
            _make_trade(0, token="GGG", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        # value 2 USD < 20% of cost (100 USD) -> loss of 0.98 SOL
        assert PortfolioTracker.calculate_unrealized_pnl(
            trades, {"GGG": 0.02}, sol_price_usd=100.0
        ) == 0.98

    def test_calculate_unrealized_pnl_profitable_ignored(self):
        trades = [
            _make_trade(0, token="GGG2", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        # value 50 USD > 20% of cost (100 USD) -> no loss
        assert PortfolioTracker.calculate_unrealized_pnl(
            trades, {"GGG2": 0.5}, sol_price_usd=100.0
        ) == 0.0

    def test_calculate_paper_gains_basic(self):
        trades = [
            _make_trade(0, token="HHH", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        gain = PortfolioTracker.calculate_paper_gains(
            trades, {"HHH": 2.0}, sol_price_usd=100.0
        )
        # current value 200 USD vs cost 100 USD -> ratio 2.0 > 1.2 -> gain 1.0 SOL
        assert gain == 1.0

    def test_calculate_paper_gains_no_gain(self):
        trades = [
            _make_trade(0, token="III", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_paper_gains(
            trades, {"III": 0.005}, sol_price_usd=100.0
        ) == 0.0

    def test_calculate_paper_gains_fallback_paths(self):
        trades = [
            _make_trade(0, token="JJJ", token_amount=None,
                        price_sol=Decimal("0.01"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, token="JJJ", token_amount=None,
                        price_sol=Decimal("0.01"), amount_sol=Decimal("0.5")),
        ]
        assert PortfolioTracker.calculate_paper_gains(
            trades, {"JJJ": 0.02}, sol_price_usd=100.0
        ) >= 0.0


class TestFetchBulkPrices:
    @pytest.mark.asyncio
    async def test_empty_list(self):
        assert await PortfolioTracker.fetch_bulk_prices([]) == {}

    @pytest.mark.asyncio
    async def test_success(self):
        class FakeResponse:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            async def json(self):
                return {"tokA": {"usdPrice": 1.5}, "tokB": {"usdPrice": None}}

            def raise_for_status(self):
                return None

        class FakeSession:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            def get(self, url, timeout=None):
                return FakeResponse()

        class FakeClientSession(FakeSession):
            def __init__(self, *a, **kw):
                pass

        class FakeAiohttp:
            ClientSession = FakeClientSession
            ClientTimeout = lambda self, total=0: None
            ClientError = ConnectionError

        fake = types.ModuleType("aiohttp")
        fake.ClientSession = FakeClientSession
        fake.ClientTimeout = lambda total=0: None
        fake.ClientError = ConnectionError
        with patch.dict(sys.modules, {"aiohttp": fake}):
            prices = await PortfolioTracker.fetch_bulk_prices(["tokA", "tokB"])
        assert prices["tokA"] == 1.5
        assert prices["tokB"] == 0.0

    @pytest.mark.asyncio
    async def test_client_error(self):
        class Boom:
            def raise_for_status(self):
                raise ConnectionError("network down")

        class FakeResponse(Boom):
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

        class FakeSession:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            def get(self, url, timeout=None):
                return FakeResponse()

        fake = types.ModuleType("aiohttp")
        fake.ClientSession = FakeSession
        fake.ClientTimeout = lambda total=0: None
        fake.ClientError = ConnectionError
        with patch.dict(sys.modules, {"aiohttp": fake}):
            prices = await PortfolioTracker.fetch_bulk_prices(["tokX"])
        assert prices["tokX"] == 0.0

    @pytest.mark.asyncio
    async def test_parse_error(self):
        class FakeResponse:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            async def json(self):
                raise ValueError("bad json")

            def raise_for_status(self):
                return None

        class FakeSession:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            def get(self, url, timeout=None):
                return FakeResponse()

        fake = types.ModuleType("aiohttp")
        fake.ClientSession = FakeSession
        fake.ClientTimeout = lambda total=0: None
        fake.ClientError = ConnectionError
        with patch.dict(sys.modules, {"aiohttp": fake}):
            prices = await PortfolioTracker.fetch_bulk_prices(["tokY"])
        assert prices["tokY"] == 0.0


# ---------------------------------------------------------------------------
# WalletAnalyzer: lifecycle, budget, caches
# ---------------------------------------------------------------------------

class TestLifecycle:
    @pytest.mark.asyncio
    async def test_close_normal_and_error_paths(self, analyzer):
        await analyzer.close()

        analyzer.helius_client.close = AsyncMock(side_effect=RuntimeError("boom"))
        analyzer.liquidity_provider.close = AsyncMock(side_effect=RuntimeError("boom"))
        analyzer.rugcheck_client.close = AsyncMock(side_effect=RuntimeError("boom"))
        await analyzer.close()  # must not raise

    @pytest.mark.asyncio
    async def test_shutdown(self, analyzer):
        analyzer.helius_client.close = AsyncMock(side_effect=RuntimeError("boom"))
        analyzer.rugcheck_client.close = AsyncMock(side_effect=RuntimeError("boom"))
        analyzer.liquidity_provider.close = AsyncMock(side_effect=RuntimeError("boom"))
        await analyzer.shutdown()

    def test_can_spend_budget_no_manager(self, analyzer):
        assert analyzer.can_spend_budget(100) == (True, "No budget manager configured")

    def test_can_spend_budget_insufficient(self, analyzer):
        mgr = Mock()
        mgr.get_realtime_snapshot.return_value = Mock(credits_remaining=10,
                                                      alert_level=Mock(value="ok"))
        analyzer._budget_manager = mgr
        ok, reason = analyzer.can_spend_budget(100)
        assert not ok
        assert "Insufficient credits" in reason

    def test_can_spend_budget_critical_alert(self, analyzer):
        mgr = Mock()
        mgr.get_realtime_snapshot.return_value = Mock(credits_remaining=1000,
                                                      alert_level=Mock(value="critical"))
        analyzer._budget_manager = mgr
        ok, reason = analyzer.can_spend_budget(100)
        assert not ok
        assert "critical" in reason

    def test_can_spend_budget_ok(self, analyzer):
        mgr = Mock()
        mgr.get_realtime_snapshot.return_value = Mock(credits_remaining=1000,
                                                      alert_level=Mock(value="ok"))
        analyzer._budget_manager = mgr
        assert analyzer.can_spend_budget(100) == (True, "Budget OK")

    def test_can_spend_budget_exception(self, analyzer):
        mgr = Mock()
        mgr.get_realtime_snapshot.side_effect = RuntimeError("nope")
        analyzer._budget_manager = mgr
        ok, reason = analyzer.can_spend_budget(100)
        assert ok
        assert "Budget check failed" in reason

    def test_record_credit_usage_no_manager(self, analyzer):
        analyzer.record_credit_usage(50, "analysis", value=1.0)

    def test_record_credit_usage_success(self, analyzer):
        mgr = Mock()
        analyzer._budget_manager = mgr
        analyzer.record_credit_usage(50, "discovery", value=3.0)
        mgr.record_category_usage.assert_called_once()
        analyzer.record_credit_usage(10, "UNKNOWN_CATEGORY")

    def test_record_credit_usage_exception(self, analyzer):
        mgr = Mock()
        mgr.record_category_usage.side_effect = RuntimeError("budget error")
        analyzer._budget_manager = mgr
        analyzer.record_credit_usage(50, "analysis")

    def test_get_budget_summary_no_manager(self, analyzer):
        assert analyzer.get_budget_summary() == {"status": "No budget manager configured"}

    def test_get_budget_summary_ok(self, analyzer):
        mgr = Mock()
        mgr.get_daily_summary.return_value = {"credits": 100}
        analyzer._budget_manager = mgr
        assert analyzer.get_budget_summary() == {"credits": 100}

    def test_get_budget_summary_exception(self, analyzer):
        mgr = Mock()
        mgr.get_daily_summary.side_effect = RuntimeError("oops")
        analyzer._budget_manager = mgr
        assert "Error" in analyzer.get_budget_summary()["status"]

    @pytest.mark.asyncio
    async def test_cache_helpers_eviction(self, analyzer):
        await analyzer.clear_wallet_cache("some_addr")
        analyzer._metrics_cache_maxlen = 10
        for i in range(20):
            await analyzer._metrics_cache_set(f"k{i}", i)
        assert len(analyzer._metrics_cache) == 10
        await analyzer.clear_all_caches()
        assert not analyzer._metrics_cache
        assert not analyzer._trades_cache
        assert not analyzer._parse_cache

    def test_parse_cache_set_eviction(self, analyzer):
        for i in range(60):
            analyzer._parse_cache_set(f"k{i}", i, maxlen=50)
        assert len(analyzer._parse_cache) == 50

    def test_ordered_cache_set_eviction(self, analyzer):
        cache = OrderedDict()
        for i in range(60):
            analyzer._ordered_cache_set(cache, f"k{i}", i, maxlen=50)
        assert len(cache) == 50

    def test_redis_init_import_error(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "core.redis_client", None)
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        assert a._redis_client is None

    def test_redis_init_available(self, monkeypatch):
        class FakeRedis:
            def __init__(self, redis_url=None, enabled=True):
                pass

            def is_available(self):
                return True

        monkeypatch.setattr("core.redis_client.RedisClient", FakeRedis)
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_redis_enabled",
                            staticmethod(lambda: True))
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_redis_url",
                            staticmethod(lambda: "redis://x"))
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        assert a._redis_client is not None
        assert a.helius_client._redis is a._redis_client

    def test_redis_init_unavailable(self, monkeypatch):
        class FakeRedis:
            def __init__(self, redis_url=None, enabled=True):
                pass

            def is_available(self):
                return False

        monkeypatch.setattr("core.redis_client.RedisClient", FakeRedis)
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_redis_enabled",
                            staticmethod(lambda: True))
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        assert a._redis_client is None


# ---------------------------------------------------------------------------
# create / _async_init / discovery
# ---------------------------------------------------------------------------

class TestAsyncInit:
    @pytest.mark.asyncio
    async def test_create_loads_wallet_file(self, monkeypatch, tmp_path):
        wallet_file = tmp_path / "wallets.txt"
        wallet_file.write_text("# comment\n7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU\n\n"
                               "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890\n")
        monkeypatch.setenv("SCOUT_WALLET_LIST_FILE", str(wallet_file))
        a = await WalletAnalyzer.create(helius_api_key="test-key",
                                        discover_wallets=False, max_wallets=20)
        assert len(a._candidate_wallets) == 2
        await a.close()

    @pytest.mark.asyncio
    async def test_create_empty_wallet_file(self, monkeypatch, tmp_path):
        wallet_file = tmp_path / "empty.txt"
        wallet_file.write_text("")
        monkeypatch.setenv("SCOUT_WALLET_LIST_FILE", str(wallet_file))
        a = await WalletAnalyzer.create(helius_api_key="test-key",
                                        discover_wallets=False, max_wallets=20)
        assert a._candidate_wallets == []
        await a.close()

    @pytest.mark.asyncio
    async def test_create_unreadable_wallet_file(self, monkeypatch, tmp_path):
        # A directory exists -> open() raises IsADirectoryError
        wallet_dir = tmp_path / "wallet_dir"
        wallet_dir.mkdir()
        monkeypatch.setenv("SCOUT_WALLET_LIST_FILE", str(wallet_dir))
        a = await WalletAnalyzer.create(helius_api_key="test-key",
                                        discover_wallets=False, max_wallets=20)
        assert a._candidate_wallets == []
        await a.close()

    @pytest.mark.asyncio
    async def test_create_missing_wallet_file(self, monkeypatch):
        monkeypatch.setenv("SCOUT_WALLET_LIST_FILE", "/nonexistent/wallets.txt")
        a = await WalletAnalyzer.create(helius_api_key="test-key",
                                        discover_wallets=False, max_wallets=20)
        assert a._candidate_wallets == []
        await a.close()

    @pytest.mark.asyncio
    async def test_try_discover_wallets_async(self, analyzer):
        analyzer._discover_wallets = True
        analyzer._discover_with_multi_timeframe_system = AsyncMock()
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_enabled",
                   staticmethod(lambda: True)):
            await analyzer._try_discover_wallets_async()
        analyzer._discover_with_multi_timeframe_system.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_try_discover_manual_fallback(self, analyzer):
        analyzer._discover_wallets = True
        analyzer._discover_with_manual_implementation = AsyncMock()
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_enabled",
                   staticmethod(lambda: False)):
            await analyzer._try_discover_wallets_async()
        analyzer._discover_with_manual_implementation.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_try_discover_exception(self, analyzer, monkeypatch, capsys):
        analyzer._discover_wallets = True
        async def boom():
            raise RuntimeError("discovery failed")

        analyzer._discover_with_multi_timeframe_system = boom
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_enabled",
                   staticmethod(lambda: True)):
            await analyzer._try_discover_wallets_async()
        assert "Warning: Failed to discover wallets" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_try_discover_verbose_traceback(self, analyzer, monkeypatch):
        monkeypatch.setenv("SCOUT_VERBOSE", "true")
        analyzer._discover_wallets = True
        async def boom():
            raise RuntimeError("verbose failure")

        analyzer._discover_with_manual_implementation = boom
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_enabled",
                   staticmethod(lambda: False)):
            await analyzer._try_discover_wallets_async()

    def test_load_sample_data(self, analyzer):
        analyzer._load_sample_data()
        assert len(analyzer._candidate_wallets) == 5
        assert len(analyzer._metrics_cache) == 5
        assert len(analyzer._trades_cache) == 5

    def test_generate_sample_trades(self, analyzer):
        analyzer._load_sample_data()
        trades = analyzer._generate_sample_trades()
        assert trades
        for wallet_trades in trades.values():
            assert all(isinstance(t, HistoricalTrade) for t in wallet_trades)

    def test_get_candidate_wallets(self, analyzer):
        assert analyzer.get_candidate_wallets() == []


class TestMultiTimeframeDiscovery:
    def _make_result(self, ranked=True):
        tf_result = Mock()
        tf_result.wallets_discovered = ["w1", "w2"]
        tf_result.execution_time_seconds = 1.5
        tf_result.credits_consumed = 10
        result = Mock()
        result.cross_timeframe_ranking = (
            [("w1", 90.0), ("w2", 80.0)] if ranked else None
        )
        result.total_unique_wallets = 2
        result.deduplication_stats = {"deduplication_ratio": 0.5,
                                      "multi_timeframe_wallets": 1}
        result.total_execution_time_seconds = 3.0
        result.total_credits_consumed = 30
        from core.multitimeframe_discovery import DiscoveryTimeframe
        result.timeframe_results = {DiscoveryTimeframe.DEEP: tf_result}
        return result

    @pytest.mark.asyncio
    async def test_success_with_pre_screen(self, analyzer, monkeypatch):
        mt = Mock()
        mt.discover_all_timeframes = AsyncMock(return_value=self._make_result())
        monkeypatch.setattr("core.multitimeframe_discovery.get_multi_timeframe_discovery",
                            lambda helius_client=None: mt)
        analyzer._profitability_pre_screen = AsyncMock(
            return_value=["w2", "w1"])
        analyzer._max_wallets = 1
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_parallel",
                   staticmethod(lambda: True)), \
             patch("core.analyzer.ScoutConfig.get_multi_timeframe_goal",
                   staticmethod(lambda: "growth")), \
             patch("core.analyzer.ScoutConfig.get_discovery_profitability_filter",
                   staticmethod(lambda: True)), \
             patch("core.analyzer.ScoutConfig.get_max_api_calls_per_run",
                   staticmethod(lambda: 500)):
            await analyzer._discover_with_multi_timeframe_system()
        assert analyzer._candidate_wallets == ["w2", "w1"]

    @pytest.mark.asyncio
    async def test_success_no_pre_screen(self, analyzer, monkeypatch):
        mt = Mock()
        mt.discover_all_timeframes = AsyncMock(return_value=self._make_result())
        monkeypatch.setattr("core.multitimeframe_discovery.get_multi_timeframe_discovery",
                            lambda helius_client=None: mt)
        analyzer._max_wallets = 10
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_parallel",
                   staticmethod(lambda: True)), \
             patch("core.analyzer.ScoutConfig.get_discovery_profitability_filter",
                   staticmethod(lambda: False)):
            await analyzer._discover_with_multi_timeframe_system()
        assert analyzer._candidate_wallets == ["w1", "w2"]

    @pytest.mark.asyncio
    async def test_success_with_state_persistence(self, analyzer, monkeypatch):
        mt = Mock()
        mt.discover_all_timeframes = AsyncMock(return_value=self._make_result())
        monkeypatch.setattr("core.multitimeframe_discovery.get_multi_timeframe_discovery",
                            lambda helius_client=None: mt)
        persistence = Mock()
        persistence.save_multi_timeframe_discovery_stats = Mock()
        monkeypatch.setattr("core.analyzer.StatePersistence",
                            lambda: persistence)
        analyzer._max_wallets = 10
        with patch("core.analyzer.ScoutConfig.get_discovery_profitability_filter",
                   staticmethod(lambda: False)):
            await analyzer._discover_with_multi_timeframe_system()
        persistence.save_multi_timeframe_discovery_stats.assert_called_once()

    @pytest.mark.asyncio
    async def test_no_ranking_falls_through(self, analyzer, monkeypatch, capsys):
        mt = Mock()
        mt.discover_all_timeframes = AsyncMock(return_value=self._make_result(ranked=False))
        monkeypatch.setattr("core.multitimeframe_discovery.get_multi_timeframe_discovery",
                            lambda helius_client=None: mt)
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_parallel",
                   staticmethod(lambda: True)):
            await analyzer._discover_with_multi_timeframe_system()
        assert "No wallets discovered" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_exception_falls_back(self, analyzer, monkeypatch):
        mt = Mock()
        mt.discover_all_timeframes = AsyncMock(side_effect=RuntimeError("mt down"))
        monkeypatch.setattr("core.multitimeframe_discovery.get_multi_timeframe_discovery",
                            lambda helius_client=None: mt)
        with patch("core.analyzer.ScoutConfig.get_multi_timeframe_parallel",
                   staticmethod(lambda: True)):
            await analyzer._discover_with_multi_timeframe_system()


class TestManualDiscovery:
    @pytest.mark.asyncio
    async def test_budget_blocked(self, analyzer, monkeypatch, capsys):
        analyzer.can_spend_budget = Mock(return_value=(False, "Insufficient credits"))
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")
        await analyzer._discover_with_manual_implementation()
        assert "Skipping wallet discovery" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_success_no_pre_screen(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer._max_wallets = 10
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            return_value=["w1", "w2", "w3"]
        )
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")
        monkeypatch.setenv("SCOUT_DISCOVERY_HOURS", "168")
        monkeypatch.setenv("SCOUT_MIN_TRADE_COUNT", "3")
        await analyzer._discover_with_manual_implementation()
        assert analyzer._candidate_wallets == ["w1", "w2", "w3"]

    @pytest.mark.asyncio
    async def test_success_with_pre_screen(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer._max_wallets = 2
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            return_value=["w1", "w2", "w3", "w4", "w5"]
        )
        analyzer._profitability_pre_screen = AsyncMock(return_value=["w5", "w4"])
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "true")
        await analyzer._discover_with_manual_implementation()
        assert analyzer._candidate_wallets == ["w5", "w4"]

    @pytest.mark.asyncio
    async def test_timeout(self, analyzer, monkeypatch, capsys):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            side_effect=asyncio.TimeoutError()
        )
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")
        await analyzer._discover_with_manual_implementation()
        assert "timeout" in capsys.readouterr().out.lower()

    @pytest.mark.asyncio
    async def test_exception_falls_to_db(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            side_effect=RuntimeError("helius down")
        )
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")
        await analyzer._discover_with_manual_implementation()
        # No wallets, DB unreachable -> sample data loaded
        assert len(analyzer._candidate_wallets) == 5

    @pytest.mark.asyncio
    async def test_db_loaded_wallets(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            return_value=[]
        )
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")

        class FakeCursor:
            def execute(self, sql, params=None):
                self._is_table_check = "information_schema" in sql

            def fetchone(self):
                return [1] if getattr(self, "_is_table_check", False) else None

            def fetchall(self):
                return [{"address": "db_wallet_1"}, {"address": "db_wallet_2"}]

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        await analyzer._discover_with_manual_implementation()
        assert analyzer._candidate_wallets == ["db_wallet_1", "db_wallet_2"]

    @pytest.mark.asyncio
    async def test_db_error_falls_to_sample(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            return_value=[]
        )
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")

        class BrokenConn:
            def cursor(self):
                raise RuntimeError("db broken")

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: BrokenConn())
        await analyzer._discover_with_manual_implementation()
        assert len(analyzer._candidate_wallets) == 5

    @pytest.mark.asyncio
    async def test_no_api_key_sample_data(self, analyzer, monkeypatch):
        analyzer.helius_client.api_key = None
        analyzer._discover_wallets = True
        await analyzer._try_discover_wallets_async()
        assert len(analyzer._candidate_wallets) == 0


class TestProfitabilityPreScreen:
    @pytest.mark.asyncio
    async def test_empty(self, analyzer):
        assert await analyzer._profitability_pre_screen([], 5) == []

    @pytest.mark.asyncio
    async def test_budget_blocked(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(False, "no credits"))
        result = await analyzer._profitability_pre_screen(
            ["w1", "w2", "w3"], max_wallets=2)
        assert result == ["w1", "w2"]

    @pytest.mark.asyncio
    async def test_success(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_sol_balances = AsyncMock(
            return_value={"w1": 5.0, "w2": 0.0, "w3": 2.0}
        )
        result = await analyzer._profitability_pre_screen(
            ["w1", "w2", "w3"], max_wallets=2)
        assert result == ["w1", "w3"]

    @pytest.mark.asyncio
    async def test_exception(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.get_wallet_sol_balances = AsyncMock(
            side_effect=RuntimeError("helius down")
        )
        result = await analyzer._profitability_pre_screen(
            ["w1", "w2", "w3"], max_wallets=2)
        assert result == ["w1", "w2"]
# ---------------------------------------------------------------------------
# get_wallet_metrics
# ---------------------------------------------------------------------------

class TestGetWalletMetrics:
    @pytest.mark.asyncio
    async def test_cache_hit(self, analyzer):
        m = WalletMetrics(address="addr1")
        await analyzer._metrics_cache_set("addr1", m)
        assert await analyzer.get_wallet_metrics("addr1") is m

    @pytest.mark.asyncio
    async def test_db_row_fresh(self, analyzer, monkeypatch):
        row = {
            "wqs_score": 80.0, "roi_7d": 10.0, "roi_30d": 20.0,
            "trade_count_30d": 30, "win_rate": 0.7, "max_drawdown_30d": 5.0,
            "avg_trade_size_sol": 0.5,
            "last_trade_at": (datetime.now(timezone.utc) - timedelta(hours=2)).isoformat(),
        }

        class FakeCursor:
            def execute(self, sql, params=None):
                pass

            def fetchone(self):
                return row

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        metrics = await analyzer.get_wallet_metrics("addr_db")
        assert metrics is not None
        assert metrics.address == "addr_db"
        assert metrics.win_rate == 0.7

    @pytest.mark.asyncio
    async def test_db_row_stale_refetches(self, analyzer, monkeypatch):
        row = {
            "wqs_score": 80.0, "roi_7d": 10.0, "roi_30d": 20.0,
            "trade_count_30d": 30, "win_rate": 0.7, "max_drawdown_30d": 5.0,
            "avg_trade_size_sol": 0.5,
            "last_trade_at": (datetime.now(timezone.utc) - timedelta(days=45)).isoformat(),
        }

        class FakeCursor:
            def execute(self, sql, params=None):
                pass

            def fetchone(self):
                return row

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        analyzer._fetch_real_wallet_metrics = AsyncMock(return_value=None)
        metrics = await analyzer.get_wallet_metrics("addr_stale")
        assert metrics is None

    @pytest.mark.asyncio
    async def test_db_row_bad_date(self, analyzer, monkeypatch):
        row = {
            "wqs_score": 80.0, "roi_7d": 10.0, "roi_30d": 20.0,
            "trade_count_30d": 30, "win_rate": 0.7, "max_drawdown_30d": 5.0,
            "avg_trade_size_sol": 0.5, "last_trade_at": "not-a-date",
        }

        class FakeCursor:
            def execute(self, sql, params=None):
                pass

            def fetchone(self):
                return row

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        analyzer._fetch_real_wallet_metrics = AsyncMock(return_value=None)
        # Invalid date -> treated as non-stale -> row returned from DB
        metrics = await analyzer.get_wallet_metrics("addr_baddate")
        assert metrics is not None
        assert metrics.address == "addr_baddate"

    @pytest.mark.asyncio
    async def test_db_row_all_null(self, analyzer, monkeypatch):
        row = {"wqs_score": None, "roi_7d": None, "roi_30d": None,
               "trade_count_30d": None, "win_rate": None,
               "max_drawdown_30d": None, "avg_trade_size_sol": None,
               "last_trade_at": None}

        class FakeCursor:
            def execute(self, sql, params=None):
                pass

            def fetchone(self):
                return row

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        analyzer._fetch_real_wallet_metrics = AsyncMock(return_value=None)
        metrics = await analyzer.get_wallet_metrics("addr_nullrow")
        assert metrics is None

    @pytest.mark.asyncio
    async def test_db_error_falls_back(self, analyzer, monkeypatch):
        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection",
                            lambda db_path=None: (_ for _ in ()).throw(RuntimeError("db down")))
        analyzer._fetch_real_wallet_metrics = AsyncMock(return_value=None)
        metrics = await analyzer.get_wallet_metrics("addr_err")
        assert metrics is None

    @pytest.mark.asyncio
    async def test_fetch_success(self, analyzer):
        m = WalletMetrics(address="addr_fetch", roi_7d=5.0)
        analyzer._fetch_real_wallet_metrics = AsyncMock(return_value=m)
        metrics = await analyzer.get_wallet_metrics("addr_fetch")
        assert metrics is m

    @pytest.mark.asyncio
    async def test_fetch_exception_falls_back(self, analyzer):
        async def boom(address):
            raise RuntimeError("helius exploded")

        analyzer._fetch_real_wallet_metrics = boom
        metrics = await analyzer.get_wallet_metrics("addr_nofetch")
        assert metrics is None

    @pytest.mark.asyncio
    async def test_sample_fallback(self, analyzer):
        analyzer.helius_client.api_key = None
        await analyzer._metrics_cache_set("addr_sample", WalletMetrics(address="addr_sample"))
        metrics = await analyzer.get_wallet_metrics("addr_sample")
        assert metrics.address == "addr_sample"


# ---------------------------------------------------------------------------
# _fetch_real_wallet_metrics
# ---------------------------------------------------------------------------

def _wallet_metrics(**kw):
    base = dict(address="w", roi_7d=10.0, roi_30d=20.0, trade_count_30d=10,
                win_rate=0.6, max_drawdown_30d=5.0)
    base.update(kw)
    return WalletMetrics(**base)


class TestFetchRealWalletMetrics:
    @pytest.mark.asyncio
    async def test_budget_blocked(self, analyzer, capsys):
        analyzer.can_spend_budget = Mock(return_value=(False, "no credits"))
        assert await analyzer._fetch_real_wallet_metrics("w1") is None
        assert "Skipping wallet analysis" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_no_transactions(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.get_wallet_transactions = AsyncMock(return_value=[])
        assert await analyzer._fetch_real_wallet_metrics("w1") is None

    @pytest.mark.asyncio
    async def test_happy_path_with_parse_cache(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()

        tx1 = _tx("sig1", source="JUPITER")
        tx2 = _tx("sig2", source="RAYDIUM")
        tx3 = _tx("sig3", source="JUPITER_LIMIT")
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx1, tx1, tx2, tx3])
        analyzer.helius_client.parse_swap_transaction = Mock(
            side_effect=lambda tx, wallet_address=None: _swap_dict(tx["signature"]))

        trade = _make_trade(0)
        analyzer._parse_swap_to_trade = AsyncMock(return_value=trade)
        analyzer._calculate_metrics_from_trades = AsyncMock(
            return_value=_wallet_metrics(address="w1"))

        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        # sig1 parsed twice: first miss, then cache hit; sig2/sig3 miss
        assert analyzer._parse_cache_hits == 1
        assert analyzer._parse_cache_misses == 3
        assert analyzer._parse_stats["swaps_parsed"] == 4
        assert analyzer._parse_stats["trades_valid"] == 4
        # trades got cached for the backtester
        assert "w1" in analyzer._trades_cache

    @pytest.mark.asyncio
    async def test_parse_failures_cached(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()

        tx_bad = _tx("bad_sig", source="UNKNOWN")
        tx_ok = _tx("ok_sig")
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx_bad, tx_bad, tx_ok])
        analyzer.helius_client.parse_swap_transaction = Mock(
            side_effect=lambda tx, wallet_address=None:
                _swap_dict("ok_sig") if tx["signature"] == "ok_sig" else None)

        trade = _make_trade(0)
        analyzer._parse_swap_to_trade = AsyncMock(return_value=trade)
        analyzer._calculate_metrics_from_trades = AsyncMock(
            return_value=_wallet_metrics(address="w1"))

        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert analyzer._parse_stats["parse_failures_total"] == 2
        assert analyzer._parse_cache_misses == 3  # 2 bad sigs + 1 ok sig
        reason = analyzer._categorize_parse_failure(tx_bad, "w1")
        assert reason == "not_involved"

    @pytest.mark.asyncio
    async def test_no_valid_trades(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("sig_only")]
        )
        analyzer.helius_client.parse_swap_transaction = Mock(return_value=None)
        assert await analyzer._fetch_real_wallet_metrics("w1") is None
        assert analyzer._discovery_stats["wallets_with_no_trades"] == 1

    @pytest.mark.asyncio
    async def test_debug_dump_and_bot_detection(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        monkeypatch.setenv("SCOUT_DEBUG_TX_DUMP", "true")
        monkeypatch.setenv("SCOUT_DEBUG_PARSE_FAILURES", "true")

        analyzer.helius_client.KNOWN_BOT_ROUTERS = {"botprog"}
        tx1 = _tx("bot_tx", source="JUPITER", programId="botprog",
                  feePayer="botprog")
        tx_bad = _tx("bad_tx", source="UNKNOWN")
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx1, tx_bad] * 6)
        analyzer.helius_client.parse_swap_transaction = Mock(
            side_effect=lambda tx, wallet_address=None:
                _swap_dict("bot_tx") if tx["signature"] == "bot_tx" else None)
        trade = _make_trade(0)
        analyzer._parse_swap_to_trade = AsyncMock(return_value=trade)
        analyzer._calculate_metrics_from_trades = AsyncMock(
            return_value=_wallet_metrics(address="w1"))

        data_dir = os.path.join(os.path.dirname(os.path.dirname(
            os.path.abspath(analyzer_mod.__file__))), "data", "parse_failures")
        try:
            metrics = await analyzer._fetch_real_wallet_metrics("w1")
            assert metrics is not None
            assert analyzer._parse_stats["parse_failures_by_reason"]["not_involved"] == 6
            assert os.path.isdir(data_dir)
        finally:
            shutil.rmtree(data_dir, ignore_errors=True)

    @pytest.mark.asyncio
    async def test_mev_and_limit_order_detection(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        jito_tip = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4"

        tx_limit = _tx("limit_tx", source="JUPITER_LIMIT")
        tx_mev = _tx("mev_tx", source="JUPITER",
                     nativeTransfers=[{"toUserAccount": jito_tip, "amount": 0.001}],
                     tokenTransfers=[{}, {}, {}, {}])
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx_limit, tx_mev] * 5)
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("s"))
        trade = _make_trade(0)
        analyzer._parse_swap_to_trade = AsyncMock(return_value=trade)
        captured = {}

        async def fake_calc(address, trades, **kw):
            captured.update(kw)
            return _wallet_metrics(address=address)

        analyzer._calculate_metrics_from_trades = fake_calc
        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert captured["uses_limit_orders"] is True
        assert captured["uses_mev_protection"] is True
        assert captured["mev_risk_score"] is not None
        assert captured["dex_diversity_score"] == 2
        assert captured["is_tg_bot_user"] is False


# ---------------------------------------------------------------------------
# _parse_swap_to_trade
# ---------------------------------------------------------------------------

class TestParseSwapToTrade:
    @pytest.mark.asyncio
    async def test_bad_direction(self, analyzer):
        swap = _swap_dict(direction="HOLD")
        assert await analyzer._parse_swap_to_trade(swap, "wallet") is None

    @pytest.mark.asyncio
    async def test_missing_mint(self, analyzer):
        swap = _swap_dict(token_mint="")
        assert await analyzer._parse_swap_to_trade(swap, "wallet") is None

    @pytest.mark.asyncio
    async def test_bad_timestamp(self, analyzer):
        swap = _swap_dict(timestamp="not-a-number")
        assert await analyzer._parse_swap_to_trade(swap, "wallet") is None

    @pytest.mark.asyncio
    async def test_happy_path_with_liquidity(self, analyzer):
        analyzer.liquidity_provider.get_historical_liquidity_or_current = Mock(
            return_value=LiquidityData(
                token_address="tokA", liquidity_usd=Decimal("100000"),
                price_usd=Decimal("0.01"), volume_24h_usd=Decimal("50000"),
                timestamp=datetime.now(timezone.utc), source="mock",
            )
        )
        analyzer.liquidity_provider.get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer._get_token_symbol_async = AsyncMock(return_value="SYM")
        swap = _swap_dict(sig="s1", token_symbol="UNKNOWN", price_sol="0.01")
        trade = await analyzer._parse_swap_to_trade(swap, "wallet")
        assert trade is not None
        assert trade.token_symbol == "SYM"
        assert trade.liquidity_at_trade_usd == Decimal("100000")
        assert trade.price_usd == Decimal("1.0")

    @pytest.mark.asyncio
    async def test_usd_denominated_swap(self, analyzer):
        analyzer.liquidity_provider.get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.liquidity_provider.get_historical_liquidity_or_current = Mock(
            return_value=None)
        analyzer._get_token_symbol_async = AsyncMock(return_value=None)
        swap = _swap_dict(sig="s2", sol_amount=None, usd_amount="50.0",
                          token_amount="1000", token_symbol="TOKX")
        trade = await analyzer._parse_swap_to_trade(swap, "wallet")
        assert trade is not None
        assert trade.amount_sol == Decimal("0.5")

    @pytest.mark.asyncio
    async def test_price_usd_from_sol_price(self, analyzer):
        analyzer.liquidity_provider.get_sol_price_usd = AsyncMock(return_value=200.0)
        analyzer.liquidity_provider.get_historical_liquidity_or_current = Mock(
            side_effect=RuntimeError("liq error"))
        analyzer._get_token_symbol_async = AsyncMock(return_value="T")
        swap = _swap_dict(sig="s3", token_symbol="T", price_usd=None, price_sol="0.5")
        trade = await analyzer._parse_swap_to_trade(swap, "wallet")
        assert trade is not None
        assert trade.price_usd == Decimal("0.5") * Decimal("200")


# ---------------------------------------------------------------------------
# _get_token_symbol / _get_token_symbol_async
# ---------------------------------------------------------------------------

class TestTokenSymbol:
    @pytest.mark.asyncio
    async def test_empty_mint(self, analyzer):
        assert await analyzer._get_token_symbol("") is None

    @pytest.mark.asyncio
    async def test_redis_cache_hit(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps({"symbol": "BONK"})
        analyzer._redis_client = redis
        assert await analyzer._get_token_symbol("mint1") == "BONK"

    @pytest.mark.asyncio
    async def test_redis_error_then_memory_cache(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.side_effect = RuntimeError("redis down")
        analyzer._redis_client = redis
        async with analyzer._token_meta_cache_lock:
            analyzer._token_meta_cache["mint2"] = {"symbol": "WIF"}
        assert await analyzer._get_token_symbol("mint2") == "WIF"

    @pytest.mark.asyncio
    async def test_known_token(self, analyzer):
        mint = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        symbol = await analyzer._get_token_symbol(mint)
        assert symbol == "BONK"

    @pytest.mark.asyncio
    async def test_unknown_token(self, analyzer):
        assert await analyzer._get_token_symbol("unknown_mint_xyz") is None

    @pytest.mark.asyncio
    async def test_async_with_birdeye(self, analyzer):
        birdeye = Mock()
        birdeye.get_token_metadata = AsyncMock(
            return_value={"symbol": "NEWSYM"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        assert await analyzer._get_token_symbol_async("mint_new") == "NEWSYM"

    @pytest.mark.asyncio
    async def test_async_no_birdeye(self, analyzer):
        analyzer.liquidity_provider.birdeye_client = None
        assert await analyzer._get_token_symbol_async("mint_new2") is None

    @pytest.mark.asyncio
    async def test_async_birdeye_exception(self, analyzer):
        birdeye = Mock()
        birdeye.get_token_metadata = AsyncMock(side_effect=RuntimeError("birdeye down"))
        analyzer.liquidity_provider.birdeye_client = birdeye
        assert await analyzer._get_token_symbol_async("mint_new3") is None

    @pytest.mark.asyncio
    async def test_async_symbol_from_sync(self, analyzer):
        analyzer._get_token_symbol = AsyncMock(return_value="EXISTING")
        assert await analyzer._get_token_symbol_async("mint_new4") == "EXISTING"


# ---------------------------------------------------------------------------
# _replay_positions / _enrich_trades_with_realized_pnl
# ---------------------------------------------------------------------------

class TestReplayPositions:
    def test_swap_fields_basic(self):
        trades = [
            _make_trade(0, token="AAA", days=2, token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, token="AAA", days=1,
                        token_amount=Decimal("40"), sol_amount=Decimal("0.5"),
                        amount_sol=Decimal("0.5")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert cost_sold > 0
        assert "AAA" in positions

    def test_swap_fields_with_data_gap(self):
        trades = [
            _make_trade(0, token="BBB", days=2, token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
            # sell MORE than held -> mismatch
            _make_trade(1, is_sell=True, token="BBB", days=1,
                        token_amount=Decimal("150"), sol_amount=Decimal("1.5"),
                        amount_sol=Decimal("1.5")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert gap == 1.0
        assert cost_sold > 0

    def test_swap_fields_qty_fallback(self):
        trades = [
            _make_trade(0, token="CCC", days=2, token_amount=None,
                        price_at_trade=Decimal("0.5"), sol_amount=Decimal("1.0"),
                        amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, token="CCC", days=1,
                        token_amount=Decimal("1"), sol_amount=Decimal("0.6"),
                        amount_sol=Decimal("0.6")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert per_trade  # sell produced realized pnl
        assert positions["CCC"]["qty"] == 1  # 1 of 2 derived tokens sold

    def test_swap_fields_skip_unknown_qty(self):
        trades = [
            _make_trade(0, token="DDD", token_amount=None,
                        price_at_trade=Decimal("0"), sol_amount=Decimal("0"),
                        amount_sol=Decimal("0")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert positions == {}

    def test_legacy_fields(self):
        trades = [
            _make_trade(0, token="EEE", days=2, token_amount=None, sol_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("2.0"),
                        pnl_sol=Decimal("0.3")),
            _make_trade(1, is_sell=True, token="EEE", days=1, token_amount=None,
                        sol_amount=None, price_at_trade=Decimal("0.5"),
                        amount_sol=Decimal("1.0"), pnl_sol=Decimal("0.3")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert per_trade
        assert "EEE" in positions

    def test_legacy_fields_zero_values(self):
        trades = [
            _make_trade(0, token="FFF", token_amount=None, sol_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("0")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert positions == {}

    def test_sell_without_position(self):
        trades = [
            _make_trade(0, is_sell=True, token="GGG", token_amount=Decimal("5"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert positions == {}

    def test_enrich_no_swap_fields(self):
        trades = [
            _make_trade(0, token_amount=None, sol_amount=None, price_sol=None,
                        price_at_trade=Decimal("0.5")),
        ]
        result = analyzer_mod.WalletAnalyzer()._enrich_trades_with_realized_pnl(trades)
        assert result is trades

    def test_enrich_sets_pnl(self):
        trades = [
            _make_trade(0, token="HHH", days=2, token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, token="HHH", days=1,
                        token_amount=Decimal("50"), sol_amount=Decimal("0.7"),
                        amount_sol=Decimal("0.7"), pnl_sol=None),
        ]
        analyzer_mod.WalletAnalyzer()._enrich_trades_with_realized_pnl(trades)
        assert trades[1].pnl_sol is not None
# ---------------------------------------------------------------------------
# _fetch_token_creation_time
# ---------------------------------------------------------------------------

class _FakeSession:
    def __init__(self, status=200, payload=None):
        self._status = status
        self._payload = payload or {}

    async def __aenter__(self):
        return self

    async def __aexit__(self, *a):
        return False

    async def __call__(self, *a, **kw):
        return self

    def get(self, *a, **kw):
        return self

    def post(self, *a, **kw):
        return self

    @property
    def status(self):
        return self._status

    async def json(self):
        return self._payload


class _FakeAiohttpModule:
    ClientTimeout = lambda total=0: None
    ClientError = ConnectionError
    ClientSession = _FakeSession


class TestTokenCreationTime:
    @pytest.mark.asyncio
    async def test_empty_address(self, analyzer):
        assert await analyzer._fetch_token_creation_time("") is None

    @pytest.mark.asyncio
    async def test_redis_hit(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps(1234567890.0)
        analyzer._redis_client = redis
        assert await analyzer._fetch_token_creation_time("mint_red") == 1234567890.0

    @pytest.mark.asyncio
    async def test_redis_null_cached(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = json.dumps(None)
        analyzer._redis_client = redis
        assert await analyzer._fetch_token_creation_time("mint_rednull") is None

    @pytest.mark.asyncio
    async def test_redis_error(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.side_effect = RuntimeError("redis down")
        analyzer._redis_client = redis
        async with analyzer._token_creation_cache_lock:
            analyzer._token_creation_cache["mint_mem"] = 555.0
        assert await analyzer._fetch_token_creation_time("mint_mem") == 555.0

    @pytest.mark.asyncio
    async def test_birdeye_creation(self, analyzer):
        birdeye = Mock()
        birdeye.get_token_creation_info = AsyncMock(
            return_value={"blockUnixTime": "1700000000"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        ts = await analyzer._fetch_token_creation_time("mint_be")
        assert ts == 1700000000.0

    @pytest.mark.asyncio
    async def test_birdeye_exception_then_jupiter(self, analyzer, monkeypatch):
        birdeye = Mock()
        birdeye.get_token_creation_info = AsyncMock(
            side_effect=RuntimeError("birdeye down"))
        analyzer.liquidity_provider.birdeye_client = birdeye
        session = _FakeSession(
            payload={"data": {"mint_jup": {"extensions": {"created_at": "1700000001"}}}}
        )
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        ts = await analyzer._fetch_token_creation_time("mint_jup")
        assert ts == 1700000001.0

    @pytest.mark.asyncio
    async def test_own_session_creation(self, analyzer, monkeypatch):
        # No helius api key -> analyzer creates its own aiohttp session
        analyzer.helius_client.api_key = None
        session = _FakeSession(
            payload={"data": {"mint_own": {"extensions": {"creation_time": "1700000002"}}}}
        )
        monkeypatch.setitem(sys.modules, "aiohttp", _FakeAiohttpModule)
        monkeypatch.setattr(_FakeAiohttpModule, "ClientSession", lambda *a, **kw: session)
        ts = await analyzer._fetch_token_creation_time("mint_own")
        assert ts == 1700000002.0

    @pytest.mark.asyncio
    async def test_helius_first_tx_fallback(self, analyzer):
        analyzer.helius_client.get_token_first_tx_timestamp = AsyncMock(
            return_value=1700000003)
        ts = await analyzer._fetch_token_creation_time("mint_htx")
        assert ts == 1700000003.0
        assert analyzer._parse_stats["token_creation_fallback_helix"] == 1

    @pytest.mark.asyncio
    async def test_helius_first_tx_exception(self, analyzer):
        analyzer.helius_client.get_token_first_tx_timestamp = AsyncMock(
            side_effect=RuntimeError("helius down"))
        session = _FakeSession(payload=[{"created_at": "2024-01-01T00:00:00+00:00"}])
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        ts = await analyzer._fetch_token_creation_time("mint_meta")
        assert ts == 1704067200.0

    @pytest.mark.asyncio
    async def test_token_metadata_int_created(self, analyzer):
        session = _FakeSession(payload=[{"creation_time": 1700000004}])
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        ts = await analyzer._fetch_token_creation_time("mint_meta2")
        assert ts == 1700000004.0

    @pytest.mark.asyncio
    async def test_token_metadata_bad_date(self, analyzer):
        session = _FakeSession(payload=[{"created_at": "garbage-date"}])
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        ts = await analyzer._fetch_token_creation_time("mint_meta3")
        assert ts is None

    @pytest.mark.asyncio
    async def test_all_sources_fail(self, analyzer):
        analyzer._parse_stats["token_creation_failed"] = 0
        ts = await analyzer._fetch_token_creation_time("mint_fail")
        assert ts is None
        assert analyzer._parse_stats["token_creation_failed"] == 1
        assert analyzer._parse_stats["token_creation_fetched"] == 1

    @pytest.mark.asyncio
    async def test_redis_write_on_success(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        analyzer._redis_client = redis
        birdeye = Mock()
        birdeye.get_token_creation_info = AsyncMock(return_value={"txTime": "1700000005"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        ts = await analyzer._fetch_token_creation_time("mint_redisw")
        assert ts == 1700000005.0
        redis.set.assert_called()


# ---------------------------------------------------------------------------
# token safety
# ---------------------------------------------------------------------------

class TestTokenSafety:
    @pytest.mark.asyncio
    async def test_is_token_safe_empty(self, analyzer):
        assert await analyzer._is_token_safe("") is False

    @pytest.mark.asyncio
    async def test_is_token_safe_cache_hit(self, analyzer):
        analyzer._token_safety_cache = {"tok": (True, 1234.0)}
        analyzer._is_token_safe_uncached = AsyncMock()
        with patch("core.analyzer.time.time", return_value=1500.0):
            assert await analyzer._is_token_safe("tok") is True
        analyzer._is_token_safe_uncached.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_is_token_safe_cache_stale(self, analyzer):
        analyzer._token_safety_cache = {"tok": (True, 1000.0)}
        analyzer._is_token_safe_uncached = AsyncMock(return_value=False)
        with patch("core.analyzer.time.time", return_value=2000.0):
            assert await analyzer._is_token_safe("tok") is False
        assert analyzer._safety_check_total == 1
        assert analyzer._safety_check_failures == 1

    @pytest.mark.asyncio
    async def test_is_token_safe_counts(self, analyzer):
        analyzer._is_token_safe_uncached = AsyncMock(return_value=True)
        assert await analyzer._is_token_safe("tok_safe") is True
        assert analyzer._safety_check_total == 1
        assert analyzer._safety_check_failures == 0

    @pytest.mark.asyncio
    async def test_uncached_known_safe(self, analyzer):
        assert await analyzer._is_token_safe_uncached(SOL_MINT) is True
        assert await analyzer._is_token_safe_uncached(USDC_MINT) is True
        assert await analyzer._is_token_safe_uncached(
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB") is True

    @pytest.mark.asyncio
    async def test_uncached_no_api_key(self, analyzer):
        analyzer.helius_client.api_key = None
        assert await analyzer._is_token_safe_uncached("random_mint_1") is True

    @pytest.mark.asyncio
    async def test_uncached_standard_mint_no_freeze(self, analyzer):
        raw = bytearray(64)
        payload = {
            "result": {"value": {
                "data": [base64.b64encode(bytes(raw)).decode(), "base64"],
                "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            }}
        }
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        assert await analyzer._is_token_safe_uncached("mint_free") is True

    @pytest.mark.asyncio
    async def test_uncached_freeze_authority(self, analyzer):
        raw = bytearray(64)
        raw[46:50] = struct.pack("<I", 1)
        payload = {
            "result": {"value": {
                "data": [base64.b64encode(bytes(raw)).decode(), "base64"],
                "owner": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            }}
        }
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        assert await analyzer._is_token_safe_uncached("mint_freeze") is False

    @pytest.mark.asyncio
    async def test_uncached_token2022_risky_extension(self, analyzer, monkeypatch):
        raw = bytearray(200)
        raw[165:167] = struct.pack("<H", 14)  # TransferHook
        raw[167:169] = struct.pack("<H", 0)
        payload = {
            "result": {"value": {
                "data": [base64.b64encode(bytes(raw)).decode(), "base64"],
                "owner": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
            }}
        }
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        # allowlist excludes this mint
        fake_config = types.ModuleType("scout.config")
        fake_config.ScoutConfig = Mock()
        fake_config.ScoutConfig.get_token_2022_allowlist = staticmethod(lambda: [])
        fake_pkg = types.ModuleType("scout")
        fake_pkg.config = fake_config
        monkeypatch.setitem(sys.modules, "scout", fake_pkg)
        monkeypatch.setitem(sys.modules, "scout.config", fake_config)
        assert await analyzer._is_token_safe_uncached("mint_risky") is False

    @pytest.mark.asyncio
    async def test_uncached_token2022_allowlisted(self, analyzer, monkeypatch):
        raw = bytearray(200)
        raw[165:167] = struct.pack("<H", 14)
        payload = {
            "result": {"value": {
                "data": [base64.b64encode(bytes(raw)).decode(), "base64"],
                "owner": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
            }}
        }
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        fake_config = types.ModuleType("scout.config")
        fake_config.ScoutConfig = Mock()
        fake_config.ScoutConfig.get_token_2022_allowlist = staticmethod(
            lambda: ["mint_allow"])
        fake_pkg = types.ModuleType("scout")
        fake_pkg.config = fake_config
        monkeypatch.setitem(sys.modules, "scout", fake_pkg)
        monkeypatch.setitem(sys.modules, "scout.config", fake_config)
        assert await analyzer._is_token_safe_uncached("mint_allow") is True

    @pytest.mark.asyncio
    async def test_uncached_token2022_safe_extensions(self, analyzer, monkeypatch):
        raw = bytearray(200)
        raw[165:167] = struct.pack("<H", 3)  # safe extension type
        raw[167:169] = struct.pack("<H", 4)
        raw[173:175] = struct.pack("<H", 0)  # end sentinel
        payload = {
            "result": {"value": {
                "data": [base64.b64encode(bytes(raw)).decode(), "base64"],
                "owner": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
            }}
        }
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        fake_config = types.ModuleType("scout.config")
        fake_config.ScoutConfig = Mock()
        fake_config.ScoutConfig.get_token_2022_allowlist = staticmethod(lambda: [])
        fake_pkg = types.ModuleType("scout")
        fake_pkg.config = fake_config
        monkeypatch.setitem(sys.modules, "scout", fake_pkg)
        monkeypatch.setitem(sys.modules, "scout.config", fake_config)
        assert await analyzer._is_token_safe_uncached("mint_safeext") is True

    @pytest.mark.asyncio
    async def test_uncached_no_account_data_closed(self, analyzer, monkeypatch):
        payload = {"result": {"value": None}}
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        monkeypatch.delenv("SCOUT_SAFETY_FAIL_MODE", raising=False)
        assert await analyzer._is_token_safe_uncached("mint_missing") is False

    @pytest.mark.asyncio
    async def test_uncached_no_account_data_open(self, analyzer, monkeypatch):
        payload = {"result": {"value": None}}
        session = _FakeSession(payload=payload)
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        monkeypatch.setenv("SCOUT_SAFETY_FAIL_MODE", "open")
        assert await analyzer._is_token_safe_uncached("mint_missing2") is True

    @pytest.mark.asyncio
    async def test_uncached_non_200_closed(self, analyzer):
        session = _FakeSession(status=500, payload={})
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        assert await analyzer._is_token_safe_uncached("mint_500") is False

    @pytest.mark.asyncio
    async def test_uncached_non_200_open(self, analyzer, monkeypatch):
        session = _FakeSession(status=500, payload={})
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        monkeypatch.setenv("SCOUT_SAFETY_FAIL_MODE", "open")
        assert await analyzer._is_token_safe_uncached("mint_500b") is True

    @pytest.mark.asyncio
    async def test_uncached_exception_open(self, analyzer, monkeypatch):
        async def boom():
            raise RuntimeError("rpc down")

        analyzer.helius_client._get_session = boom
        monkeypatch.setenv("SCOUT_SAFETY_FAIL_MODE", "open")
        assert await analyzer._is_token_safe_uncached("mint_err") is True

    @pytest.mark.asyncio
    async def test_uncached_exception_closed(self, analyzer):
        async def boom():
            raise RuntimeError("rpc down")

        analyzer.helius_client._get_session = boom
        assert await analyzer._is_token_safe_uncached("mint_err2") is False

    def test_log_safety_health_summary_warns(self, caplog):
        analyzer = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        analyzer._safety_check_total = 100
        analyzer._safety_check_failures = 50
        analyzer.log_safety_health_summary()
        assert any("Token safety health" in r.message for r in caplog.records)

    def test_log_safety_health_summary_no_warn(self, analyzer, caplog):
        analyzer._safety_check_total = 100
        analyzer._safety_check_failures = 5
        analyzer.log_safety_health_summary()
        assert not any("Token safety health" in r.message for r in caplog.records)

    def test_log_safety_health_summary_zero(self, analyzer):
        analyzer._safety_check_total = 0
        analyzer.log_safety_health_summary()


# ---------------------------------------------------------------------------
# _get_sol_price_usd
# ---------------------------------------------------------------------------

class TestSolPrice:
    @pytest.mark.asyncio
    async def test_cached(self, analyzer):
        analyzer._sol_price_usd = 150.0
        assert await analyzer._get_sol_price_usd() == 150.0

    @pytest.mark.asyncio
    async def test_fetch_success(self, analyzer):
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={SOL_MINT: 175.0})):
            price = await analyzer._get_sol_price_usd()
        assert price == 175.0
        assert analyzer._sol_price_usd == 175.0

    @pytest.mark.asyncio
    async def test_fetch_error_sync_fallback(self, analyzer):
        async def boom(tokens):
            raise RuntimeError("price api down")

        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices", boom):
            price = await analyzer._get_sol_price_usd()
        assert price == 100.0

    @pytest.mark.asyncio
    async def test_sync_fallback_error_env(self, analyzer, monkeypatch):
        async def boom(tokens):
            raise RuntimeError("price api down")

        monkeypatch.setenv("SCOUT_SOL_FALLBACK_PRICE_USD", "250")
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices", boom):
            analyzer.liquidity_provider.get_sol_price_usd_sync = Mock(
                side_effect=RuntimeError("sync down"))
            price = await analyzer._get_sol_price_usd()
        assert price == 250.0


# ---------------------------------------------------------------------------
# archetype / hold time / insider patterns
# ---------------------------------------------------------------------------

class TestDetermineArchetype:
    def test_arbitrage(self, analyzer):
        m = _wallet_metrics(address="a", round_trip_ratio=0.75)
        assert analyzer.determine_archetype(m, []) == TraderArchetype.ARBITRAGE

    def test_arbitrage_config_threshold(self, analyzer, monkeypatch):
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_arb_round_trip_threshold_pct",
                            staticmethod(lambda: 0.4))
        m = _wallet_metrics(address="a", round_trip_ratio=0.5)
        assert analyzer.determine_archetype(m, []) == TraderArchetype.ARBITRAGE

    def test_insider(self, analyzer):
        m = _wallet_metrics(address="a", is_fresh_wallet=True)
        assert analyzer.determine_archetype(m, []) == TraderArchetype.INSIDER

    def test_whale(self, analyzer):
        m = _wallet_metrics(address="a", avg_trade_size_sol=Decimal("60"))
        assert analyzer.determine_archetype(m, []) == TraderArchetype.WHALE

    def test_sniper(self, analyzer):
        m = _wallet_metrics(address="a", avg_entry_delay_seconds=60.0)
        assert analyzer.determine_archetype(m, []) == TraderArchetype.SNIPER

    def test_swing(self, analyzer):
        m = _wallet_metrics(address="a", avg_entry_delay_seconds=1000.0)
        trades = [
            _make_trade(0, days=4, token="SSS", token_amount=Decimal("10"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=3, token="SSS",
                        token_amount=Decimal("10"), sol_amount=Decimal("1.0")),
        ]
        assert analyzer.determine_archetype(m, trades) == TraderArchetype.SWING

    def test_scalper_default(self, analyzer):
        m = _wallet_metrics(address="a", avg_entry_delay_seconds=1000.0)
        trades = [
            _make_trade(0, days=2, token="TTT", token_amount=Decimal("10"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=2, token="TTT",
                        token_amount=Decimal("10"), sol_amount=Decimal("1.0")),
        ]
        assert analyzer.determine_archetype(m, trades) == TraderArchetype.SCALPER


class TestAvgHoldTime:
    def test_empty(self, analyzer):
        assert analyzer._calculate_avg_hold_time([]) is None

    def test_fifo_hold_times(self, analyzer):
        trades = [
            _make_trade(0, days=5, token="AAA", token_amount=Decimal("10"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=4, token="AAA",
                        token_amount=Decimal("6"), sol_amount=Decimal("0.6")),
            _make_trade(2, is_sell=True, days=3, token="AAA",
                        token_amount=Decimal("4"), sol_amount=Decimal("0.4")),
        ]
        hold = analyzer._calculate_avg_hold_time(trades)
        assert hold is not None
        assert hold > 0

    def test_sell_without_position(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=1, token="BBB",
                        token_amount=Decimal("5"), sol_amount=Decimal("1.0")),
        ]
        assert analyzer._calculate_avg_hold_time(trades) is None

    def test_negative_hold_time_skipped(self, analyzer):
        # Sell BEFORE buy (negative hold) is not counted; still returns None
        trades = [
            _make_trade(0, days=1, token="CCC", token_amount=Decimal("5"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=2, token="CCC",
                        token_amount=Decimal("5"), sol_amount=Decimal("1.0")),
        ]
        assert analyzer._calculate_avg_hold_time(trades) is None


class TestInsiderPatterns:
    @pytest.mark.asyncio
    async def test_no_trades(self, analyzer):
        result = await analyzer._detect_insider_patterns("w1", [])
        assert result["is_fresh_wallet"] is False

    @pytest.mark.asyncio
    async def test_fresh_wallet_creation(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(
            return_value=time.time() - 3600)  # created 1h ago
        import time as _time
        trades = [_make_trade(0, days=1)]
        result = await analyzer._detect_insider_patterns("w1", trades)
        assert result["is_fresh_wallet"] is True
        assert result["suspicion_score"] == 100.0

    @pytest.mark.asyncio
    async def test_old_wallet(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(
            return_value=time.time() - 86400 * 100)  # created 100d ago
        trades = [_make_trade(0, days=2)]
        result = await analyzer._detect_insider_patterns("w1", trades)
        assert result["is_fresh_wallet"] is False

    @pytest.mark.asyncio
    async def test_no_creation_fallback_fresh(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)
        trades = [_make_trade(0, days=1)]  # first trade < 3 days ago
        result = await analyzer._detect_insider_patterns("w1", trades)
        assert result["is_fresh_wallet"] is True

    @pytest.mark.asyncio
    async def test_no_creation_fallback_old(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)
        trades = [_make_trade(0, days=10)]
        result = await analyzer._detect_insider_patterns("w1", trades)
        assert result["is_fresh_wallet"] is False

    @pytest.mark.asyncio
    async def test_token_creation_awareness(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)
        analyzer._token_creation_cache["tokA"] = (
            datetime.now(timezone.utc).timestamp() - 200)  # 200s before buy
        trades = [_make_trade(0, days=0, token="tokA")]
        result = await analyzer._detect_insider_patterns(
            "w1", trades, avg_entry_delay=60.0)
        assert result["is_fresh_wallet"] is True
        assert result["token_creation_awareness_ratio"] == 1.0

    @pytest.mark.asyncio
    async def test_token_creation_not_aware(self, analyzer):
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)
        analyzer._token_creation_cache["tokB"] = (
            datetime.now(timezone.utc).timestamp() - 3600 * 10)
        trades = [_make_trade(0, days=1, token="tokB")]
        result = await analyzer._detect_insider_patterns(
            "w1", trades, avg_entry_delay=60.0)
        assert result["token_creation_awareness_ratio"] == 0.0


class TestWalletCreationTime:
    @pytest.mark.asyncio
    async def test_cached(self, analyzer):
        async with analyzer._wallet_age_cache_lock:
            analyzer._wallet_age_cache["w1"] = 123.0
        analyzer.helius_client.get_wallet_first_transaction = AsyncMock()
        assert await analyzer._get_wallet_creation_time_cached("w1") == 123.0

    @pytest.mark.asyncio
    async def test_fetch(self, analyzer):
        analyzer.helius_client.get_wallet_first_transaction = AsyncMock(
            return_value=456.0)
        assert await analyzer._get_wallet_creation_time_cached("w2") == 456.0

    @pytest.mark.asyncio
    async def test_fetch_exception(self, analyzer):
        analyzer.helius_client.get_wallet_first_transaction = AsyncMock(
            side_effect=RuntimeError("boom"))
        assert await analyzer._get_wallet_creation_time_cached("w3") is None
# ---------------------------------------------------------------------------
# _calculate_metrics_from_trades
# ---------------------------------------------------------------------------

def _metric_trades(n=12, start_days=15, symbols=None, mint_suffix="", n_mints=3):
    """Realistic trades with swap fields; sells carry pnl_sol."""
    trades = []
    for i in range(n):
        is_sell = i % 2 == 1
        sym = (symbols or ["TOK0", "TOK1", "TOK2"])[i % max(1, len(symbols or ["TOK0"]))]
        trades.append(HistoricalTrade(
            token_address=f"mint_{i % max(1, n_mints)}{mint_suffix}",
            token_symbol=sym,
            action=TradeAction.SELL if is_sell else TradeAction.BUY,
            amount_sol=Decimal("1.0"),
            price_at_trade=Decimal("0.5"),
            timestamp=datetime.now(timezone.utc) - timedelta(days=start_days - i // 4),
            tx_signature=f"mtx{i}",
            token_amount=Decimal("100"),
            sol_amount=Decimal("1.0"),
            price_sol=Decimal("0.5"),
            pnl_sol=Decimal("0.05") if is_sell else None,
        ))
    return trades


class TestCalculateMetricsFromTrades:
    @pytest.mark.asyncio
    async def test_empty_trades(self, analyzer):
        assert await analyzer._calculate_metrics_from_trades("w1", []) is None

    @pytest.mark.asyncio
    async def test_happy_path(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={"mint_0": 0.5})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics is not None
        assert metrics.address == "w1"
        assert metrics.trade_count_30d > 0
        assert metrics.win_rate is not None
        assert metrics.profit_factor is not None

    @pytest.mark.asyncio
    async def test_scam_correlation(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value="funder")
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=False)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics.correlated_with_scam is True

    @pytest.mark.asyncio
    async def test_scam_check_disabled(self, analyzer, monkeypatch):
        monkeypatch.setenv("SCOUT_ENABLE_SCAM_CHECK", "false")
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics.correlated_with_scam is False

    @pytest.mark.asyncio
    async def test_scam_check_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(
            side_effect=RuntimeError("helius down"))
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics.correlated_with_scam is False

    @pytest.mark.asyncio
    async def test_known_scam_address(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics.correlated_with_scam is True

    @pytest.mark.asyncio
    async def test_rugcheck_all_safe(self, analyzer):
        rug = Mock()
        rug.is_token_safe = AsyncMock(return_value=True)
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_rugcheck_filters_all(self, analyzer):
        rug = Mock()
        rug.is_token_safe = AsyncMock(return_value=False)
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        # All trades filtered as risky -> empty trade set -> metrics computed
        assert metrics is not None
        assert metrics.trade_count_30d == 0

    @pytest.mark.asyncio
    async def test_rugcheck_circuit_breaker_closed(self, analyzer, monkeypatch):
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_rugcheck_fail_mode",
                            staticmethod(lambda: "closed"))
        rug = Mock()
        # 4 unique tokens, 3 risky -> ratio 0.75 > 0.5, >= 3 tokens -> breaker
        rug.is_token_safe = AsyncMock(side_effect=[True, False, False, False])
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        trades = _metric_trades(12)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_rugcheck_circuit_breaker_open(self, analyzer, monkeypatch):
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_rugcheck_fail_mode",
                            staticmethod(lambda: "open"))
        rug = Mock()
        rug.is_token_safe = AsyncMock(side_effect=[True, False, False, False])
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_rugcheck_small_sample(self, analyzer):
        rug = Mock()
        # Only 2 unique tokens; both risky but < 3 min for circuit breaker
        rug.is_token_safe = AsyncMock(side_effect=[False, False])
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        trades = _metric_trades(6, symbols=["A", "B"], n_mints=2)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_rugcheck_timeout(self, analyzer):
        rug = Mock()

        async def slow(token):
            raise asyncio.TimeoutError()

        rug.is_token_safe = slow
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_rugcheck_error(self, analyzer):
        rug = Mock()

        async def error(token):
            raise RuntimeError("rugcheck down")

        rug.is_token_safe = error
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_with_entry_delays_and_insider(self, analyzer):
        analyzer.rugcheck_client = None
        now = datetime.now(timezone.utc).timestamp()
        analyzer._token_creation_cache = {
            "mint_0": now - 60, "mint_1": now - 60, "mint_2": now - 60,
        }
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={"mint_0": 0.5})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics.avg_entry_delay_seconds is not None

    @pytest.mark.asyncio
    async def test_round_trip_detection(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        analyzer._detect_round_trip_ratio_from_transactions = Mock(return_value=0.3)
        txs = [_tx(f"r{i}") for i in range(10)]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=txs)
        assert metrics.round_trip_ratio == 0.3

    @pytest.mark.asyncio
    async def test_round_trip_detection_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)

        def boom(transactions, address):
            raise RuntimeError("round trip broke")

        analyzer._detect_round_trip_ratio_from_transactions = boom
        txs = [_tx(f"e{i}") for i in range(10)]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=txs)
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_pumpfun_concentration(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(mint_suffix="pump"), transactions=[])
        assert metrics.pumpfun_trade_ratio == 1.0

    @pytest.mark.asyncio
    async def test_bag_holder_penalty(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        # Old buy (40 days) with no sell -> bag penalty applied
        trades = [
            HistoricalTrade(
                token_address="bag_mint", token_symbol="BAG",
                action=TradeAction.BUY, amount_sol=Decimal("10.0"),
                price_at_trade=Decimal("0.5"),
                timestamp=datetime.now(timezone.utc) - timedelta(days=40),
                tx_signature="bag1", token_amount=Decimal("20"),
                sol_amount=Decimal("10.0"),
            ),
            *[_make_trade(2 + i, is_sell=i % 2 == 1, token="reg_mint",
                          token_amount=Decimal("100"), sol_amount=Decimal("1.0"))
              for i in range(8)],
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None


# ---------------------------------------------------------------------------
# compute_wallet_trade_stats / shared stats helpers
# ---------------------------------------------------------------------------

class TestWalletTradeStats:
    def test_empty(self, analyzer):
        result = analyzer.compute_wallet_trade_stats([])
        assert result["avg_win_sol"] is None

    def test_no_pnls(self, analyzer):
        trades = [_make_trade(0)]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["realized_pnl_30d_sol"] == Decimal(0)

    def test_wins_losses(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=1, pnl_sol=Decimal("0.5")),
            _make_trade(1, is_sell=True, days=2, pnl_sol=Decimal("-0.2")),
            _make_trade(2, is_sell=True, days=3, pnl_sol=Decimal("0.3")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["avg_win_sol"] == 0.4
        assert result["avg_loss_sol"] == 0.2
        assert result["profit_factor"] == 4.0
        assert result["realized_pnl_30d_sol"] == Decimal("0.6")

    def test_bag_penalty(self, analyzer):
        trades = [
            HistoricalTrade(
                token_address="oldbag", token_symbol="OLD",
                action=TradeAction.BUY, amount_sol=Decimal("1.0"),
                price_at_trade=Decimal("0.5"),
                timestamp=datetime.now(timezone.utc) - timedelta(days=40),
                tx_signature="ob1", token_amount=Decimal("100"),
                sol_amount=Decimal("1.0"),
            ),
            _make_trade(1, is_sell=True, days=1, pnl_sol=Decimal("0.5")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        # base PF 2.0 * (1 - 0.1) bag penalty
        assert result["profit_factor"] == 1.8

    def test_compute_base_profit_factor(self, analyzer):
        assert analyzer._compute_base_profit_factor(
            Decimal("10"), Decimal("0"), win_count=3) == 6.0
        assert analyzer._compute_base_profit_factor(
            Decimal("10"), Decimal("0"), win_count=100) == 100.0
        assert analyzer._compute_base_profit_factor(
            Decimal("0"), Decimal("0"), win_count=0) == 0.0
        assert analyzer._compute_base_profit_factor(
            Decimal("10"), Decimal("5"), win_count=5) == 2.0


class TestRoiWinRate:
    def test_roi_empty(self, analyzer):
        assert analyzer._calculate_roi_from_trades([]) == 0.0

    def test_roi_zero_cost(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=1, pnl_sol=Decimal("0.5"),
                        token_amount=Decimal("0"), sol_amount=Decimal("0")),
        ]
        assert analyzer._calculate_roi_from_trades(trades) == 0.0

    def test_roi_positive(self, analyzer):
        trades = [
            _make_trade(0, days=2, token="AAA", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="AAA",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.6")),
        ]
        roi = analyzer._calculate_roi_from_trades(trades)
        assert roi > 0

    def test_win_rate_empty(self, analyzer):
        assert analyzer._calculate_win_rate_from_trades([]) == 0.0

    def test_win_rate_no_closes(self, analyzer):
        trades = [_make_trade(0)]
        assert analyzer._calculate_win_rate_from_trades(trades) == 0.0

    def test_win_rate_mixed(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=1, pnl_sol=Decimal("0.5")),
            _make_trade(1, is_sell=True, days=2, pnl_sol=Decimal("-0.2")),
            _make_trade(2, is_sell=True, days=3, pnl_sol=None),
            _make_trade(3, is_sell=True, days=4, pnl_sol=Decimal("0.1")),
        ]
        assert analyzer._calculate_win_rate_from_trades(trades) == 2 / 3

    def test_win_rate_all_pnl_none(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=1, pnl_sol=None),
            _make_trade(1, is_sell=True, days=2, pnl_sol=None),
        ]
        assert analyzer._calculate_win_rate_from_trades(trades) == 0.0


class TestDecayHelpers:
    def test_alpha_decay_insufficient(self):
        trades = [_make_trade(i, is_sell=True, pnl_sol=Decimal("0.1")) for i in range(2)]
        assert WalletAnalyzer._calculate_alpha_decay(trades) is None

    def test_alpha_decay_all_neutral(self):
        trades = [_make_trade(i, is_sell=True, pnl_sol=Decimal("0")) for i in range(5)]
        assert WalletAnalyzer._calculate_alpha_decay(trades) is None

    def test_alpha_decay_zero_alltime(self):
        # Zero PnL sells are neither wins nor losses -> all_total == 0
        trades = [_make_trade(i, is_sell=True, pnl_sol=Decimal("0")) for i in range(5)]
        assert WalletAnalyzer._calculate_alpha_decay(trades) is None

    def test_alpha_decay_normal(self):
        trades = [_make_trade(i, is_sell=True, pnl_sol=Decimal("0.1")) for i in range(12)]
        ratio = WalletAnalyzer._calculate_alpha_decay(trades)
        assert ratio == 1.0

    def test_trade_size_decay_insufficient(self):
        trades = [_make_trade(i) for i in range(5)]
        assert WalletAnalyzer._calculate_trade_size_decay(trades) is None

    def test_trade_size_decay_time_split(self):
        trades = [
            _make_trade(i, days=10 - i, amount_sol=Decimal("1.0"))
            for i in range(8)
        ]
        ratio = WalletAnalyzer._calculate_trade_size_decay(trades)
        assert ratio == 1.0

    def test_trade_size_decay_count_split(self):
        trades = [_make_trade(i, amount_sol=Decimal("1.0")) for i in range(8)]
        ratio = WalletAnalyzer._calculate_trade_size_decay(trades)
        assert ratio == 1.0

    def test_trade_size_decay_zero_first(self):
        trades = [
            _make_trade(i, days=10 - i, amount_sol=Decimal("0"))
            for i in range(8)
        ]
        assert WalletAnalyzer._calculate_trade_size_decay(trades) is None

    def test_trade_size_decay_shrinking(self):
        trades = [
            _make_trade(i, days=10 - i, amount_sol=Decimal(str(10 - i)))
            for i in range(8)
        ]
        ratio = WalletAnalyzer._calculate_trade_size_decay(trades)
        assert 0.0 <= ratio < 1.0

    def test_token_rotation_insufficient(self):
        trades = [_make_trade(i) for i in range(9)]
        assert WalletAnalyzer._calculate_token_rotation_decay(trades) is None

    def test_token_rotation_normal(self):
        trades = [_make_trade(i, token=f"tok{i % 5}") for i in range(12)]
        ratio = WalletAnalyzer._calculate_token_rotation_decay(trades)
        assert ratio == 1.0

    def test_composite_no_components(self):
        assert WalletAnalyzer._calculate_composite_decay([]) is None

    def test_composite_with_components(self):
        trades = [
            _make_trade(i, is_sell=i % 2 == 1, days=20 - i,
                        pnl_sol=Decimal("0.1") if i % 2 == 1 else None,
                        token=f"tok{i % 5}", amount_sol=Decimal("1.0"))
            for i in range(20)
        ]
        score = WalletAnalyzer._calculate_composite_decay(trades)
        assert score is not None
        assert 0.0 <= score <= 1.0


class TestSurvivorshipAndCategory:
    def test_survivorship_empty(self):
        assert WalletAnalyzer._compute_survivorship_flag([]) == "UNKNOWN"

    def test_survivorship_30d(self):
        trades = [
            _make_trade(0, days=40),
            _make_trade(1, days=1),
        ]
        assert WalletAnalyzer._compute_survivorship_flag(trades) == "SURVIVED_30D"

    def test_survivorship_90d_age(self):
        trades = [_make_trade(0, days=5), _make_trade(1, days=1)]
        assert WalletAnalyzer._compute_survivorship_flag(
            trades, wallet_age_days=100) == "SURVIVED_90D"

    def test_survivorship_fresh(self):
        trades = [_make_trade(0, days=5), _make_trade(1, days=1)]
        assert WalletAnalyzer._compute_survivorship_flag(trades) == "FRESH_30D"

    def test_classify_token_category(self):
        assert WalletAnalyzer._classify_token_category("") is None
        assert WalletAnalyzer._classify_token_category("UNKNOWN") is None
        assert WalletAnalyzer._classify_token_category("WIF") == "memecoin"
        assert WalletAnalyzer._classify_token_category("JUP") == "infrastructure"
        assert WalletAnalyzer._classify_token_category("MNDE") == "defi"
        assert WalletAnalyzer._classify_token_category("USDC") == "stablecoin"
        assert WalletAnalyzer._classify_token_category("GALA") == "gaming"
        assert WalletAnalyzer._classify_token_category("GIGACOIN") == "memecoin"
        assert WalletAnalyzer._classify_token_category("WEIRDTOKEN") == "other"


class TestDrawdownAndStreak:
    def test_drawdown_empty(self, analyzer):
        assert analyzer._calculate_drawdown_from_trades([]) == 0.0

    def test_drawdown_recovery(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=3, pnl_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=2, pnl_sol=Decimal("-0.5")),
            _make_trade(2, is_sell=True, days=1, pnl_sol=Decimal("0.5")),
        ]
        dd = analyzer._calculate_drawdown_from_trades(trades)
        assert 0 < dd <= 100.0

    def test_drawdown_never_profitable(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=2, pnl_sol=Decimal("-0.5")),
            _make_trade(1, is_sell=True, days=1, pnl_sol=Decimal("-0.2")),
        ]
        assert analyzer._calculate_drawdown_from_trades(trades) == 100.0

    def test_drawdown_float_pnl(self, analyzer):
        trades = [
            _make_trade(0, is_sell=True, days=2, pnl_sol=0.5),
            _make_trade(1, is_sell=True, days=1, pnl_sol=-0.1),
        ]
        dd = analyzer._calculate_drawdown_from_trades(trades)
        assert dd > 0

    def test_streak_empty(self, analyzer):
        assert analyzer._calculate_win_streak_consistency([]) == 0.0

    def test_streak_too_few(self, analyzer):
        trades = [_make_trade(i, is_sell=True, pnl_sol=Decimal("0.1"))
                  for i in range(4)]
        assert analyzer._calculate_win_streak_consistency(trades) == 0.0

    def test_streak_alternating(self, analyzer):
        trades = [
            _make_trade(i, is_sell=True, days=i + 1,
                        pnl_sol=Decimal("0.1") if i % 2 == 0 else Decimal("-0.1"))
            for i in range(10)
        ]
        score = analyzer._calculate_win_streak_consistency(trades)
        assert 0.0 <= score <= 1.0

    def test_streak_all_wins(self, analyzer):
        trades = [_make_trade(i, is_sell=True, days=i + 1,
                              pnl_sol=Decimal("0.1")) for i in range(10)]
        score = analyzer._calculate_win_streak_consistency(trades)
        assert score == 1.0
# ---------------------------------------------------------------------------
# get_historical_trades / _fetch_real_historical_trades / fetch_recent_trades
# ---------------------------------------------------------------------------

class TestHistoricalTrades:
    @pytest.mark.asyncio
    async def test_cache_hit(self, analyzer):
        old = _make_trade(0, days=40)
        new = _make_trade(1, days=1)
        await analyzer._trades_cache_set("w1", [old, new])
        trades = await analyzer.get_historical_trades("w1", days=30)
        assert trades == [new]

    @pytest.mark.asyncio
    async def test_fetch_success(self, analyzer):
        analyzer._fetch_real_historical_trades = AsyncMock(
            return_value=[_make_trade(0, days=1)])
        trades = await analyzer.get_historical_trades("w1", days=30)
        assert len(trades) == 1
        assert "w1" in analyzer._trades_cache

    @pytest.mark.asyncio
    async def test_fetch_exception_fallback(self, analyzer):
        async def boom(address, days):
            raise RuntimeError("helius down")

        analyzer._fetch_real_historical_trades = boom
        await analyzer._trades_cache_set("w1", [_make_trade(0, days=1)])
        trades = await analyzer.get_historical_trades("w1", days=30)
        assert len(trades) == 1

    @pytest.mark.asyncio
    async def test_no_api_key_fallback(self, analyzer):
        analyzer.helius_client.api_key = None
        await analyzer._trades_cache_set("w1", [_make_trade(0, days=1)])
        trades = await analyzer.get_historical_trades("w1", days=30)
        assert len(trades) == 1

    @pytest.mark.asyncio
    async def test_fetch_real_budget_blocked(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(False, "no credits"))
        assert await analyzer._fetch_real_historical_trades("w1", 30) == []

    @pytest.mark.asyncio
    async def test_fetch_real_no_transactions(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(return_value=[])
        assert await analyzer._fetch_real_historical_trades("w1", 30) == []

    @pytest.mark.asyncio
    async def test_fetch_real_happy_path(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("h1"), _tx("h2")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("h1"))
        analyzer._parse_swap_to_trade = AsyncMock(
            return_value=_make_trade(0, days=1))
        analyzer.liquidity_provider.get_current_liquidity = Mock(
            return_value=LiquidityData(
                token_address="tok0", liquidity_usd=Decimal("50000"),
                price_usd=Decimal("0.01"), volume_24h_usd=Decimal("10000"),
                timestamp=datetime.now(timezone.utc), source="mock",
            ))
        analyzer.liquidity_provider.store_liquidity_batch = Mock(return_value=2)
        trades = await analyzer._fetch_real_historical_trades("w1", 30)
        assert len(trades) == 2
        analyzer.liquidity_provider.store_liquidity_batch.assert_called_once()

    @pytest.mark.asyncio
    async def test_fetch_real_liquidity_exception(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("h3")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("h3"))
        analyzer._parse_swap_to_trade = AsyncMock(
            return_value=_make_trade(0, days=1))
        analyzer.liquidity_provider.get_current_liquidity = Mock(
            side_effect=RuntimeError("liq down"))
        trades = await analyzer._fetch_real_historical_trades("w1", 30)
        assert len(trades) == 1  # liquidity failure is non-fatal

    @pytest.mark.asyncio
    async def test_fetch_real_store_batch_exception(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("h4")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("h4"))
        analyzer._parse_swap_to_trade = AsyncMock(
            return_value=_make_trade(0, days=1))
        analyzer.liquidity_provider.get_current_liquidity = Mock(
            return_value=LiquidityData(
                token_address="tok0", liquidity_usd=Decimal("50000"),
                price_usd=Decimal("0.01"), volume_24h_usd=Decimal("10000"),
                timestamp=datetime.now(timezone.utc), source="mock",
            ))
        analyzer.liquidity_provider.store_liquidity_batch = Mock(
            side_effect=RuntimeError("db down"))
        trades = await analyzer._fetch_real_historical_trades("w1", 30)
        assert len(trades) == 1

    @pytest.mark.asyncio
    async def test_fetch_real_enrich_exception(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("h5")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("h5"))
        analyzer._parse_swap_to_trade = AsyncMock(
            return_value=_make_trade(0, days=1))
        analyzer._enrich_trades_with_realized_pnl = Mock(
            side_effect=RuntimeError("enrich broke"))
        trades = await analyzer._fetch_real_historical_trades("w1", 30)
        assert len(trades) == 1

    @pytest.mark.asyncio
    async def test_fetch_recent_trades(self, analyzer):
        analyzer.get_historical_trades = AsyncMock(
            return_value=[_make_trade(0, days=1)])
        result = await analyzer.fetch_recent_trades("w1")
        assert result[0]["action"] == "BUY"
        assert "token_address" in result[0]


# ---------------------------------------------------------------------------
# _categorize_parse_failure
# ---------------------------------------------------------------------------

class TestCategorizeParseFailure:
    @pytest.fixture
    def analyzer(self):
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        a.helius_client.api_key = "test-key"
        return a

    def _wallet_tx(self, wallet="w1", **kw):
        tx = {
            "signature": "sig_cat",
            "type": "SWAP",
            "feePayer": wallet,
            "tokenTransfers": [],
            "nativeTransfers": [],
            "events": {},
            "instructions": [],
        }
        tx.update(kw)
        return tx

    def test_not_involved(self, analyzer):
        tx = self._wallet_tx(feePayer="someone_else")
        assert analyzer._categorize_parse_failure(tx, "w1") == "not_involved"

    def test_no_token_transfers(self, analyzer):
        tx = self._wallet_tx(events={})
        assert analyzer._categorize_parse_failure(tx, "w1") == "no_token_transfers"

    def test_events_malformed(self, analyzer):
        tx = self._wallet_tx(events={"swap": {"nativeInput": {"amount": 1}}})
        assert analyzer._categorize_parse_failure(tx, "w1") == "events_malformed"

    def test_events_empty(self, analyzer):
        tx = self._wallet_tx(events={"swap": {"unrelatedField": 1}})
        assert analyzer._categorize_parse_failure(tx, "w1") == "events_empty"

    def test_no_primary_token(self, analyzer):
        tx = self._wallet_tx(tokenTransfers=[
            {
                "mint": SOL_MINT, "fromUserAccount": "w1", "toUserAccount": "dex",
                "tokenAmount": "1000000",
            },
        ])
        assert analyzer._categorize_parse_failure(tx, "w1") == "no_primary_token"

    def test_direction_ambiguous(self, analyzer):
        # mintA bought and sold in equal amounts -> net delta zero
        tx = self._wallet_tx(tokenTransfers=[
            {
                "mint": "mintA", "fromUserAccount": "w1", "toUserAccount": "dex",
                "tokenAmount": "1000",
            },
            {
                "mint": "mintA", "fromUserAccount": "dex", "toUserAccount": "w1",
                "tokenAmount": "1000",
            },
        ])
        assert analyzer._categorize_parse_failure(tx, "w1") == "direction_ambiguous"

    def test_unknown(self, analyzer):
        # Non-SOL token delta present AND SOL movement present -> unknown
        tx = self._wallet_tx(tokenTransfers=[
            {
                "mint": "mintB", "fromUserAccount": "w1", "toUserAccount": "dex",
                "tokenAmount": "1000",
            },
            {
                "mint": SOL_MINT, "fromUserAccount": "dex", "toUserAccount": "w1",
                "tokenAmount": "500000",
            },
        ])
        assert analyzer._categorize_parse_failure(tx, "w1") == "unknown"


# ---------------------------------------------------------------------------
# dashboard / parse-rate health
# ---------------------------------------------------------------------------

class TestDashboard:
    def test_print_parse_health_dashboard(self, analyzer, capsys):
        analyzer.helius_client.get_discovery_stats = Mock(
            return_value={"infrastructure_filtered": 5, "balance_checked": 10,
                          "balance_filtered": 3})
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 25
        analyzer._parse_stats["trades_valid"] = 20
        analyzer._parse_stats["parse_failures_total"] = 60
        analyzer._parse_stats["parse_failures_by_reason"] = {
            "no_primary_token": 30, "direction_ambiguous": 20, "unknown": 10,
            "not_involved": 0, "other": 0,
        }
        analyzer._parse_stats["token_creation_fetched"] = 50
        analyzer._parse_stats["token_creation_success"] = 5
        analyzer._parse_stats["token_creation_fallback_helix"] = 2
        analyzer._parse_stats["token_creation_failed"] = 43
        analyzer._parse_cache_hits = 5
        analyzer._parse_cache_misses = 95
        analyzer.print_parse_health_dashboard()
        out = capsys.readouterr().out
        assert "Parse Health Dashboard" in out
        assert "CRITICAL" in out
        assert "Token Creation Time Quality" in out
        assert "Parse Cache Statistics" in out
        assert "WARNING: Low cache hit rate" in out

    def test_print_parse_health_dashboard_warning_level(self, analyzer, capsys, monkeypatch):
        monkeypatch.setenv("SCOUT_PARSE_HEALTH_WARN_PCT", "50")
        monkeypatch.setenv("SCOUT_PARSE_HEALTH_CRIT_PCT", "30")
        analyzer.helius_client.get_discovery_stats = Mock(return_value={})
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 40
        analyzer._parse_stats["parse_failures_total"] = 60
        analyzer._parse_stats["parse_failures_by_reason"] = {"unknown": 40}
        analyzer._parse_stats["token_creation_fetched"] = 0
        analyzer._parse_cache_hits = 0
        analyzer._parse_cache_misses = 0
        analyzer.print_parse_health_dashboard()
        assert "WARNING: Overall parse rate" in capsys.readouterr().out

    def test_print_parse_health_dashboard_no_failures(self, analyzer, capsys):
        analyzer.helius_client.get_discovery_stats = Mock(return_value={})
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 95
        analyzer._parse_stats["parse_failures_total"] = 5
        analyzer._parse_stats["parse_failures_by_reason"] = {"unknown": 5}
        analyzer._parse_stats["token_creation_fetched"] = 0
        analyzer._parse_cache_hits = 0
        analyzer._parse_cache_misses = 0
        analyzer.print_parse_health_dashboard()
        out = capsys.readouterr().out
        assert "Failure rate" in out

    def test_is_parse_rate_below_threshold_no_data(self, analyzer):
        analyzer._parse_stats["transactions_fetched"] = 0
        assert analyzer.is_parse_rate_below_threshold() is False

    def test_is_parse_rate_below_threshold_bad(self, analyzer, monkeypatch):
        monkeypatch.setenv("SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT", "40")
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 30
        assert analyzer.is_parse_rate_below_threshold() is True

    def test_is_parse_rate_below_threshold_good(self, analyzer):
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 80
        assert analyzer.is_parse_rate_below_threshold() is False

    def test_get_overall_parse_rate_no_data(self, analyzer):
        analyzer._parse_stats["transactions_fetched"] = 0
        assert analyzer.get_overall_parse_rate() == 1.0

    def test_get_overall_parse_rate(self, analyzer):
        analyzer._parse_stats["transactions_fetched"] = 100
        analyzer._parse_stats["swaps_parsed"] = 50
        assert analyzer.get_overall_parse_rate() == 0.5


# ---------------------------------------------------------------------------
# batch / prefetch error paths
# ---------------------------------------------------------------------------

class TestBatchErrorPaths:
    @pytest.mark.asyncio
    async def test_process_batch_gather_exception(self, analyzer, monkeypatch):
        async def fake_gather(*tasks, return_exceptions=False):
            return [ValueError("unexpected failure")]

        monkeypatch.setattr(analyzer_mod.asyncio, "gather", fake_gather)
        results = await analyzer._process_batch(["w1", "w2"], concurrency=1)
        assert results == {}

    @pytest.mark.asyncio
    async def test_process_batch_individual_failure(self, analyzer):
        async def boom(address):
            raise RuntimeError("wallet failed")

        analyzer.get_wallet_metrics = boom
        results = await analyzer._process_batch(["w_fail", "w_ok"], concurrency=2)
        assert results["w_fail"] is None

    @pytest.mark.asyncio
    async def test_prefetch_sol_price_failure(self, analyzer, monkeypatch):
        async def boom():
            raise RuntimeError("price down")

        analyzer._get_sol_price_usd = boom
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)
        await analyzer.prefetch_wallet_data(["w1"])

    @pytest.mark.asyncio
    async def test_prefetch_wallet_age_failure(self, analyzer, monkeypatch):
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)

        async def boom(address):
            raise RuntimeError("age down")

        analyzer._get_wallet_creation_time_cached = boom
        await analyzer.prefetch_wallet_data(["w1", "w2"])


# ---------------------------------------------------------------------------
# module import fallback guards
# ---------------------------------------------------------------------------

class TestImportFallback:
    def test_import_guards(self, monkeypatch):
        import core.analyzer as mod

        real_import = builtins.__import__

        def blocked(name, *args, **kwargs):
            if name in ("config", "core.state_persistence"):
                raise ImportError("blocked for test")
            return real_import(name, *args, **kwargs)

        # Force re-import: None in sys.modules makes __import__ raise/rebuild
        monkeypatch.setitem(sys.modules, "config", None)
        monkeypatch.setitem(sys.modules, "core.state_persistence", None)
        monkeypatch.setattr(builtins, "__import__", blocked)
        try:
            importlib.reload(mod)
            assert mod.SECURITY_AVAILABLE is False
            assert mod.STATE_PERSISTENCE_AVAILABLE is False
            assert mod.ScoutConfig is None
            assert mod.RugCheckClient is None
        finally:
            monkeypatch.setattr(builtins, "__import__", real_import)
            monkeypatch.delitem(sys.modules, "config")
            monkeypatch.delitem(sys.modules, "core.state_persistence")
            importlib.reload(mod)
            assert mod.SECURITY_AVAILABLE is True
            assert mod.STATE_PERSISTENCE_AVAILABLE is True
# ---------------------------------------------------------------------------
# remaining PortfolioTracker branches
# ---------------------------------------------------------------------------

class TestPortfolioTrackerExtra:
    def test_unrealized_pnl_sell_fallback(self):
        # SELL with token_amount None -> derive from price_sol
        trades = [
            _make_trade(0, days=2, token="S1", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="S1", token_amount=None,
                        price_sol=Decimal("0.01"), sol_amount=Decimal("0.5"),
                        amount_sol=Decimal("0.5")),
        ]
        loss = PortfolioTracker.calculate_unrealized_pnl(trades, {"S1": 0.0})
        assert loss == 0.5

    def test_unrealized_pnl_sell_fallback_price_at_trade(self):
        trades = [
            _make_trade(0, days=2, token="S2", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="S2", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0.01"),
                        sol_amount=Decimal("0.5"), amount_sol=Decimal("0.5")),
        ]
        loss = PortfolioTracker.calculate_unrealized_pnl(trades, {"S2": 0.0})
        assert loss == 0.5

    def test_unrealized_pnl_sell_skip_unresolvable(self):
        trades = [
            _make_trade(0, days=2, token="S3", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="S3", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0"),
                        sol_amount=Decimal("0.5"), amount_sol=Decimal("0.5")),
        ]
        # Sell skipped -> 100 tokens held at cost 1.0 SOL -> worthless -> 1.0
        assert PortfolioTracker.calculate_unrealized_pnl(trades, {"S3": 0.0}) == 1.0

    def test_unrealized_pnl_zero_qty_skipped(self):
        trades = [
            _make_trade(0, days=2, token="S4", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="S4", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(2, is_sell=True, days=0, token="S4", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_unrealized_pnl(trades, {"S4": 0.0}) == 0.0

    def test_paper_gains_buy_fallback(self):
        trades = [
            _make_trade(0, days=2, token="P1", token_amount=None,
                        price_sol=Decimal("0.01"), sol_amount=Decimal("1.0")),
        ]
        # 100 tokens at 0.02 USD -> value 2 vs cost 1 SOL -> 2x -> gain 1.0
        assert PortfolioTracker.calculate_paper_gains(trades, {"P1": 0.02}) == 1.0

    def test_paper_gains_sell_fallback(self):
        trades = [
            _make_trade(0, days=2, token="P2", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="P2", token_amount=None,
                        price_sol=Decimal("0.01"), sol_amount=Decimal("0.5"),
                        amount_sol=Decimal("0.5")),
        ]
        # remaining 50 tokens at 0.02 -> value 1.0 vs cost 0.5 SOL -> gain 0.5
        assert PortfolioTracker.calculate_paper_gains(trades, {"P2": 0.02}) == 0.5

    def test_paper_gains_sell_unresolvable(self):
        trades = [
            _make_trade(0, days=2, token="P3", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="P3", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0"),
                        sol_amount=Decimal("0.5"), amount_sol=Decimal("0.5")),
        ]
        gain = PortfolioTracker.calculate_paper_gains(trades, {"P3": 2.0})
        # 100 tokens * 2.0 = 200 vs cost 1.0 SOL -> gain 199
        assert gain == 199.0

    def test_paper_gains_sell_price_at_trade_fallback(self):
        trades = [
            _make_trade(0, days=2, token="P6", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="P6", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0.01"),
                        sol_amount=Decimal("0.5"), amount_sol=Decimal("0.5")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"P6": 0.02}) == 0.5

    def test_paper_gains_sell_unresolvable_qty(self):
        trades = [
            _make_trade(0, days=2, token="P7", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="P7", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0"),
                        sol_amount=Decimal("0.5"), amount_sol=Decimal("0.5")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"P7": 2.0}) == 199.0

    def test_paper_gains_dust_skipped(self):
        trades = [
            _make_trade(0, days=2, token="P4", token_amount=Decimal("10"),
                        sol_amount=Decimal("0.01"), amount_sol=Decimal("0.01")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"P4": 2.0}) == 0.0

    def test_paper_gains_sell_all(self):
        trades = [
            _make_trade(0, days=2, token="P5", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="P5", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"P5": 2.0}) == 0.0

    @pytest.mark.asyncio
    async def test_fetch_bulk_prices_bad_float(self):
        class FakeResponse:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            async def json(self):
                return {"tokX": {"usdPrice": "not-a-number"}}

            def raise_for_status(self):
                return None

        class FakeSession:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            def get(self, url, timeout=None):
                return FakeResponse()

        fake = types.ModuleType("aiohttp")
        fake.ClientSession = FakeSession
        fake.ClientTimeout = lambda total=0: None
        fake.ClientError = ConnectionError
        with patch.dict(sys.modules, {"aiohttp": fake}):
            prices = await PortfolioTracker.fetch_bulk_prices(["tokX"])
        assert prices["tokX"] == 0.0


# ---------------------------------------------------------------------------
# remaining analyzer misc branches
# ---------------------------------------------------------------------------

class TestMiscBranches:
    def test_rugcheck_init_failure(self, monkeypatch):
        class BrokenRug:
            def __init__(self):
                raise RuntimeError("no rugcheck api")

        monkeypatch.setattr("core.analyzer.RugCheckClient", BrokenRug)
        monkeypatch.setattr("core.analyzer.ScoutConfig.get_rugcheck_enabled",
                            staticmethod(lambda: True))
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        assert a.rugcheck_client is None

    @pytest.mark.asyncio
    async def test_trades_cache_eviction(self, analyzer):
        analyzer._trades_cache_maxlen = 10
        for i in range(20):
            await analyzer._trades_cache_set(f"k{i}", [i])
        assert len(analyzer._trades_cache) == 10

    @pytest.mark.asyncio
    async def test_manual_discovery_no_api_key(self, analyzer, monkeypatch):
        analyzer.helius_client.api_key = None
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.helius_client.discover_wallets_from_recent_swaps = AsyncMock(
            return_value=[])
        monkeypatch.setenv("SCOUT_DISCOVERY_PROFITABILITY_FILTER", "false")
        await analyzer._discover_with_manual_implementation()
        assert len(analyzer._candidate_wallets) == 5

    def test_generate_sample_trades_skips_unknown(self, analyzer):
        analyzer._candidate_wallets = ["known", "unknown"]
        analyzer._metrics_cache = OrderedDict({
            "known": WalletMetrics(address="known", trade_count_30d=5,
                                   win_rate=0.5, avg_trade_size_sol=Decimal("0.5")),
        })
        trades = analyzer._generate_sample_trades()
        assert "unknown" not in trades
        assert "known" in trades

    @pytest.mark.asyncio
    async def test_db_row_naive_timestamp(self, analyzer, monkeypatch):
        row = {
            "wqs_score": 80.0, "roi_7d": 10.0, "roi_30d": 20.0,
            "trade_count_30d": 30, "win_rate": 0.7, "max_drawdown_30d": 5.0,
            "avg_trade_size_sol": 0.5,
            "last_trade_at": (datetime.utcnow() - timedelta(hours=2)).isoformat(),
        }

        class FakeCursor:
            def execute(self, sql, params=None):
                pass

            def fetchone(self):
                return row

        class FakeConn:
            def cursor(self):
                return FakeCursor()

            def close(self):
                pass

        monkeypatch.setattr("core.db._is_postgres", lambda: True)
        monkeypatch.setattr("core.db.get_connection", lambda db_path=None: FakeConn())
        metrics = await analyzer.get_wallet_metrics("addr_naive")
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_progress_and_trade_failure(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        txs = [_tx(f"p{i}") for i in range(26)]
        analyzer.helius_client.get_wallet_transactions = AsyncMock(return_value=txs)
        analyzer.helius_client.parse_swap_transaction = Mock(
            side_effect=lambda tx, wallet_address=None: _swap_dict(tx["signature"]))
        trade = _make_trade(0)
        analyzer._parse_swap_to_trade = AsyncMock(
            side_effect=lambda swap, wallet: trade if swap.get("signature") == "p0" else None)
        analyzer._calculate_metrics_from_trades = AsyncMock(
            return_value=_wallet_metrics(address="w1"))
        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert analyzer._parse_stats["trades_valid"] == 1
        assert analyzer._parse_stats["swaps_parsed"] == 26

    @pytest.mark.asyncio
    async def test_metrics_calc_none(self, analyzer, capsys):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("n1")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("n1"))
        analyzer._parse_swap_to_trade = AsyncMock(return_value=_make_trade(0))
        analyzer._calculate_metrics_from_trades = AsyncMock(return_value=None)
        assert await analyzer._fetch_real_wallet_metrics("w1") is None
        assert "✗" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_parse_swap_usd_conversion_exception(self, analyzer):
        analyzer.liquidity_provider.get_sol_price_usd = AsyncMock(
            side_effect=RuntimeError("no price"))
        analyzer._get_token_symbol_async = AsyncMock(return_value=None)
        swap = _swap_dict(sig="usd1", sol_amount=None, usd_amount="50.0",
                          token_amount="1000", token_symbol="X")
        trade = await analyzer._parse_swap_to_trade(swap, "wallet")
        assert trade is not None
        assert trade.amount_sol == Decimal("0")

    @pytest.mark.asyncio
    async def test_get_token_symbol_known_with_redis(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        analyzer._redis_client = redis
        mint = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        assert await analyzer._get_token_symbol(mint) == "BONK"
        redis.set.assert_called()

    @pytest.mark.asyncio
    async def test_get_token_symbol_unknown_with_redis(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        analyzer._redis_client = redis
        assert await analyzer._get_token_symbol("mystery_mint_1") is None
        redis.set.assert_called()

    @pytest.mark.asyncio
    async def test_get_token_symbol_async_birdeye_redis(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        analyzer._redis_client = redis
        birdeye = Mock()
        birdeye.get_token_metadata = AsyncMock(return_value={"symbol": "NEW1"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        assert await analyzer._get_token_symbol_async("mint_redis_new") == "NEW1"
        redis.set.assert_called()

    def test_replay_tiny_sell_skipped(self):
        trades = [
            _make_trade(0, days=2, token="TINY", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="TINY",
                        token_amount=Decimal("1e-12"), sol_amount=Decimal("1e-12")),
        ]
        cost_sold, pnl, positions, per_trade, gap = (
            analyzer_mod.WalletAnalyzer._replay_positions(trades))
        assert not per_trade
        assert positions["TINY"]["qty"] == Decimal("100")

    @pytest.mark.asyncio
    async def test_token_creation_birdeye_verbose(self, analyzer, monkeypatch):
        monkeypatch.setenv("SCOUT_VERBOSE", "true")
        birdeye = Mock()
        birdeye.get_token_creation_info = AsyncMock(
            side_effect=RuntimeError("birdeye down"))
        analyzer.liquidity_provider.birdeye_client = birdeye
        analyzer.helius_client.get_token_first_tx_timestamp = AsyncMock(
            return_value=None)
        analyzer.helius_client._get_session = AsyncMock(
            side_effect=RuntimeError("session down"))
        assert await analyzer._fetch_token_creation_time("mint_v") is None

    @pytest.mark.asyncio
    async def test_token_creation_jupiter_bad_ts(self, analyzer):
        session = _FakeSession(
            payload={"data": {"mint_badts": {"extensions": {"created_at": "abc"}}}}
        )
        analyzer.helius_client._get_session = AsyncMock(return_value=session)
        analyzer.helius_client.get_token_first_tx_timestamp = AsyncMock(
            return_value=None)
        assert await analyzer._fetch_token_creation_time("mint_badts") is None

    @pytest.mark.asyncio
    async def test_token_creation_redis_write_failure(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        redis.set.side_effect = RuntimeError("redis full")
        analyzer._redis_client = redis
        birdeye = Mock()
        birdeye.get_token_creation_info = AsyncMock(return_value={"txTime": "123"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        assert await analyzer._fetch_token_creation_time("mint_rwf") == 123.0

    @pytest.mark.asyncio
    async def test_is_token_safe_reinitializes_counters(self, analyzer):
        del analyzer._safety_check_total
        del analyzer._safety_check_failures
        analyzer._is_token_safe_uncached = AsyncMock(return_value=True)
        assert await analyzer._is_token_safe("tok_reinit") is True
        assert analyzer._safety_check_total == 1

    @pytest.mark.asyncio
    async def test_is_token_safe_uncached_empty(self, analyzer):
        assert await analyzer._is_token_safe_uncached("") is False

    def test_round_trip_real_method(self, analyzer):
        # insufficient transactions -> 0.0
        assert analyzer._detect_round_trip_ratio_from_transactions([], "w") == 0.0
        assert analyzer._detect_round_trip_ratio_from_transactions(
            [_tx("a"), _tx("b")], "w") == 0.0

    def test_round_trip_skips_non_swap(self, analyzer):
        txs = [_tx("a", tx_type="TRANSFER"), _tx("b", tx_type="SWAP"),
               _tx("c", tx_type="SWAP")]
        assert analyzer._detect_round_trip_ratio_from_transactions(txs, "w") == 0.0

    def test_round_trip_analyzed_zero(self, analyzer):
        txs = [_tx("a", tx_type="TRANSFER")] * 4
        assert analyzer._detect_round_trip_ratio_from_transactions(txs, "w") == 0.0

    def test_round_trip_amount_parsing(self, analyzer):
        txs = [
            _tx("r1", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintA", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": "not-a-number"},
            ]),
            _tx("r2", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintB", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 123},
            ]),
            _tx("r3", tx_type="SWAP", tokenTransfers=[
                {"mint": "", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 5},
            ]),
        ]
        assert analyzer._detect_round_trip_ratio_from_transactions(txs, "w") == 0.0

    def test_round_trip_detected(self, analyzer, capsys):
        txs = [
            _tx("rt1", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintA", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 100},
                {"mint": "mintA", "fromUserAccount": "d", "toUserAccount": "w",
                 "tokenAmount": 100},
            ]),
            _tx("rt2", tx_type="SWAP", tokenTransfers=[
                {"mint": SOL_MINT, "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 100},
                {"mint": SOL_MINT, "fromUserAccount": "d", "toUserAccount": "w",
                 "tokenAmount": 100},
            ]),
            _tx("rt3", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintB", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 50},
            ]),
        ]
        ratio = analyzer._detect_round_trip_ratio_from_transactions(txs, "w")
        assert ratio == 1 / 3
        assert "Round-trip detected" in capsys.readouterr().out


class TestMetricsCalcExtra:
    @pytest.mark.asyncio
    async def test_risky_ratio_low_filters_individual(self, analyzer):
        rug = Mock()
        rug.is_token_safe = AsyncMock(side_effect=[True, True, True, False])
        analyzer.rugcheck_client = rug
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(8, n_mints=4), transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_enrich_failure_returns_none(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._enrich_trades_with_realized_pnl = Mock(
            side_effect=RuntimeError("enrich crash"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is None

    @pytest.mark.asyncio
    async def test_win_rate_failure_returns_none(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._enrich_trades_with_realized_pnl = Mock(return_value=None)
        analyzer._calculate_win_rate_from_trades = Mock(
            side_effect=RuntimeError("wr crash"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is None

    @pytest.mark.asyncio
    async def test_drawdown_failure_returns_none(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._enrich_trades_with_realized_pnl = Mock(return_value=None)
        analyzer._calculate_win_rate_from_trades = Mock(return_value=0.5)
        analyzer._calculate_drawdown_from_trades = Mock(
            side_effect=RuntimeError("dd crash"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is None

    @pytest.mark.asyncio
    async def test_bag_position_qty_fallback_zeros(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            # BUY with no resolvable qty -> qty 0, skipped
            _make_trade(0, days=3, token="Z1", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("1.0"),
                        sol_amount=Decimal("1.0")),
            # SELL with no resolvable qty -> qty 0, skipped
            _make_trade(1, is_sell=True, days=2, token="Z1", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("1.0"),
                        sol_amount=Decimal("1.0"), pnl_sol=Decimal("0.1")),
            *[_make_trade(2 + i, is_sell=i % 2 == 1, token="reg",
                          token_amount=Decimal("100"), sol_amount=Decimal("1.0"),
                          pnl_sol=Decimal("0.05") if i % 2 == 1 else None)
              for i in range(8)],
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_five_unique_tokens_break(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            _make_trade(i, token=f"uniq{i}", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0"),
                        pnl_sol=Decimal("0.05") if i % 2 == 1 else None)
            for i in range(6)
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_insider_detection_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._detect_insider_patterns = AsyncMock(
            side_effect=RuntimeError("insider crash"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None
        assert metrics.is_fresh_wallet is False

    @pytest.mark.asyncio
    async def test_replay_gap_print(self, analyzer, capsys):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            _make_trade(0, days=2, token="GAP", token_amount=Decimal("50"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="GAP",
                        token_amount=Decimal("100"), sol_amount=Decimal("2.0"),
                        pnl_sol=Decimal("0.5")),
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None
        assert "FIFO replay data gap ratio" in capsys.readouterr().out

    @pytest.mark.asyncio
    async def test_unrealized_pnl_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(side_effect=RuntimeError("no price"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None
        assert metrics.total_unrealized_loss_sol is None

    @pytest.mark.asyncio
    async def test_sortino_exception(self, analyzer):
        class EvilDecimal(Decimal):
            def __gt__(self, other):
                raise TypeError("no comparison allowed")

        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            _make_trade(0, days=2, token="STR", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="STR",
                        token_amount=Decimal("50"), sol_amount=EvilDecimal("1.0"),
                        pnl_sol=Decimal("0.05")),
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None
        assert metrics.sortino_ratio is None

    @pytest.mark.asyncio
    async def test_sortino_zero_cost_basis(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            _make_trade(0, days=2, token="ZC", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            # sol_amount == pnl_sol -> cost basis zero -> return 0.0
            _make_trade(1, is_sell=True, days=1, token="ZC",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.05"),
                        pnl_sol=Decimal("0.05")),
            _make_trade(2, is_sell=True, days=0, token="ZC",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.08"),
                        pnl_sol=Decimal("0.04")),
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_sortino_zero_cost_basis_unmatched_sell(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        # SELL before any BUY -> not matched by FIFO replay -> pnl_sol stays as
        # given; sol_amount == pnl_sol -> cost basis zero -> _infer_return 0.0
        trades = [
            _make_trade(0, is_sell=True, days=3, token="UC",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.05"),
                        pnl_sol=Decimal("0.05")),
            _make_trade(1, days=2, token="UC", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(2, is_sell=True, days=1, token="UC",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.08"),
                        pnl_sol=Decimal("0.04")),
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None

    @pytest.mark.asyncio
    async def test_archetype_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer.determine_archetype = Mock(side_effect=RuntimeError("arch crash"))
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", _metric_trades(), transactions=[])
        assert metrics is not None
        assert metrics.archetype is None

    @pytest.mark.asyncio
    async def test_trajectory_exception(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        import core.wqs as _wqs_mod
        with patch.object(_wqs_mod, "_interpret_trajectory",
                          Mock(side_effect=RuntimeError("traj crash"))), \
             patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", _metric_trades(), transactions=[])
        assert metrics is not None
        assert metrics.trajectory is None


class TestTradeStatsExtra:
    def test_buy_qty_fallback(self, analyzer):
        trades = [
            _make_trade(0, days=2, token="Q1", token_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="Q1", token_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("0.5"),
                        pnl_sol=Decimal("0.1")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["realized_pnl_30d_sol"] == Decimal("0.1")

    def test_buy_qty_zero_skipped(self, analyzer):
        trades = [
            _make_trade(0, days=2, token="Q2", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="Q2", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("0.5"),
                        pnl_sol=Decimal("0.1")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["realized_pnl_30d_sol"] == Decimal("0.1")

    def test_sell_qty_fallback(self, analyzer):
        trades = [
            _make_trade(0, days=2, token="Q3", token_amount=Decimal("100"),
                        amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="Q3", token_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("0.5"),
                        pnl_sol=Decimal("0.1")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["realized_pnl_30d_sol"] == Decimal("0.1")

    def test_sell_qty_zero_skipped(self, analyzer):
        trades = [
            _make_trade(0, days=2, token="Q4", token_amount=Decimal("100"),
                        amount_sol=Decimal("1.0")),
            _make_trade(1, is_sell=True, days=1, token="Q4", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("0.5"),
                        pnl_sol=Decimal("0.1")),
        ]
        result = analyzer.compute_wallet_trade_stats(trades)
        assert result["realized_pnl_30d_sol"] == Decimal("0.1")


class TestAlphaDecayRecentZero:
    def test_recent_total_zero(self):
        trades = [
            _make_trade(i, is_sell=True, days=i + 1,
                        pnl_sol=Decimal("0.1") if i >= 10 else Decimal("0"))
            for i in range(12)
        ]
        assert WalletAnalyzer._calculate_alpha_decay(trades) is None


class TestBatchAnalyze:
    @pytest.mark.asyncio
    async def test_analyze_wallets_batch_basic(self, analyzer, monkeypatch):
        async def fake_metrics(address):
            return _wallet_metrics(address=address)

        analyzer.get_wallet_metrics = fake_metrics
        results = await analyzer.analyze_wallets_batch(
            ["w1", "w2", "w3", "w4", "w5"], batch_size=2, concurrency_per_batch=2)
        assert len(results) == 5

    @pytest.mark.asyncio
    async def test_analyze_wallets_batch_progress(self, analyzer):
        calls = []

        def progress(batch_num, total_batches, processed, total):
            calls.append((batch_num, total_batches, processed, total))

        async def fake_metrics(address):
            return None

        analyzer.get_wallet_metrics = fake_metrics
        results = await analyzer.analyze_wallets_batch(
            ["w1", "w2", "w3"], batch_size=2, concurrency_per_batch=1,
            progress_callback=progress)
        assert len(calls) == 2
        assert results == {"w1": None, "w2": None, "w3": None}

    @pytest.mark.asyncio
    async def test_prefetch_wallet_ages_gather_failure(self, analyzer, monkeypatch):
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer._get_wallet_creation_time_cached = AsyncMock(return_value=None)

        async def fake_gather(*tasks, return_exceptions=False):
            raise RuntimeError("gather broke")

        monkeypatch.setattr(analyzer_mod.asyncio, "gather", fake_gather)
        await analyzer.prefetch_wallet_data(["w1"])


class TestCategorizeExtra:
    @pytest.fixture
    def analyzer(self):
        a = WalletAnalyzer(helius_api_key="test-key", discover_wallets=False)
        a.helius_client.api_key = "test-key"
        return a

    def test_native_transfers_wallet_owned(self, analyzer):
        tx = {
            "signature": "sig_own",
            "type": "SWAP",
            "feePayer": "w1",
            "tokenTransfers": [
                {"mint": "mintX", "fromUserAccount": "owned_acc",
                 "toUserAccount": "w1", "tokenAmount": "1000",
                 "userAccount": None},
            ],
            "nativeTransfers": [
                {"fromUserAccount": "w1", "toUserAccount": "owned_acc",
                 "amount": 0.01},
            ],
            "events": {},
        }
        # owned_acc receives SOL from wallet -> wallet-owned (only used for
        # logging; deltas still come from from/toUserAccount == wallet)
        tx2 = dict(tx)
        tx2["nativeTransfers"] = [
            {"fromUserAccount": "w1", "toUserAccount": "owned_acc", "amount": 0.01},
        ]
        assert analyzer._categorize_parse_failure(tx2, "w1") == "unknown"

    def test_transfer_without_mint(self, analyzer):
        tx = {
            "signature": "sig_nomint",
            "type": "SWAP",
            "feePayer": "w1",
            "tokenTransfers": [
                {"mint": "", "fromUserAccount": "w1", "toUserAccount": "d",
                 "tokenAmount": "1000"},
            ],
            "nativeTransfers": [],
            "events": {},
        }
        assert analyzer._categorize_parse_failure(tx, "w1") == "no_primary_token"

    def test_delegated_user_account_deltas(self, analyzer):
        # userAccount=wallet with fromUserAccount=other -> OUT (delegated)
        tx = {
            "signature": "sig_del",
            "type": "SWAP",
            "feePayer": "w1",
            "tokenTransfers": [
                {"mint": "mintD", "fromUserAccount": "other",
                 "toUserAccount": "temp", "userAccount": "w1",
                 "tokenAmount": "1000"},
                {"mint": SOL_MINT, "fromUserAccount": "dex",
                 "toUserAccount": "temp", "userAccount": "w1",
                 "tokenAmount": "500000"},
            ],
            "nativeTransfers": [],
            "events": {},
        }
        reason = analyzer._categorize_parse_failure(tx, "w1")
        assert reason in ("direction_ambiguous", "unknown")

    def test_sol_delta_from_native_transfers(self, analyzer):
        tx = {
            "signature": "sig_nd",
            "type": "SWAP",
            "feePayer": "w1",
            "tokenTransfers": [
                {"mint": "mintN", "fromUserAccount": "dex",
                 "toUserAccount": "w1", "tokenAmount": "1000"},
                {"mint": SOL_MINT, "fromUserAccount": "w1",
                 "toUserAccount": "dex", "tokenAmount": "100000"},
            ],
            "nativeTransfers": [
                {"fromUserAccount": "w1", "toUserAccount": "jito", "amount": 0.001},
                {"fromUserAccount": "vault", "toUserAccount": "w1", "amount": 0.002},
            ],
            "events": {},
        }
        # SOL delta: -100000 (token) - 0.001 (native) + 0.002 (native) -> movement
        assert analyzer._categorize_parse_failure(tx, "w1") == "unknown"
class TestFinalBranches:
    def test_paper_gains_buy_price_at_trade_fallback(self):
        trades = [
            _make_trade(0, days=1, token="PF1", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0.01"),
                        sol_amount=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"PF1": 0.02}) == 1.0

    def test_paper_gains_buy_unresolvable_skipped(self):
        trades = [
            _make_trade(0, days=1, token="PF2", token_amount=None,
                        price_sol=Decimal("0"), price_at_trade=Decimal("0"),
                        sol_amount=Decimal("1.0")),
        ]
        assert PortfolioTracker.calculate_paper_gains(trades, {"PF2": 2.0}) == 0.0

    @pytest.mark.asyncio
    async def test_known_token_redis_set_failure(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        redis.set.side_effect = RuntimeError("redis full")
        analyzer._redis_client = redis
        mint = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"
        assert await analyzer._get_token_symbol(mint) == "BONK"

    @pytest.mark.asyncio
    async def test_unknown_token_redis_set_failure(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        redis.set.side_effect = RuntimeError("redis full")
        analyzer._redis_client = redis
        assert await analyzer._get_token_symbol("mystery_mint_2") is None

    @pytest.mark.asyncio
    async def test_async_birdeye_redis_set_failure(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        redis.set.side_effect = RuntimeError("redis full")
        analyzer._redis_client = redis
        birdeye = Mock()
        birdeye.get_token_metadata = AsyncMock(return_value={"symbol": "NEW2"})
        analyzer.liquidity_provider.birdeye_client = birdeye
        assert await analyzer._get_token_symbol_async("mint_redis_fail") == "NEW2"
        assert redis.set.called, "redis.set must have been attempted"

    @pytest.mark.asyncio
    async def test_token_creation_negative_redis_write(self, analyzer):
        redis = Mock()
        redis.is_available.return_value = True
        redis.get.return_value = None
        analyzer._redis_client = redis
        analyzer.helius_client.get_token_first_tx_timestamp = AsyncMock(
            return_value=None)
        analyzer.helius_client._get_session = AsyncMock(
            side_effect=RuntimeError("no session"))
        assert await analyzer._fetch_token_creation_time("mint_negcache") is None
        # negative result cached as "null" for 1h
        call_args = redis.set.call_args
        assert call_args.args[1] == "null"

    def test_round_trip_weird_amount_type(self, analyzer):
        txs = [
            _tx("w1", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintW", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": [1, 2]},
            ]),
            _tx("w2", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintW2", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": "10"},
            ]),
            _tx("w3", tx_type="SWAP", tokenTransfers=[
                {"mint": "mintW3", "fromUserAccount": "w", "toUserAccount": "d",
                 "tokenAmount": 10},
            ]),
        ]
        assert analyzer._detect_round_trip_ratio_from_transactions(txs, "w") == 0.0

    @pytest.mark.asyncio
    async def test_unrealized_pnl_exception_with_holdings(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(side_effect=RuntimeError("no price"))
        trades = [
            _make_trade(0, days=3, token="HLD", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(1, days=2, token="HLD", token_amount=Decimal("100"),
                        sol_amount=Decimal("1.0")),
            _make_trade(2, is_sell=True, days=1, token="HLD",
                        token_amount=Decimal("50"), sol_amount=Decimal("0.5"),
                        pnl_sol=Decimal("0.05")),
        ]
        metrics = await analyzer._calculate_metrics_from_trades(
            "w1", trades, transactions=[])
        assert metrics is not None
        assert metrics.total_unrealized_loss_sol is None

    @pytest.mark.asyncio
    async def test_historical_trades_fetch_exception(self, analyzer):
        async def boom(address, days):
            raise RuntimeError("helius exploded")

        analyzer._fetch_real_historical_trades = boom
        trades = await analyzer.get_historical_trades("w1", days=30)
        assert trades == []

    @pytest.mark.asyncio
    async def test_fetch_real_liquidity_collection_exception(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("lc1")])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("lc1"))
        analyzer._parse_swap_to_trade = AsyncMock(return_value=_make_trade(0, days=1))

        async def broken_gather(*tasks, return_exceptions=False):
            raise RuntimeError("gather broke")

        monkeypatch.setattr(analyzer_mod.asyncio, "gather", broken_gather)
        trades = await analyzer._fetch_real_historical_trades("w1", 30)
        assert len(trades) == 1

    def test_delegated_user_account_in(self, analyzer):
        tx = {
            "signature": "sig_delin",
            "type": "SWAP",
            "feePayer": "w1",
            "tokenTransfers": [
                # userAccount == wallet, fromUserAccount == wallet,
                # toUserAccount != wallet -> elif branch adds +amt
                {"mint": "mintE", "fromUserAccount": "w1", "toUserAccount": "temp",
                 "userAccount": "w1", "tokenAmount": "1000"},
                {"mint": SOL_MINT, "fromUserAccount": "w1", "toUserAccount": "temp",
                 "userAccount": "w1", "tokenAmount": "500000"},
            ],
            "nativeTransfers": [],
            "events": {},
        }
        assert analyzer._categorize_parse_failure(tx, "w1") == "direction_ambiguous"


class TestDebugDumpExtras:
    @pytest.mark.asyncio
    async def test_long_dump_and_bot_instructions(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        monkeypatch.setenv("SCOUT_DEBUG_TX_DUMP", "true")
        analyzer.helius_client.KNOWN_BOT_ROUTERS = {"botprog"}
        long_tx = _tx("long_sig", source="JUPITER",
                      instructions=[{"programId": "botprog"}] * 150)
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[long_tx])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("long_sig"))
        analyzer._parse_swap_to_trade = AsyncMock(return_value=_make_trade(0))
        analyzer._calculate_metrics_from_trades = AsyncMock(
            return_value=_wallet_metrics(address="w1"))
        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert analyzer._parse_stats["swaps_parsed"] == 1

    @pytest.mark.asyncio
    async def test_unknown_failure_debug_logging(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        tx_unknown = {
            "signature": "unk_sig", "type": "SWAP", "source": "PHANTOM",
            "feePayer": "w1", "description": "Swapped 1 SOL for tokens",
            "instructions": [{"programId": "prog1"}], "events": {"swap": {}},
            "accountData": [{}], "nativeTransfers": [],
            "tokenTransfers": [
                {"mint": "mintU", "fromUserAccount": "w1", "toUserAccount": "d",
                 "tokenAmount": "1000"},
                {"mint": SOL_MINT, "fromUserAccount": "d", "toUserAccount": "w1",
                 "tokenAmount": "500000"},
            ],
        }
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx_unknown])
        analyzer.helius_client.parse_swap_transaction = Mock(return_value=None)
        assert await analyzer._fetch_real_wallet_metrics("w1") is None
        reason = analyzer._categorize_parse_failure(tx_unknown, "w1")
        assert reason == "unknown"

    @pytest.mark.asyncio
    async def test_debug_dump_makedirs_exception(self, analyzer, monkeypatch):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        monkeypatch.setenv("SCOUT_DEBUG_PARSE_FAILURES", "true")
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[_tx("mkfail")])
        analyzer.helius_client.parse_swap_transaction = Mock(return_value=None)

        def broken_makedirs(path, exist_ok=False):
            raise OSError("permission denied")

        monkeypatch.setattr(analyzer_mod.os, "makedirs", broken_makedirs)
        assert await analyzer._fetch_real_wallet_metrics("w1") is None

    @pytest.mark.asyncio
    async def test_limit_order_program_instructions(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        tx_lo = _tx("lo_sig", source="JUPITER",
                    instructions=[{"programId": "j1o2qRpjcyUwEvwtcfhEQefh773ZgjxcVRry7LDqg5X"}])
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx_lo])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("lo_sig"))
        analyzer._parse_swap_to_trade = AsyncMock(return_value=_make_trade(0))
        captured = {}

        async def fake_calc(address, trades, **kw):
            captured.update(kw)
            return _wallet_metrics(address=address)

        analyzer._calculate_metrics_from_trades = fake_calc
        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert captured["uses_limit_orders"] is True

    @pytest.mark.asyncio
    async def test_mev_detection_exception(self, analyzer):
        analyzer.can_spend_budget = Mock(return_value=(True, "Budget OK"))
        analyzer.record_credit_usage = Mock()
        jito_tip = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4"
        # First tx sets uses_mev_protection so the smart-money loop skips the
        # second tx's nativeTransfers; the sandwich block still processes it.
        tx_tip = _tx("tip_sig", source="JUPITER",
                     nativeTransfers=[{"toUserAccount": jito_tip, "amount": 0.001}])
        tx_weird = _tx("mev_bad", source="JUPITER",
                       tokenTransfers=[{}, {}, {}, {}],
                       nativeTransfers=[None])  # None breaks nt.get -> exception
        analyzer.helius_client.get_wallet_transactions = AsyncMock(
            return_value=[tx_tip, tx_weird])
        analyzer.helius_client.parse_swap_transaction = Mock(
            return_value=_swap_dict("mev_bad"))
        analyzer._parse_swap_to_trade = AsyncMock(return_value=_make_trade(0))
        captured = {}

        async def fake_calc(address, trades, **kw):
            captured.update(kw)
            return _wallet_metrics(address=address)

        analyzer._calculate_metrics_from_trades = fake_calc
        metrics = await analyzer._fetch_real_wallet_metrics("w1")
        assert metrics is not None
        assert captured["mev_risk_score"] is None


class TestBagFallbackBranches:
    @pytest.mark.asyncio
    async def test_bag_buy_and_sell_qty_fallback(self, analyzer):
        analyzer.rugcheck_client = None
        analyzer._fetch_token_creation_time = AsyncMock(return_value=None)
        analyzer._get_sol_price_usd = AsyncMock(return_value=100.0)
        analyzer.helius_client.get_wallet_funder = AsyncMock(return_value=None)
        trades = [
            # BUY with token_amount None -> qty derived from price_at_trade
            _make_trade(0, days=5, token="BF", token_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("1.0"),
                        sol_amount=Decimal("1.0")),
            # SELL on existing position with qty from price_at_trade
            _make_trade(1, is_sell=True, days=4, token="BF", token_amount=None,
                        price_at_trade=Decimal("0.5"), amount_sol=Decimal("0.4"),
                        sol_amount=Decimal("0.4"), pnl_sol=Decimal("0.05")),
            # SELL on existing position with unresolvable qty -> skipped
            _make_trade(2, is_sell=True, days=3, token="BF", token_amount=None,
                        price_at_trade=Decimal("0"), amount_sol=Decimal("0.4"),
                        sol_amount=Decimal("0.4"), pnl_sol=Decimal("0.05")),
            *[_make_trade(3 + i, is_sell=i % 2 == 1, token="reg",
                          token_amount=Decimal("100"), sol_amount=Decimal("1.0"),
                          pnl_sol=Decimal("0.05") if i % 2 == 1 else None)
              for i in range(8)],
        ]
        with patch.object(analyzer_mod.PortfolioTracker, "fetch_bulk_prices",
                          AsyncMock(return_value={})), \
             patch.object(analyzer_mod, "is_known_scam_address",
                          Mock(return_value=False)), \
             patch.object(analyzer_mod, "check_wallet_correlation",
                          AsyncMock(return_value=True)):
            metrics = await analyzer._calculate_metrics_from_trades(
                "w1", trades, transactions=[])
        assert metrics is not None
