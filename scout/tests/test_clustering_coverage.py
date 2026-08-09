"""
Coverage tests for core/clustering.py.

Covers _load_exchange_funders file parsing, _resolve_funder_root stop
conditions, cluster_and_dedup fallback paths, and
apply_cross_wallet_token_correlation branches.
"""

import asyncio

import core.clustering as clustering
from core.models import WalletRecord
from core.clustering import (
    _resolve_funder_root,
    apply_cross_wallet_token_correlation,
    cluster_and_dedup,
)


EXCHANGE = "ExchangeFunder1111111111111111111111111111111111"
SYSTEM = "SystemAccount111111111111111111111111111111111111"
NON_WALLET = "NonWalletAddress111111111111111111111111111111111"


def test_load_exchange_funders_from_file(monkeypatch, tmp_path):
    path = tmp_path / "funders.txt"
    path.write_text(f"# comment\n{EXCHANGE} # inline\nshort\n")
    monkeypatch.setenv("SCOUT_EXCHANGE_FUNDERS_PATH", str(path))
    clustering._EXCHANGE_FUNDERS.clear()
    clustering._load_exchange_funders()
    assert EXCHANGE in clustering._EXCHANGE_FUNDERS
    assert "short" not in clustering._EXCHANGE_FUNDERS
    assert len(clustering._EXCHANGE_FUNDERS) == len(clustering._BUILTIN_EXCHANGE_FUNDERS) + 1


def test_load_exchange_funders_missing_file_uses_builtin(monkeypatch, tmp_path):
    monkeypatch.setenv("SCOUT_EXCHANGE_FUNDERS_PATH", str(tmp_path / "missing.txt"))
    clustering._EXCHANGE_FUNDERS.clear()
    clustering._load_exchange_funders()
    assert clustering._EXCHANGE_FUNDERS == set(clustering._BUILTIN_EXCHANGE_FUNDERS)


def test_load_exchange_funders_exception(monkeypatch, tmp_path):
    # Directory path -> IsADirectoryError on open
    monkeypatch.setenv("SCOUT_EXCHANGE_FUNDERS_PATH", str(tmp_path))
    clustering._EXCHANGE_FUNDERS.clear()
    clustering._load_exchange_funders()
    assert clustering._EXCHANGE_FUNDERS == set(clustering._BUILTIN_EXCHANGE_FUNDERS)


class FakeClient:
    def __init__(self, funders=None, root=None):
        self.funders = funders or {}
        self.root = root

    async def get_wallet_funder(self, address):
        return self.funders.get(address)


def _run(coro):
    return asyncio.run(coro)


def test_resolve_funder_root_stop_conditions(monkeypatch):
    monkeypatch.setattr("core.helius_client.HeliusClient.SYSTEM_ACCOUNTS", {SYSTEM})
    monkeypatch.setattr("core.helius_client.HeliusClient.NON_WALLET_ADDRESSES", {NON_WALLET})
    monkeypatch.setattr(clustering, "_EXCHANGE_FUNDERS", {EXCHANGE})
    cache = {}

    # Exchange funder -> None
    assert _run(_resolve_funder_root(FakeClient(), EXCHANGE, 2, cache)) is None
    # System account -> None
    assert _run(_resolve_funder_root(FakeClient(), SYSTEM, 2, cache)) is None
    # Non-wallet address -> None
    assert _run(_resolve_funder_root(FakeClient(), NON_WALLET, 2, cache)) is None


def test_resolve_funder_root_depth_and_cache():
    client = FakeClient(funders={"funder1": "funder2", "funder2": None})
    cache = {}
    # depth 0 -> address itself is root
    assert _run(_resolve_funder_root(client, "funder1", 0, cache)) == "funder1"
    # Follows one hop then hits None funder -> returns funder2 (the root)
    assert _run(_resolve_funder_root(client, "funder1", 2, cache)) == "funder2"
    # Cache hit
    assert _run(_resolve_funder_root(client, "funder1", 2, cache)) == "funder2"


def test_resolve_funder_root_cycle_detection():
    client = FakeClient(funders={"a": "b", "b": "a"})
    cache = {}
    assert _run(_resolve_funder_root(client, "a", 5, cache)) is None


def _active_record(addr, wqs, notes=None):
    return WalletRecord(
        address=addr, status="ACTIVE", wqs_score=wqs, roi_7d=1.0, roi_30d=2.0,
        trade_count_30d=10, win_rate=0.5, max_drawdown_30d=5.0,
        avg_trade_size_sol=0, notes=notes,
    )


def test_cluster_dedup_disabled(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "false")
    records = [_active_record("w1", 70.0)]
    assert _run(cluster_and_dedup(records)) is records


def test_cluster_dedup_single_active(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    records = [_active_record("w1", 70.0), _active_record("c1", 50.0)]
    records[1].status = "CANDIDATE"
    assert _run(cluster_and_dedup(records)) is records


def test_cluster_dedup_import_error_fallback(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    records = [_active_record("w1", 70.0), _active_record("w2", 60.0)]

    real_import = __import__

    def fake_import(name, *args, **kwargs):
        if name == "config":
            raise ImportError("no config module")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr("builtins.__import__", fake_import)
    result = _run(cluster_and_dedup(records, helius_client=FakeClient()))
    assert result is records
    # No funder data -> no clustering possible
    assert all(r.status == "ACTIVE" for r in records)


def test_cluster_dedup_no_funder_map(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    records = [_active_record("w1", 70.0), _active_record("w2", 60.0)]
    client = FakeClient(funders={"w1": None, "w2": None})
    result = _run(cluster_and_dedup(records, helius_client=client))
    assert result is records
    # Both are singletons -> both stay ACTIVE, cluster_id set
    assert all(getattr(r, "cluster_id", None) is not None for r in records)
    assert all(r.status == "ACTIVE" for r in records)


def test_cluster_dedup_single_hop_dedup(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    w_other = _active_record("wother", 70.0)
    records = [w_high, w_low, w_other]
    client = FakeClient(funders={"whigh": "funderX", "wlow": "funderX", "wother": "funderY"})

    class FakeTracker:
        def can_make_request(self, *args, **kwargs):
            return False, "budget denied"

    monkeypatch.setattr("core.helius_credit_tracker.get_credit_tracker", lambda: FakeTracker())
    result = _run(cluster_and_dedup(records, helius_client=client))
    # wlow demoted (same funder as whigh)
    assert result[1].status == "CANDIDATE"
    assert "cluster dedup" in result[1].notes
    assert w_high.status == "ACTIVE"
    assert w_other.status == "ACTIVE"


def test_cluster_dedup_multihop_path(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP_HOPS", "2")
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    # Both wallets' direct funder resolves to the same root "ROOTFUNDER"
    client = FakeClient(funders={"whigh": "direct1", "wlow": "direct2", "direct1": "ROOTFUNDER", "direct2": "ROOTFUNDER"})

    class FakeTracker:
        def can_make_request(self, *args, **kwargs):
            return True, "OK"

    monkeypatch.setattr("core.helius_credit_tracker.get_credit_tracker", lambda: FakeTracker())
    result = _run(cluster_and_dedup(records, helius_client=client))
    assert result[1].status == "CANDIDATE"
    assert "cluster dedup" in result[1].notes


def test_cluster_dedup_root_resolution_exception_falls_back(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]

    class BoomClient:
        async def get_wallet_funder(self, address):
            return "direct1"

    class BoomRootClient(BoomClient):
        pass

    class FakeTracker:
        def can_make_request(self, *args, **kwargs):
            return True, "OK"

    monkeypatch.setattr("core.helius_credit_tracker.get_credit_tracker", lambda: FakeTracker())
    real_resolve = clustering._resolve_funder_root

    async def boom_resolve(client, funder, depth, cache, visited=None):
        raise RuntimeError("resolve failed")

    monkeypatch.setattr(clustering, "_resolve_funder_root", boom_resolve)
    result = _run(cluster_and_dedup(records, helius_client=BoomRootClient()))
    # Falls back to the direct funder: both resolve to "direct1" -> w_low demoted
    assert result is records
    assert w_low.status == "CANDIDATE"
    monkeypatch.setattr(clustering, "_resolve_funder_root", real_resolve)


def test_cluster_dedup_creates_client_with_api_key(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    monkeypatch.setenv("HELIUS_API_KEY", "fake-key")
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    closed = []

    class FakeHelius:
        def __init__(self, api_key=None):
            self.api_key = api_key

        async def get_wallet_funder(self, address):
            return None

        async def close(self):
            closed.append(True)
            raise RuntimeError("close failed")

    class FakeTracker:
        def can_make_request(self, *args, **kwargs):
            return False, "denied"

    monkeypatch.setattr("core.helius_client.HeliusClient", FakeHelius)
    monkeypatch.setattr("core.helius_credit_tracker.get_credit_tracker", lambda: FakeTracker())
    _run(cluster_and_dedup(records))
    assert closed == [True]


def test_cluster_dedup_inner_import_error(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    records = [_active_record("w1", 70.0), _active_record("w2", 60.0)]
    real_import = __import__

    def fake_import(name, *args, **kwargs):
        if name in ("helius_credit_tracker", "helius_client"):
            raise ImportError(f"no {name}")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr("builtins.__import__", fake_import)
    result = _run(cluster_and_dedup(records))
    assert result is records
    assert all(r.status == "ACTIVE" for r in records)


def test_apply_cross_wallet_token_correlation_demoted_skip_and_small_tokens():
    w1 = _active_record("w1", 90.0)
    w2 = _active_record("w2", 80.0)
    w3 = _active_record("w3", 70.0)
    w4 = _active_record("w4", 60.0)
    records = [w1, w2, w3, w4]
    wallet_tokens = {
        "w1": {"t1", "t2"},
        "w2": {"t1", "t3"},
        "w3": {"t1", "t2"},
        "w4": {"t1"},
    }
    demoted = apply_cross_wallet_token_correlation(records, wallet_tokens)
    # w3 demoted by w1 (100% overlap); w2 and w4 stay (low overlap / <2 tokens)
    assert demoted == 1
    assert w3.status == "CANDIDATE"
    assert w2.status == "ACTIVE"
    assert w4.status == "ACTIVE"


def test_cluster_dedup_roster_cap_note(monkeypatch):
    monkeypatch.setenv("SCOUT_CLUSTER_DEDUP", "true")
    records = [_active_record(f"w{i}", 100.0 - i) for i in range(6)]
    client = FakeClient(funders={r.address: None for r in records})
    result = _run(cluster_and_dedup(records, top_n=3, helius_client=client))
    demoted = [r for r in result if r.status == "CANDIDATE"]
    assert len(demoted) == 3
    assert any("roster size cap" in (r.notes or "") for r in demoted)


def test_apply_cross_wallet_token_correlation_insufficient():
    records = [_active_record("w1", 90.0)]
    assert apply_cross_wallet_token_correlation(records, {"w1": {"t1", "t2"}}) == 0


def test_apply_cross_wallet_token_correlation_demotes():
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    w_no_tokens = _active_record("wnone", 50.0)
    records = [w_high, w_low, w_no_tokens]
    wallet_tokens = {
        "whigh": {"t1", "t2", "t3"},
        "wlow": {"t1", "t2"},
    }
    demoted = apply_cross_wallet_token_correlation(records, wallet_tokens)
    assert demoted == 1
    assert w_low.status == "CANDIDATE"
    assert "token overlap" in w_low.notes
    assert w_no_tokens.status == "ACTIVE"


def test_apply_cross_wallet_token_correlation_shared_funder_lower_threshold():
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    wallet_tokens = {
        "whigh": {"t1", "t2"},
        "wlow": {"t1", "t2"},
    }
    funder_map = {"whigh": "FUNDER1", "wlow": "FUNDER1"}
    demoted = apply_cross_wallet_token_correlation(
        records, wallet_tokens, funder_map=funder_map
    )
    assert demoted == 1
    assert "funder overlap" in w_low.notes


def test_apply_cross_wallet_token_correlation_no_overlap():
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    wallet_tokens = {
        "whigh": {"t1", "t2"},
        "wlow": {"t9", "t10"},
    }
    assert apply_cross_wallet_token_correlation(records, wallet_tokens) == 0


def test_apply_cross_wallet_token_correlation_single_token_skipped():
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    wallet_tokens = {"whigh": {"t1"}, "wlow": {"t1", "t2"}}
    assert apply_cross_wallet_token_correlation(records, wallet_tokens) == 0


def test_apply_cross_wallet_token_correlation_already_demoted_skipped():
    w_high = _active_record("whigh", 90.0)
    w_low = _active_record("wlow", 60.0)
    records = [w_high, w_low]
    wallet_tokens = {
        "whigh": {"t1", "t2", "t3"},
        "wlow": {"t1", "t2"},
    }
    apply_cross_wallet_token_correlation(records, wallet_tokens)
    # Second pass: w_low already demoted -> skipped
    assert apply_cross_wallet_token_correlation(records, wallet_tokens) == 0
