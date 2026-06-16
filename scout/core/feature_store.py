"""
Feature Store for ML (Phase 6a)

Writes per-wallet feature vectors to a time-series CSV file each Scout run.
Enables downstream ML (regression, classification) without re-computing features.

Features (per wallet per run):
- WQS components (12+)
- Wallet age (days)
- Token categories traded
- Time since last trade (days)
- Average entry delay trend
- Liquidity tier
- Archetype
- Trade count
- ROI 7d/30d
- Win rate
- Profit factor
- Sortino ratio
- Max drawdown
- MEV risk score
"""

import csv
import os
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Any


class FeatureStore:
    """
    Time-series feature store for wallet analysis.

    Usage:
        store = FeatureStore("data/features/")
        store.append_run(wallet_records, run_timestamp)
    """

    COLUMNS = [
        "run_timestamp", "wallet_address", "status", "archetype",
        "wqs_score", "roi_7d", "roi_30d", "trade_count_30d", "win_rate",
        "max_drawdown_30d", "avg_trade_size_sol", "profit_factor",
        "sortino_ratio", "avg_entry_delay_seconds", "is_fresh_wallet",
        "dex_diversity_score", "uses_limit_orders", "uses_mev_protection",
        "unique_token_categories", "mev_risk_score",
        "days_since_last_trade", "parse_rate",
        "wmi_score", "wqs_7d", "wqs_14d", "wqs_30d",
        "cluster_id", "cluster_size", "cluster_pnl_avg_sol",
    ]

    def __init__(self, output_dir: str = "data/features"):
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)

    def append_run(
        self,
        records: List[Dict[str, Any]],
        run_timestamp: Optional[datetime] = None,
        wmi_scores: Optional[Dict[str, float]] = None,
        multi_wqs: Optional[Dict[str, Dict[str, float]]] = None,
        cluster_data: Optional[Dict[str, Dict[str, Any]]] = None,
    ) -> str:
        """
        Append features for a Scout run to the feature store CSV.

        Args:
            records: List of wallet feature dicts from main.py
            run_timestamp: ISO timestamp for this run
            wmi_scores: Optional Wallet Momentum Indicator scores (wallet -> wmi)
            multi_wqs: Optional multi-timeframe WQS (wallet -> {7d, 14d, 30d})
            cluster_data: Optional cluster metadata (wallet -> {cluster_id, size, pnl_avg})

        Returns:
            Path to the written CSV file
        """
        if run_timestamp is None:
            run_timestamp = datetime.utcnow()
        ts_str = run_timestamp.isoformat() if isinstance(run_timestamp, datetime) else str(run_timestamp)

        csv_path = self.output_dir / "wallet_features.csv"
        file_exists = csv_path.exists()

        with open(csv_path, "a", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=self.COLUMNS)
            if not file_exists or csv_path.stat().st_size == 0:
                writer.writeheader()

            for rec in records:
                addr = rec.get("address", "")
                wmi = (wmi_scores or {}).get(addr)
                mwqs = (multi_wqs or {}).get(addr, {})
                clu = (cluster_data or {}).get(addr, {})

                row = {
                    "run_timestamp": ts_str,
                    "wallet_address": addr,
                    "status": rec.get("status", ""),
                    "archetype": rec.get("archetype", ""),
                    "wqs_score": rec.get("wqs_score"),
                    "roi_7d": rec.get("roi_7d"),
                    "roi_30d": rec.get("roi_30d"),
                    "trade_count_30d": rec.get("trade_count_30d"),
                    "win_rate": rec.get("win_rate"),
                    "max_drawdown_30d": rec.get("max_drawdown_30d"),
                    "avg_trade_size_sol": rec.get("avg_trade_size_sol"),
                    "profit_factor": rec.get("profit_factor"),
                    "sortino_ratio": rec.get("sortino_ratio"),
                    "avg_entry_delay_seconds": rec.get("avg_entry_delay_seconds"),
                    "is_fresh_wallet": rec.get("is_fresh_wallet"),
                    "dex_diversity_score": rec.get("dex_diversity_score"),
                    "uses_limit_orders": rec.get("uses_limit_orders"),
                    "uses_mev_protection": rec.get("uses_mev_protection"),
                    "unique_token_categories": rec.get("unique_token_categories"),
                    "mev_risk_score": rec.get("mev_risk_score"),
                    "days_since_last_trade": rec.get("days_since_last_trade"),
                    "parse_rate": rec.get("parse_rate"),
                    "wmi_score": wmi,
                    "wqs_7d": mwqs.get("7d"),
                    "wqs_14d": mwqs.get("14d"),
                    "wqs_30d": mwqs.get("30d"),
                    "cluster_id": clu.get("cluster_id"),
                    "cluster_size": clu.get("cluster_size"),
                    "cluster_pnl_avg_sol": clu.get("cluster_pnl_avg_sol"),
                }
                writer.writerow(row)

        return str(csv_path)

    def load_features(self) -> List[Dict[str, Any]]:
        """Load all features from the CSV file."""
        csv_path = self.output_dir / "wallet_features.csv"
        if not csv_path.exists():
            return []
        rows = []
        with open(csv_path, newline="") as f:
            reader = csv.DictReader(f)
            for row in reader:
                for key in row:
                    if key not in ("run_timestamp", "wallet_address", "status", "archetype"):
                        try:
                            row[key] = float(row[key]) if row[key] else None
                        except (ValueError, TypeError):
                            pass
                rows.append(row)
        return rows
