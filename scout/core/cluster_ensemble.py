"""
Cluster-Based Ensemble Scoring (Phase 6d)

Uses funder-clustering data to compute cluster-level metrics and adjusts
individual wallet WQS based on cluster quality. Wallets in profitable
clusters get a bonus; wallets in losing clusters get a penalty.

This reduces correlated risk: if all wallets in a cluster (same funder)
are losing money, the entire cluster is likely a coordinated operation,
and copying any individual is risky.
"""

from typing import Dict, List, Optional, Any


def compute_cluster_scores(
    wallet_records: List[Dict[str, Any]],
    cluster_data: Dict[str, Dict[str, Any]],
) -> Dict[str, Dict[str, float]]:
    """
    Compute cluster-level ensemble metrics.

    Args:
        wallet_records: List of wallet record dicts with address, wqs_score, roi_30d, etc.
        cluster_data: Dict mapping wallet_address -> {cluster_id, cluster_size, members}

    Returns:
        Dict mapping cluster_id -> {size, mean_wqs, mean_roi, profit_rate, risk_score}
    """
    if not cluster_data:
        return {}

    # Group wallets by cluster
    clusters: Dict[str, List[Dict[str, Any]]] = {}
    wallet_to_cluster: Dict[str, str] = {}

    for waddr, cinfo in cluster_data.items():
        cid = cinfo.get("cluster_id", waddr)  # Default to self
        wallet_to_cluster[waddr] = cid
        clusters.setdefault(cid, [])

    for rec in wallet_records:
        addr = rec.get("address", "")
        cid = wallet_to_cluster.get(addr, addr)
        clusters.setdefault(cid, []).append(rec)

    cluster_metrics: Dict[str, Dict[str, float]] = {}

    for cid in list(clusters.keys()):
        members = clusters[cid]

        # Size the cluster from cluster_data metadata (members list /
        # cluster_size) so a filtered/partial record list can't mis-size it
        declared_members: List[str] = []
        declared_size: Optional[int] = None
        for waddr, cinfo in cluster_data.items():
            if cinfo.get("cluster_id", waddr) != cid:
                continue
            members_list = cinfo.get("members")
            if isinstance(members_list, list) and members_list:
                declared_members = members_list
            size = cinfo.get("cluster_size")
            if isinstance(size, (int, float)) and size > 0:
                declared_size = max(declared_size or 0, int(size))

        if declared_size is not None:
            n = max(1, declared_size)
        elif declared_members:
            n = max(1, len(declared_members))
        else:
            n = max(1, len(members))

        # Skip clusters with no actual member records — an all-zero summary
        # would mislead consumers
        if not members:
            continue

        # Aggregate metrics only over members that actually have the metric,
        # so missing data doesn't masquerade as poor performance
        wqs_members = [m for m in members if m.get("wqs_score") is not None]
        roi_members = [m for m in members if m.get("roi_30d") is not None]
        pf_members = [m for m in members if m.get("profit_factor") is not None]

        mean_wqs = sum(m.get("wqs_score", 0.0) for m in wqs_members) / len(wqs_members) if wqs_members else 0.0
        mean_roi = sum(m.get("roi_30d", 0.0) for m in roi_members) / len(roi_members) if roi_members else 0.0
        mean_pf = sum(m.get("profit_factor", 0.0) for m in pf_members) / len(pf_members) if pf_members else 1.0
        profitable = sum(1 for m in roi_members if m.get("roi_30d") > 0)
        profit_rate = profitable / len(roi_members) if roi_members else 0.0

        # Cluster risk score: high when members have correlated low performance.
        # The mean-ROI / profit-factor adders only fire when real data exists.
        if n > 1:
            risk_score = 1.0 - profit_rate
            if roi_members and mean_roi < 0:
                risk_score += 0.3  # Extra penalty for negative mean ROI
            if pf_members and mean_pf < 1.0:
                risk_score += 0.2  # Losing cluster
        else:
            risk_score = 0.0  # Solo wallet has no cluster risk

        cluster_metrics[cid] = {
            "size": float(n),
            "mean_wqs": mean_wqs,
            "mean_roi": mean_roi,
            "mean_profit_factor": mean_pf,
            "profit_rate": profit_rate,
            "risk_score": min(1.0, risk_score),
        }

    return cluster_metrics


def apply_cluster_adjustment(
    wqs_score: float,
    wallet_address: str,
    cluster_data: Optional[Dict[str, Dict[str, Any]]] = None,
    cluster_metrics: Optional[Dict[str, Dict[str, float]]] = None,
) -> float:
    """
    Adjust individual WQS based on cluster ensemble metrics.

    A wallet in a strong cluster (high profit rate, good mean WQS) gets
    a small bonus. A wallet in a risky cluster (low profit rate, negative
    mean ROI) gets a penalty.

    Args:
        wqs_score: Individual wallet WQS (0-100)
        wallet_address: Wallet address
        cluster_data: Per-wallet cluster metadata
        cluster_metrics: Pre-computed cluster metrics

    Returns:
        Adjusted WQS (0-100)
    """
    if not cluster_data or not cluster_metrics:
        return wqs_score

    cinfo = cluster_data.get(wallet_address)
    if not cinfo:
        return wqs_score

    cid = cinfo.get("cluster_id", wallet_address)
    cmetrics = cluster_metrics.get(cid)
    if not cmetrics:
        return wqs_score

    n = cmetrics.get("size", 1)
    profit_rate = cmetrics.get("profit_rate", 0.5)
    risk_score = cmetrics.get("risk_score", 0.0)

    if n <= 1:
        return wqs_score  # No cluster data for solo wallets

    # Bonus for high-quality clusters; mutually exclusive with the penalty so
    # a profitable cluster is never penalized by the risk adders at the same time
    if profit_rate > 0.8 and cmetrics.get("mean_wqs", 0) > 60:
        wqs_score += 5.0
    elif risk_score > 0.5:
        wqs_score -= risk_score * 15.0  # Up to -15 for max risk

    return max(0.0, min(100.0, wqs_score))
