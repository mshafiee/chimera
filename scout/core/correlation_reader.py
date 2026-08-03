"""
WQS-to-Actual-PnL Correlation Reader (Phase 3a / C2 rewrite)

Reads the wqs_pnl_correlation table (written by Scout when promoting wallets)
and computes:

- Per-wallet: actual vs predicted profitability
- Per-WQS-component: which signals correlate with profit
- Per-archetype: which trader types actually profit when copied
- Per-strategy: Shield vs Spear effectiveness

Used by Phase 3b (adaptive weights) and the calibration dashboard.

PostgreSQL-only: uses the shared Connection / execute_query abstraction from
`scout.core.db` with `%s` placeholders and `information_schema.tables` for
existence checks. SQLite and local-path branches were decommissioned (2026-07).
"""

import json
import logging
from dataclasses import dataclass
from typing import Dict, List, Optional

from .db import Connection, execute_query

logger = logging.getLogger(__name__)


@dataclass
class WqsCorrelationRecord:
    """A single correlation record from the database."""
    wallet_address: str
    wqs_score_at_promotion: float
    actual_copy_pnl_7d_sol: Optional[float]
    actual_copy_pnl_30d_sol: Optional[float]
    actual_copy_pnl_all_sol: Optional[float]
    copy_trade_count_7d: int
    copy_trade_count_30d: int
    copy_trade_count_all: int
    strategy: str
    wqs_components_json: Optional[str]
    promoted_at: str
    last_updated_at: str


@dataclass
class CorrelationStats:
    """Aggregated correlation statistics."""
    total_wallets: int
    wallets_with_pnl: int
    mean_pnl_7d_sol: float
    mean_pnl_30d_sol: float
    mean_wqs_at_promotion: float
    strategy_breakdown: Dict[str, Dict[str, float]]  # strategy -> {count, mean_pnl}
    archetype_breakdown: Dict[str, Dict[str, float]]


class CorrelationReader:
    """
    Reads wqs_pnl_correlation data and computes aggregated statistics.

    Usage:
        reader = CorrelationReader()
        stats = reader.get_correlation_stats()
        top_predictors = reader.get_top_component_predictors()
    """

    def __init__(self, db_path: Optional[str] = None):
        # db_path is retained for call-site compatibility but ignored — the
        # shared PostgreSQL pool (DATABASE_URL) is the only supported backend.
        self.db_path = db_path

    def table_exists(self) -> bool:
        """Check wqs_pnl_correlation existence via information_schema (PG only)."""
        try:
            with Connection(self.db_path) as conn:
                cursor = execute_query(
                    conn,
                    """SELECT 1 FROM information_schema.tables
                       WHERE table_schema = 'public'
                         AND table_name = 'wqs_pnl_correlation'
                       LIMIT 1""",
                    None,
                )
                return cursor.fetchone() is not None
        except Exception as e:
            # Log so a live outage isn't misreported as "table missing"
            logger.warning("Failed to check wqs_pnl_correlation existence: %s", e)
            return False

    def get_all_records(
        self,
        strategy: Optional[str] = None,
        min_trades: int = 0,
    ) -> List[WqsCorrelationRecord]:
        try:
            with Connection(self.db_path) as conn:
                # Single SELECT list; the WHERE clause is built conditionally
                # so the 12 columns can never drift between branches.
                base_query = """SELECT wallet_address,
                                  wqs_score_at_promotion,
                                  actual_copy_pnl_7d_sol,
                                  actual_copy_pnl_30d_sol,
                                  actual_copy_pnl_all_sol,
                                  copy_trade_count_7d,
                                  copy_trade_count_30d,
                                  copy_trade_count_all,
                                  strategy,
                                  wqs_components_json,
                                  promoted_at,
                                  last_updated_at
                           FROM wqs_pnl_correlation
                           WHERE copy_trade_count_all >= %s"""
                params: List = [min_trades]
                if strategy:
                    base_query += " AND strategy = %s"
                    params.append(strategy)
                base_query += " ORDER BY promoted_at DESC"

                cursor = execute_query(conn, base_query, tuple(params))
                rows = cursor.fetchall()
                records = []
                for row in rows:
                    records.append(WqsCorrelationRecord(
                        wallet_address=row["wallet_address"],
                        wqs_score_at_promotion=row["wqs_score_at_promotion"],
                        actual_copy_pnl_7d_sol=row["actual_copy_pnl_7d_sol"],
                        actual_copy_pnl_30d_sol=row["actual_copy_pnl_30d_sol"],
                        actual_copy_pnl_all_sol=row["actual_copy_pnl_all_sol"],
                        copy_trade_count_7d=row["copy_trade_count_7d"] or 0,
                        copy_trade_count_30d=row["copy_trade_count_30d"] or 0,
                        copy_trade_count_all=row["copy_trade_count_all"] or 0,
                        strategy=row["strategy"],
                        wqs_components_json=row["wqs_components_json"],
                        promoted_at=row["promoted_at"],
                        last_updated_at=row["last_updated_at"],
                    ))
                return records
        except Exception as e:
            logger.warning("Failed to load wqs_pnl_correlation records: %s", e)
            return []

    def get_correlation_stats(
        self, strategy: Optional[str] = None
    ) -> Optional[CorrelationStats]:
        records = self.get_all_records(strategy=strategy, min_trades=1)
        if not records:
            return None

        pnl_7d_vals = [r.actual_copy_pnl_7d_sol for r in records if r.actual_copy_pnl_7d_sol is not None]
        pnl_30d_vals = [r.actual_copy_pnl_30d_sol for r in records if r.actual_copy_pnl_30d_sol is not None]
        wqs_vals = [r.wqs_score_at_promotion for r in records]

        strategy_counts: Dict[str, Dict[str, float]] = {}
        for r in records:
            s = r.strategy
            if s not in strategy_counts:
                strategy_counts[s] = {
                    "count": 0, "n_with_pnl": 0,
                    "total_pnl_30d": 0.0, "profitable": 0,
                }
            strategy_counts[s]["count"] += 1
            if r.actual_copy_pnl_30d_sol is not None:
                strategy_counts[s]["n_with_pnl"] += 1
                strategy_counts[s]["total_pnl_30d"] += r.actual_copy_pnl_30d_sol
                if r.actual_copy_pnl_30d_sol > 0:
                    strategy_counts[s]["profitable"] += 1

        for s in strategy_counts:
            # Divide only by the wallets that actually have 30d PnL so NULL
            # values are not silently counted as zero-pnl
            c = strategy_counts[s]["n_with_pnl"]
            strategy_counts[s]["mean_pnl_30d"] = strategy_counts[s]["total_pnl_30d"] / c if c else 0.0
            strategy_counts[s]["profit_rate"] = strategy_counts[s]["profitable"] / c if c else 0.0

        return CorrelationStats(
            total_wallets=len(records),
            wallets_with_pnl=len([r for r in records if r.actual_copy_pnl_all_sol is not None]),
            mean_pnl_7d_sol=sum(pnl_7d_vals) / len(pnl_7d_vals) if pnl_7d_vals else 0.0,
            mean_pnl_30d_sol=sum(pnl_30d_vals) / len(pnl_30d_vals) if pnl_30d_vals else 0.0,
            mean_wqs_at_promotion=sum(wqs_vals) / len(wqs_vals) if wqs_vals else 0.0,
            strategy_breakdown=strategy_counts,
            archetype_breakdown={},
        )

    def get_top_component_predictors(
        self, min_samples: int = 5
    ) -> List[Dict[str, float]]:
        """
        Analyze which WQS components best predict actual copy PnL.

        Parses wqs_components_json to extract per-component scores and
        correlates each with actual_pnl_30d_sol. Returns components
        ranked by correlation strength.
        """
        records = self.get_all_records(min_trades=1)
        if len(records) < min_samples:
            return []

        components: Dict[str, List[tuple]] = {}  # comp -> [(value, pnl), ...]

        for r in records:
            if r.actual_copy_pnl_30d_sol is None or r.wqs_components_json is None:
                continue
            try:
                comp_data = json.loads(r.wqs_components_json)
                if isinstance(comp_data, dict):
                    for key, val in comp_data.items():
                        if isinstance(val, (int, float)):
                            # Pair each component value with THIS wallet's PnL
                            # so records missing a component never misalign
                            components.setdefault(key, []).append(
                                (float(val), r.actual_copy_pnl_30d_sol)
                            )
            except (json.JSONDecodeError, ValueError):
                continue

        results = []
        for comp_name, pairs in components.items():
            if len(pairs) < min_samples:
                continue
            comp_vals = [p[0] for p in pairs]
            paired_pnls = [p[1] for p in pairs]
            corr = self._pearson_correlation(comp_vals, paired_pnls)
            results.append({"component": comp_name, "correlation": corr, "samples": len(comp_vals)})

        results.sort(key=lambda x: abs(x["correlation"]), reverse=True)
        return results

    @staticmethod
    def _pearson_correlation(x: List[float], y: List[float]) -> float:
        n = min(len(x), len(y))
        if n < 3:
            return 0.0
        mean_x = sum(x) / n
        mean_y = sum(y) / n
        num = sum((x[i] - mean_x) * (y[i] - mean_y) for i in range(n))
        den_x = sum((v - mean_x) ** 2 for v in x) ** 0.5
        den_y = sum((v - mean_y) ** 2 for v in y) ** 0.5
        if den_x == 0 or den_y == 0:
            return 0.0
        return num / (den_x * den_y)

    def print_correlation_summary(self) -> None:
        """Print a human-readable summary of correlation data."""
        if not self.table_exists():
            print("[Correlation] Table wqs_pnl_correlation does not exist yet.")
            print("[Correlation] It will be populated by the Operator when it closes copy-trades.")
            return

        stats = self.get_correlation_stats()
        if stats is None:
            print("[Correlation] Table exists but contains no data.")
            return

        print("\n[Correlation] WQS-to-Actual-PnL Correlation Summary")
        print(f"  Total wallets tracked: {stats.total_wallets}")
        print(f"  Wallets with PnL data: {stats.wallets_with_pnl}")
        print(f"  Mean 7d PnL: {stats.mean_pnl_7d_sol:.4f} SOL")
        print(f"  Mean 30d PnL: {stats.mean_pnl_30d_sol:.4f} SOL")
        print(f"  Mean WQS at promotion: {stats.mean_wqs_at_promotion:.1f}")
        print("\n  Strategy breakdown:")
        for strategy, data in stats.strategy_breakdown.items():
            print(f"    {strategy}: {int(data['count'])} wallets, "
                  f"mean 30d PnL={data.get('mean_pnl_30d', 0):.4f} SOL, "
                  f"profit rate={data.get('profit_rate', 0)*100:.0f}%")

        top_predictors = self.get_top_component_predictors()
        if top_predictors:
            print("\n  Top WQS component predictors (by correlation with 30d PnL):")
            for i, p in enumerate(top_predictors[:5]):
                print(f"    {i+1}. {p['component']}: r={p['correlation']:.3f} (n={p['samples']})")
