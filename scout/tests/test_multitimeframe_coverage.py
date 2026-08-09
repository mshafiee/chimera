"""Coverage completion tests for core/multitimeframe_discovery.py."""

from unittest.mock import MagicMock

import pytest

from core.multitimeframe_discovery import (
    AdaptiveTimeframeSelector,
    DiscoveryTimeframe,
    MultiTimeframeDiscovery,
    MultiTimeframeResult,
    TimeframeConfig,
    TimeframeResult,
    get_multi_timeframe_discovery,
)


def make_timeframe_result(tf, wallets, scores=None, credits=10, exec_time=1.0):
    scores = scores if scores is not None else {w: 50.0 + i for i, w in enumerate(wallets)}
    return TimeframeResult(
        timeframe=tf,
        wallets_discovered=wallets,
        wallet_quality_scores=scores,
        credits_consumed=credits,
        execution_time_seconds=exec_time,
    )


def make_fake_helius(wallets_by_hours=None):
    client = MagicMock()

    async def discover_wallets(hours_back=720, max_wallets=600, limit_per_token=100):
        wallets_by_hours = getattr(client, "_wallets", {})
        if hours_back in wallets_by_hours:
            return wallets_by_hours[hours_back]
        return {}

    client._wallets = wallets_by_hours or {}
    client.discover_wallets = discover_wallets
    return client


class TestTimeframeResult:
    def test_get_high_quality_wallets(self):
        result = make_timeframe_result(
            DiscoveryTimeframe.FAST, ["w1", "w2"], {"w1": 80.0, "w2": 40.0}
        )
        assert result.get_high_quality_wallets(min_score=50.0) == ["w1"]
        assert result.get_high_quality_wallets(min_score=100.0) == []

    def test_to_dict(self):
        result = make_timeframe_result(
            DiscoveryTimeframe.FAST, ["w1", "w2"], {"w1": 80.0, "w2": 40.0}
        )
        data = result.to_dict()
        assert data["timeframe"] == "fast"
        assert data["wallets_discovered"] == 2
        assert data["high_quality_count"] == 1
        assert data["average_quality"] == 60.0

    def test_to_dict_empty_scores(self):
        result = make_timeframe_result(DiscoveryTimeframe.FAST, [])
        assert result.to_dict()["average_quality"] == 0.0


class TestMultiTimeframeResult:
    def test_total_unique_wallets(self):
        result = MultiTimeframeResult(
            timeframe_results={},
            combined_wallets=["a", "b", "c"],
            combined_quality_scores={},
            cross_timeframe_ranking=[],
            deduplication_stats={},
            total_credits_consumed=0,
            total_execution_time_seconds=0.0,
        )
        assert result.total_unique_wallets == 3

    def test_get_top_wallets(self):
        result = MultiTimeframeResult(
            timeframe_results={},
            combined_wallets=[],
            combined_quality_scores={},
            cross_timeframe_ranking=[("a", 90.0), ("b", 80.0), ("c", 70.0)],
            deduplication_stats={},
            total_credits_consumed=0,
            total_execution_time_seconds=0.0,
        )
        assert result.get_top_wallets(2) == ["a", "b"]

    def test_to_dict(self):
        tf_result = make_timeframe_result(DiscoveryTimeframe.FAST, ["w1"], {"w1": 90.0})
        result = MultiTimeframeResult(
            timeframe_results={DiscoveryTimeframe.FAST: tf_result},
            combined_wallets=["w1"],
            combined_quality_scores={"w1": 90.0},
            cross_timeframe_ranking=[("w1", 90.0)],
            deduplication_stats={"total_raw_wallets": 1},
            total_credits_consumed=10,
            total_execution_time_seconds=1.5,
        )
        data = result.to_dict()
        assert "fast" in data["timeframe_results"]
        assert data["total_unique_wallets"] == 1
        assert data["top_wallets"] == ["w1"]


class TestInit:
    def test_init_configs(self, monkeypatch):
        monkeypatch.setenv("SCOUT_DEEP_SCAN_HOURS", "100")
        monkeypatch.setenv("SCOUT_FAST_MAX_WALLETS", "77")
        disco = MultiTimeframeDiscovery()
        assert disco._timeframe_configs[DiscoveryTimeframe.DEEP].hours_back == 100
        assert disco._timeframe_configs[DiscoveryTimeframe.FAST].max_wallets == 77
        assert disco._execution_stats["total_runs"] == 0


class TestDiscoverAll:
    async def test_parallel_discovery(self):
        helius = make_fake_helius({
            720: {"w1": 5, "w2": 3},
            24: {"w1": 2, "w3": 4},
            4: {"w3": 1},
        })
        disco = MultiTimeframeDiscovery(helius)
        result = await disco.discover_all_timeframes()
        assert len(result.combined_wallets) == 3
        assert result.deduplication_stats["total_raw_wallets"] == 5
        assert result.total_credits_consumed == 150
        assert disco._execution_stats["total_runs"] == 1
        assert disco._execution_stats["successful_runs"] == 1
        assert disco._execution_stats["average_time_seconds"] > 0

    async def test_parallel_with_budget_and_subset(self):
        helius = make_fake_helius({720: {"w1": 5}, 24: {"w2": 2}})
        disco = MultiTimeframeDiscovery(helius)
        result = await disco.discover_all_timeframes(
            budget_credits=100,
            timeframes=[DiscoveryTimeframe.DEEP, DiscoveryTimeframe.FAST],
        )
        assert result.deduplication_stats["total_raw_wallets"] == 2
        assert result.total_credits_consumed == 100

    async def test_sequential_discovery(self):
        helius = make_fake_helius({720: {"w1": 5}, 24: {"w2": 2}, 4: {"w3": 1}})
        disco = MultiTimeframeDiscovery(helius)
        result = await disco.discover_all_timeframes(parallel=False)
        assert len(result.combined_wallets) == 3

    async def test_sequential_budget_exhausted(self):
        helius = make_fake_helius({720: {"w1": 5}, 24: {"w2": 2}, 4: {"w3": 1}})
        disco = MultiTimeframeDiscovery(helius)
        result = await disco.discover_all_timeframes(parallel=False, budget_credits=60)
        # 50 for trending + 10 for fast, then budget exhausted
        assert result.total_credits_consumed == 60
        assert len(result.timeframe_results) == 2


    async def test_failing_timeframe_gets_empty_result(self):
        class FailingClient:
            async def discover_wallets(self, hours_back=720, max_wallets=600, limit_per_token=100):
                if hours_back == 720:
                    raise RuntimeError("helius down")
                return {"w9": 1}

        disco = MultiTimeframeDiscovery(FailingClient())
        result = await disco.discover_all_timeframes()
        deep = result.timeframe_results[DiscoveryTimeframe.DEEP]
        assert deep.wallets_discovered == []
        assert "error" in deep.metadata
        assert len(result.combined_wallets) == 1

    async def test_no_helius_client_fallback(self):
        disco = MultiTimeframeDiscovery(None)
        result = await disco.discover_all_timeframes()
        assert result.combined_wallets == []

    async def test_single_timeframe_failure_propagates(self):
        class FailingClient:
            async def discover_wallets(self, hours_back=720, max_wallets=600, limit_per_token=100):
                raise RuntimeError("boom")

        disco = MultiTimeframeDiscovery(FailingClient())
        with pytest.raises(RuntimeError, match="boom"):
            await disco._execute_single_timeframe(
                DiscoveryTimeframe.DEEP,
                disco._timeframe_configs[DiscoveryTimeframe.DEEP],
                None,
            )


class TestCombine:
    async def test_combine_with_bonus(self):
        disco = MultiTimeframeDiscovery()
        results = {
            DiscoveryTimeframe.DEEP: make_timeframe_result(
                DiscoveryTimeframe.DEEP, ["w1"], {"w1": 50.0}, credits=10
            ),
            DiscoveryTimeframe.FAST: make_timeframe_result(
                DiscoveryTimeframe.FAST, ["w1", "w2"], {"w1": 60.0, "w2": 55.0}, credits=10
            ),
        }
        combined = await disco._combine_timeframe_results(results)
        assert combined.deduplication_stats["total_raw_wallets"] == 3
        assert combined.deduplication_stats["multi_timeframe_wallets"] == 1
        assert combined.combined_quality_scores["w1"] == 65.0  # avg 55 + 10 bonus
        assert combined.combined_quality_scores["w2"] == 55.0
        assert combined.cross_timeframe_ranking[0][0] == "w1"

    async def test_combine_empty(self):
        disco = MultiTimeframeDiscovery()
        combined = await disco._combine_timeframe_results({})
        assert combined.combined_wallets == []
        assert combined.deduplication_stats == {
            "total_raw_wallets": 0,
            "unique_wallets": 0,
            "deduplication_ratio": 0.0,
            "multi_timeframe_wallets": 0,
        }


class TestExecutionStats:
    def test_get_execution_stats(self):
        disco = MultiTimeframeDiscovery()
        assert disco.get_execution_stats()["total_runs"] == 0


class TestTimeframeConfigs:
    def test_get_timeframe_config(self):
        disco = MultiTimeframeDiscovery()
        config = disco.get_timeframe_config(DiscoveryTimeframe.DEEP)
        assert config.timeframe == DiscoveryTimeframe.DEEP
        assert disco.get_timeframe_config(DiscoveryTimeframe.CUSTOM) is None

    def test_set_timeframe_config(self):
        disco = MultiTimeframeDiscovery()
        new_config = TimeframeConfig(
            timeframe=DiscoveryTimeframe.FAST,
            hours_back=12,
            max_wallets=50,
            limit_per_token=10,
            execution_priority=1,
            expected_quality_score=70.0,
            description="test config",
        )
        disco.set_timeframe_config(new_config)
        assert disco.get_timeframe_config(DiscoveryTimeframe.FAST).max_wallets == 50


class TestAdaptiveTimeframeSelector:
    def make_selector(self):
        disco = MultiTimeframeDiscovery()
        return AdaptiveTimeframeSelector(disco)

    def test_goal_quality(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="quality")
        assert selected == [DiscoveryTimeframe.TRENDING, DiscoveryTimeframe.FAST]

    def test_goal_quantity(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="quantity")
        assert selected == [DiscoveryTimeframe.DEEP, DiscoveryTimeframe.FAST, DiscoveryTimeframe.TRENDING]

    def test_goal_balanced(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="balanced")
        assert selected == [DiscoveryTimeframe.FAST, DiscoveryTimeframe.TRENDING, DiscoveryTimeframe.DEEP]

    def test_goal_speed(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="speed")
        assert selected == [DiscoveryTimeframe.TRENDING]

    def test_unknown_goal_falls_back_to_balanced(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="weird")
        assert DiscoveryTimeframe.FAST in selected

    def test_budget_filter(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="quantity", budget_credits=1500)
        # DEEP 100*10=1000, FAST 150*10=1500 affordable; TRENDING 200*10=2000 not
        assert selected == [DiscoveryTimeframe.DEEP, DiscoveryTimeframe.FAST]

    def test_budget_nothing_affordable(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="quantity", budget_credits=100)
        assert selected == []

    def test_time_limit_filter(self):
        selector = self.make_selector()
        selected = selector.select_optimal_timeframes(goal="balanced", time_limit_seconds=15)
        # FAST estimated 20s too slow; TRENDING 10s fits; DEEP 60s too slow
        assert selected == [DiscoveryTimeframe.TRENDING]

    def test_selection_history(self):
        selector = self.make_selector()
        selector.select_optimal_timeframes(goal="speed")
        history = selector.get_selection_history()
        assert len(history) == 1
        assert history[0]["goal"] == "speed"
        assert history[0]["selected"] == ["trending"]


class TestSingleton:
    def test_get_multi_timeframe_discovery(self, monkeypatch):
        import core.multitimeframe_discovery as mtf

        monkeypatch.setattr(mtf, "_multi_timeframe_instance", None)
        first = get_multi_timeframe_discovery()
        second = get_multi_timeframe_discovery()
        assert first is second
        monkeypatch.setattr(mtf, "_multi_timeframe_instance", None)
