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

    for cid, members in clusters.items():
        n = max(1, len(members))
        wqs_scores = [m.get("wqs_score", 0.0) or 0.0 for m in members]
        roi_vals = [m.get("roi_30d", 0.0) or 0.0 for m in members]
        pf_vals = [m.get("profit_factor", 0.0) or 0.0 for m in members]

        mean_wqs = sum(wqs_scores) / n if wqs_scores else 0.0
        mean_roi = sum(roi_vals) / n if roi_vals else 0.0
        mean_pf = sum(pf_vals) / n if pf_vals else 1.0
        profitable = sum(1 for r in roi_vals if r > 0)
        profit_rate = profitable / n if n > 0 else 0.0

        # Cluster risk score: high when members have correlated low performance
        if n > 1:
            risk_score = 1.0 - profit_rate
            if mean_roi < 0:
                risk_score += 0.3  # Extra penalty for negative mean ROI
            if mean_pf < 1.0:
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

    # Bonus for high-quality clusters
    if profit_rate > 0.8 and cmetrics.get("mean_wqs", 0) > 60:
        wqs_score += 5.0

    # Penalty for high-risk clusters
    if risk_score > 0.5:
        wqs_score -= risk_score * 15.0  # Up to -15 for max risk

    return max(0.0, min(100.0, wqs_score))
