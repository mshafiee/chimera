"""
Shadow WQS comparison mode.

When SCOUT_WQS_COMPARISON_MODE=true, each wallet is scored under BOTH
the old (full-data, look-ahead contaminated) and new (in-sample only,
clean) WQS calculations. Results are written to a JSONL file and logged
so we can measure whether the split-WQS regime actually improves
promotion outcomes.
"""

import json
import os
from dataclasses import dataclass

from .wqs import calculate_wqs_with_confidence
from .utils import utcnow


@dataclass
class WqsComparisonResult:
    """Comparison between old (contaminated) and new (clean) WQS."""

    wallet_address: str
    timestamp: str
    old_wqs: float
    new_wqs: float
    old_confidence: float
    new_confidence: float
    old_status: str
    new_status: str
    delta: float = 0.0
    promoted_by_new_only: bool = False
    demoted_by_new_only: bool = False

    def __post_init__(self):
        self.delta = self.new_wqs - self.old_wqs
        self.promoted_by_new_only = (
            self.new_status == "ACTIVE" and self.old_status != "ACTIVE"
        )
        self.demoted_by_new_only = (
            self.old_status == "ACTIVE" and self.new_status != "ACTIVE"
        )


def _resolve_status(wqs: float, confidence: float, active_threshold: float = 65.0) -> str:
    """Determine status from WQS + confidence."""
    if wqs >= active_threshold and confidence >= 0.70:
        return "ACTIVE"
    elif wqs >= 20.0:
        return "CANDIDATE"
    return "REJECTED"


def compute_comparison(
    wallet_address: str,
    full_metrics,
    wqs_metrics,
    active_threshold: float = 65.0,
) -> WqsComparisonResult:
    """
    Compute WQS under both regimes and return the comparison.

    Args:
        wallet_address: The wallet being analyzed.
        full_metrics: Metrics computed from ALL available trades (contaminated).
        wqs_metrics: Metrics computed from in-sample trades only (clean).
        active_threshold: Minimum WQS for ACTIVE status.

    Returns:
        WqsComparisonResult with old and new scores.
    """
    old_result = calculate_wqs_with_confidence(full_metrics)
    new_result = calculate_wqs_with_confidence(wqs_metrics)

    old_status = _resolve_status(old_result.score, old_result.confidence, active_threshold)
    new_status = _resolve_status(new_result.score, new_result.confidence, active_threshold)

    return WqsComparisonResult(
        wallet_address=wallet_address,
        timestamp=utcnow().isoformat(),
        old_wqs=old_result.score,
        new_wqs=new_result.score,
        old_confidence=old_result.confidence,
        new_confidence=new_result.confidence,
        old_status=old_status,
        new_status=new_status,
    )


def append_to_log(comparison: WqsComparisonResult):
    """Append a comparison result to the JSONL log file."""
    log_path = os.getenv(
        "SCOUT_WQS_COMPARISON_LOG",
        os.path.join(os.path.dirname(__file__), "..", "data", "wqs_comparison.jsonl"),
    )
    try:
        os.makedirs(os.path.dirname(log_path), exist_ok=True)
        with open(log_path, "a") as f:
            f.write(json.dumps({
                "wallet_address": comparison.wallet_address,
                "timestamp": comparison.timestamp,
                "old_wqs": round(comparison.old_wqs, 1),
                "new_wqs": round(comparison.new_wqs, 1),
                "old_status": comparison.old_status,
                "new_status": comparison.new_status,
                "delta": round(comparison.delta, 1),
                "promoted_by_new_only": comparison.promoted_by_new_only,
                "demoted_by_new_only": comparison.demoted_by_new_only,
            }) + "\n")
    except OSError as e:
        print(f"[Scout] WQS comparison log write failed: {e}")
