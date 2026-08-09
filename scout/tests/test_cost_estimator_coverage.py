"""Coverage completion tests for core/cost_estimator.py (Helius fee estimation)."""

import json
import time
from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

import core.cost_estimator as ce
from core.cost_estimator import (
    DEFAULT_JITO_TIP_SOL,
    DEFAULT_PRIORITY_FEE_SOL,
    CostEstimator,
)


def make_estimator(monkeypatch, tmp_path, api_key="test-key", rpc_url=None):
    monkeypatch.setenv("SCOUT_FEE_CACHE_PATH", str(tmp_path / "fee_cache.json"))
    if rpc_url is None:
        monkeypatch.delenv("CHIMERA_RPC__PRIMARY_URL", raising=False)
        monkeypatch.delenv("SOLANA_RPC_URL", raising=False)
    else:
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", rpc_url)
    return CostEstimator(helius_api_key=api_key)


class FakeResponse:
    """Minimal aiohttp response stand-in."""

    def __init__(self, status=200, data=None, error=None):
        self.status = status
        self._data = data
        self._error = error

    async def json(self):
        if self._error is not None:
            raise self._error
        return self._data

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False


class FakeSession:
    """aiohttp session stand-in that returns preset responses.

    aiohttp's session.post() returns a request context manager (not a
    coroutine); the code does `async with session.post(...)`.
    """

    def __init__(self, responses):
        self.responses = list(responses)
        self.posted = []

    def post(self, url, json=None, params=None):
        self.posted.append((url, json, params))
        if self.responses:
            resp = self.responses.pop(0)
        else:
            resp = FakeResponse(status=500)
        return resp

    async def close(self):
        self.closed = True


class TestRpcParams:
    def test_params_with_key(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        assert est._get_rpc_params() == {"api-key": "test-key"}

    def test_params_without_key(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path, api_key=None)
        assert est._get_rpc_params() == {}


class TestSession:
    async def test_get_session_creates_once(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = MagicMock()
        with patch.object(ce.aiohttp, "ClientSession", return_value=session):
            first = await est._get_session()
            second = await est._get_session()
        assert first is second is session

    async def test_close_closes_session(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = AsyncMock()
        est._session = session
        await est.close()
        session.close.assert_awaited_once()
        assert est._session is None

    async def test_close_without_session(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        await est.close()


class TestPriorityFeeEstimate:
    async def test_default_percentile_shield(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "_fetch_raw_fee_estimates", return_value=[0.001, 0.002, 0.003, 0.004]):
            fee = await est.get_priority_fee_estimate()
        assert float(fee) == pytest.approx(0.00325)

    async def test_spear_strategy_percentile(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "_fetch_raw_fee_estimates", return_value=[0.001, 0.002, 0.003, 0.004]):
            fee = await est.get_priority_fee_estimate(strategy="spear")
        assert fee == Decimal("0.0037")

    async def test_explicit_percentile(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "_fetch_raw_fee_estimates", return_value=[0.001, 0.002, 0.003, 0.004]):
            fee = await est.get_priority_fee_estimate(percentile=50)
        assert fee == Decimal("0.0025")

    async def test_raw_none_returns_default(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "_fetch_raw_fee_estimates", return_value=None):
            assert await est.get_priority_fee_estimate() == DEFAULT_PRIORITY_FEE_SOL


class TestJitoTip:
    async def test_spear_markup(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "get_priority_fee_estimate", return_value=Decimal("0.001")):
            tip = await est.get_jito_tip_estimate(strategy="SPEAR")
        assert tip == Decimal("0.0012")

    async def test_shield_no_markup(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "get_priority_fee_estimate", return_value=Decimal("0.001")):
            tip = await est.get_jito_tip_estimate(strategy="SHIELD")
        assert tip == Decimal("0.001")

    async def test_minimum_default(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "get_priority_fee_estimate", return_value=Decimal("0.0000001")):
            tip = await est.get_jito_tip_estimate()
        assert tip == DEFAULT_JITO_TIP_SOL


class TestGetAllEstimates:
    async def test_returns_tuple(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        with patch.object(est, "get_priority_fee_estimate", return_value=Decimal("0.001")), patch.object(
            est, "get_jito_tip_estimate", return_value=Decimal("0.0012")
        ):
            prio, jito = await est.get_all_estimates()
        assert (prio, jito) == (Decimal("0.001"), Decimal("0.0012"))


class TestFeeCachePath:
    def test_default_path_env_override(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        monkeypatch.setenv("SCOUT_FEE_CACHE_PATH", str(tmp_path / "custom.json"))
        assert est._fee_cache_path() == str(tmp_path / "custom.json")


class TestLoadFeeCache:
    def test_missing_file(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        assert "raw_fees" not in est._cache

    def test_valid_cache(self, monkeypatch, tmp_path):
        path = tmp_path / "fee_cache.json"
        path.write_text(json.dumps({"timestamp": time.time(), "fee_levels": [0.001, 0.002]}))
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        ts, levels = est._cache["raw_fees"]
        assert list(levels) == [0.001, 0.002]

    def test_stale_cache_ignored(self, monkeypatch, tmp_path):
        path = tmp_path / "fee_cache.json"
        path.write_text(json.dumps({"timestamp": time.time() - 8 * 86400, "fee_levels": [0.001]}))
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        assert "raw_fees" not in est._cache

    def test_non_list_fee_levels(self, monkeypatch, tmp_path):
        path = tmp_path / "fee_cache.json"
        path.write_text(json.dumps({"timestamp": time.time(), "fee_levels": "nope"}))
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        assert "raw_fees" not in est._cache

    def test_empty_fee_levels(self, monkeypatch, tmp_path):
        path = tmp_path / "fee_cache.json"
        path.write_text(json.dumps({"timestamp": time.time(), "fee_levels": []}))
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        assert "raw_fees" not in est._cache

    def test_corrupt_json(self, monkeypatch, tmp_path):
        path = tmp_path / "fee_cache.json"
        path.write_text("{not json")
        est = make_estimator(monkeypatch, tmp_path)
        est._load_fee_cache()
        assert "raw_fees" not in est._cache


class TestPersistFeeCache:
    def test_no_cache_entry(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._persist_fee_cache()

    def test_empty_levels(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (time.time(), [])
        est._persist_fee_cache()

    def test_persists(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (1234.0, (0.001, 0.002))
        est._persist_fee_cache()
        data = json.loads((tmp_path / "fee_cache.json").read_text())
        assert data["fee_levels"] == [0.001, 0.002]
        assert data["stale_threshold_days"] == ce.FEE_CACHE_STALE_DAYS

    def test_persist_os_error(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (1234.0, (0.001,))
        with patch.object(ce.os, "makedirs", side_effect=OSError("denied")):
            est._persist_fee_cache()


class TestFetchRawFeeEstimates:
    async def test_no_rpc_url(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path, api_key=None)
        assert await est._fetch_raw_fee_estimates() is None

    async def test_no_api_key(self, monkeypatch, tmp_path):
        monkeypatch.setenv("CHIMERA_RPC__PRIMARY_URL", "https://rpc.example")
        est = CostEstimator(helius_api_key=None)
        assert await est._fetch_raw_fee_estimates() is None

    async def test_fresh_cache(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (time.time(), (0.001, 0.002))
        assert await est._fetch_raw_fee_estimates() == [0.001, 0.002]

    async def test_fresh_cache_non_list(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (time.time(), "not-a-list")
        assert await est._fetch_raw_fee_estimates() is None

    async def test_http_success(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = FakeSession([FakeResponse(status=200, data={"result": {"priorityFeeEstimate": 1000}})])
        est._session = session
        result = await est._fetch_raw_fee_estimates()
        assert result == [1e-06]
        assert "raw_fees" in est._cache
        assert (tmp_path / "fee_cache.json").exists()

    async def test_http_error_with_cached_fallback(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (time.time() - 99999, (0.005,))  # stale cache entry
        session = FakeSession([FakeResponse(status=503)])
        est._session = session
        assert await est._fetch_raw_fee_estimates() == [0.005]

    async def test_http_error_without_cache(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = FakeSession([FakeResponse(status=503)])
        est._session = session
        assert await est._fetch_raw_fee_estimates() is None

    async def test_exception_with_cached_fallback(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        est._cache["raw_fees"] = (time.time() - 99999, (0.005,))
        session = FakeSession([FakeResponse(status=200, error=RuntimeError("boom"))])
        est._session = session
        assert await est._fetch_raw_fee_estimates() == [0.005]

    async def test_exception_without_cache(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = FakeSession([FakeResponse(status=200, error=RuntimeError("boom"))])
        est._session = session
        assert await est._fetch_raw_fee_estimates() is None

    async def test_response_not_parsable(self, monkeypatch, tmp_path):
        est = make_estimator(monkeypatch, tmp_path)
        session = FakeSession([FakeResponse(status=200, data={"nope": True})])
        est._session = session
        assert await est._fetch_raw_fee_estimates() is None

    def test_redact_removes_api_key(self):
        text = "Error calling https://rpc?api-key=SECRET123&foo=bar"
        assert "SECRET123" not in CostEstimator._redact(text)
        assert "REDACTED" in CostEstimator._redact(text)


class TestParseFeeResponse:
    def test_no_result(self):
        assert CostEstimator._parse_fee_response({}) is None

    def test_dict_priority_fee_levels(self):
        data = {"result": {"priorityFeeLevels": {"low": 1000, "high": 5000}}}
        assert CostEstimator._parse_fee_response(data) == [1e-06, 5e-06]

    def test_dict_percentiles(self):
        data = {"result": {"percentiles": {"50": 2000, "90": 6000}}}
        assert CostEstimator._parse_fee_response(data) == [2e-06, 6e-06]

    def test_dict_single_estimate(self):
        data = {"result": {"priorityFeeEstimate": 3000}}
        assert CostEstimator._parse_fee_response(data) == [3e-06]

    def test_dict_priority_fee_levels_empty(self):
        data = {"result": {"priorityFeeLevels": {}}}
        assert CostEstimator._parse_fee_response(data) is None

    def test_numeric_result(self):
        assert CostEstimator._parse_fee_response({"result": 4000}) == [4e-06]

    def test_other_result_type(self):
        assert CostEstimator._parse_fee_response({"result": "abc"}) is None


class TestPercentile:
    def test_empty(self):
        assert CostEstimator._percentile([], 50) == DEFAULT_PRIORITY_FEE_SOL

    def test_single(self):
        assert CostEstimator._percentile([0.002], 50) == Decimal("0.002")

    def test_exact_index(self):
        values = [0.001, 0.002, 0.003]
        assert CostEstimator._percentile(values, 100) == Decimal("0.003")

    def test_interpolation(self):
        values = [0.001, 0.002, 0.003]
        assert CostEstimator._percentile(values, 50) == Decimal("0.002")
        assert CostEstimator._percentile(values, 25) == Decimal("0.0015")


class TestInitWithEnvConfig:
    def test_api_key_from_env(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SCOUT_FEE_CACHE_PATH", str(tmp_path / "fc.json"))
        monkeypatch.setenv("HELIUS_API_KEY", "env-key")
        est = CostEstimator()
        assert est._api_key == "env-key"
        assert est._rpc_url == "https://mainnet.helius-rpc.com/"
