"""Hydra Phase 2.2: detector unit tests (no network)."""

from decimal import Decimal

from core.cluster_detector import ClusterBuy, detect_clusters


def _buy(wallet, mint, ts, sol):
    return ClusterBuy(wallet=wallet, mint=mint, ts=float(ts), amount_sol=Decimal(str(sol)))


def test_trigger_three_wallets_120s_40sol():
    buys = [
        _buy("w1", "MINT", 0, 15),
        _buy("w2", "MINT", 60, 15),
        _buy("w3", "MINT", 119, 15),
    ]
    out = detect_clusters(buys, sol_price_usd=Decimal("100"))
    assert len(out) == 1
    assert out[0].wallet_count == 3
    assert out[0].total_cluster_sol == Decimal("45")


def test_no_trigger_outside_window():
    buys = [
        _buy("w1", "MINT", 0, 20),
        _buy("w2", "MINT", 60, 20),
        _buy("w3", "MINT", 300, 20),  # outside 120s of w1
    ]
    # w2..w3 span 240s; only pairs in-window → no triple
    out = detect_clusters(buys, sol_price_usd=Decimal("100"))
    assert out == []


def test_no_trigger_below_volume():
    buys = [
        _buy("w1", "MINT", 0, 1),
        _buy("w2", "MINT", 10, 1),
        _buy("w3", "MINT", 20, 1),
    ]
    out = detect_clusters(buys, sol_price_usd=Decimal("100"))  # $300 < $7500, 3 SOL < 40
    assert out == []


def test_usd_threshold_triggers_without_40sol():
    buys = [
        _buy("w1", "MINT", 0, 10),
        _buy("w2", "MINT", 10, 10),
        _buy("w3", "MINT", 20, 10),
    ]
    # 30 SOL @ $300 = $9000 >= $7500 → trigger
    out = detect_clusters(buys, sol_price_usd=Decimal("300"))
    assert len(out) == 1


def test_shared_root_required_when_clusters_given():
    buys = [
        _buy("a1", "MINT", 0, 15),
        _buy("b1", "MINT", 10, 15),
        _buy("c1", "MINT", 20, 15),
    ]
    # Three singletons → no root has 3 → reject
    out = detect_clusters(buys, wallet_clusters=[{"a1"}, {"b1"}, {"c1"}])
    assert out == []
    # One root with 3 → admit
    out2 = detect_clusters(buys, wallet_clusters=[{"a1", "b1", "c1"}])
    assert len(out2) == 1
