"""
Prediction Matcher for Scout ML Models

Matches ML predictions to actual trading results from the correlation table.
This module bridges predictions made at analysis time with actual PnL results
recorded by the Rust Operator when copy-trades close.

Usage:
    matcher = PredictionMatcher(db_path="data/chimera.db")
    results = matcher.match_predictions_to_actuals(lookback_days=7)
    print(f"Matched {results.matched_count} predictions")
"""

import logging
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional, List

from scout.core.prediction_logger import PredictionLogger, PredictionRecord
from scout.core.correlation_reader import CorrelationReader, WqsCorrelationRecord
from .db import get_connection

logger = logging.getLogger(__name__)


@dataclass
class MatchingResults:
    """Results from a prediction matching operation."""
    total_pending: int
    matched_count: int
    failed_count: int
    skipped_count: int
    processing_time_seconds: float
    model_type: Optional[str]
    lookback_days: int
    timestamp: str

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return asdict(self)


@dataclass
class MatchedPrediction:
    """A successfully matched prediction with its actual result."""
    prediction_id: int
    wallet_address: str
    model_type: str
    predicted_pnl_sol: float
    actual_pnl_sol: float
    prediction_timestamp: str
    match_timestamp: str
    days_to_match: int
    error: float
    abs_error: float
    direction_correct: bool


class PredictionMatcher:
    """
    Matches ML predictions to actual trading results.

    This class:
    - Retrieves pending predictions from the database
    - Queries the wqs_pnl_correlation table for actual results
    - Matches predictions to actuals by wallet and time window
    - Updates predictions with actual values
    - Calculates match statistics
    """

    def __init__(
        self,
        db_path: Optional[str] = None,
        correlation_reader: Optional[CorrelationReader] = None
    ):
        """
        Initialize the prediction matcher.

        Args:
            db_path: Path to SQLite database
            correlation_reader: Optional CorrelationReader instance
        """
        if db_path is None:
            db_path = "data/chimera.db"

        self.db_path = Path(db_path)
        self.prediction_logger = PredictionLogger(db_path)
        self.correlation_reader = correlation_reader or CorrelationReader(db_path)

    def match_predictions_to_actuals(
        self,
        lookback_days: int = 7,
        model_type: Optional[str] = None,
        match_window_days: int = 14,
        dry_run: bool = False
    ) -> MatchingResults:
        """
        Match pending predictions to actual PnL from correlation table.

        Args:
            lookback_days: How far back to look for predictions to match
            model_type: Optional model type filter
            match_window_days: Maximum days between prediction and actual result
            dry_run: If True, don't actually update predictions

        Returns:
            MatchingResults with statistics
        """
        start_time = datetime.utcnow()

        logger.info(f"Starting prediction matching (lookback: {lookback_days}d, window: {match_window_days}d)")

        # Get pending predictions
        pending_predictions = self.prediction_logger.get_pending_predictions(
            model_type=model_type,
            max_age_days=lookback_days + match_window_days,
            limit=None
        )

        if not pending_predictions:
            logger.info("No pending predictions to match")
            return MatchingResults(
                total_pending=0,
                matched_count=0,
                failed_count=0,
                skipped_count=0,
                processing_time_seconds=0.0,
                model_type=model_type,
                lookback_days=lookback_days,
                timestamp=datetime.utcnow().isoformat()
            )

        logger.info(f"Found {len(pending_predictions)} pending predictions")

        matched_count = 0
        failed_count = 0
        skipped_count = 0
        matched_predictions = []

        # Check if correlation table exists
        if not self.correlation_reader.table_exists():
            logger.warning("Correlation table does not exist yet - skipping matching")
            return MatchingResults(
                total_pending=len(pending_predictions),
                matched_count=0,
                failed_count=0,
                skipped_count=len(pending_predictions),
                processing_time_seconds=(datetime.utcnow() - start_time).total_seconds(),
                model_type=model_type,
                lookback_days=lookback_days,
                timestamp=datetime.utcnow().isoformat()
            )

        # Get correlation records
        try:
            correlation_records = self.correlation_reader.get_all_records()
            logger.info(f"Found {len(correlation_records)} correlation records")

            # Build lookup: wallet_address -> list of records
            correlation_by_wallet = {}
            for record in correlation_records:
                if record.wallet_address not in correlation_by_wallet:
                    correlation_by_wallet[record.wallet_address] = []
                correlation_by_wallet[record.wallet_address].append(record)

        except Exception as e:
            logger.error(f"Failed to load correlation records: {e}")
            return MatchingResults(
                total_pending=len(pending_predictions),
                matched_count=0,
                failed_count=0,
                skipped_count=len(pending_predictions),
                processing_time_seconds=(datetime.utcnow() - start_time).total_seconds(),
                model_type=model_type,
                lookback_days=lookback_days,
                timestamp=datetime.utcnow().isoformat()
            )

        # Match each prediction
        for prediction in pending_predictions:
            try:
                # Find correlation record for this wallet
                wallet_records = correlation_by_wallet.get(prediction.wallet_address, [])

                if not wallet_records:
                    skipped_count += 1
                    continue

                # Find the best matching record based on timing
                best_match = self._find_best_match(
                    prediction,
                    wallet_records,
                    match_window_days
                )

                if best_match:
                    # Determine which actual PnL to use based on time window
                    actual_pnl = self._select_actual_pnl(
                        best_match,
                        prediction,
                        lookback_days
                    )

                    if not dry_run:
                        # Update the prediction
                        success = self.prediction_logger.mark_matched(
                            prediction_id=prediction.id,
                            actual_pnl_sol=actual_pnl['total'],
                            actual_pnl_7d_sol=actual_pnl.get('7d'),
                            actual_pnl_30d_sol=actual_pnl.get('30d')
                        )

                        if success:
                            matched_count += 1

                            # Create matched prediction record
                            matched_pred = MatchedPrediction(
                                prediction_id=prediction.id,
                                wallet_address=prediction.wallet_address,
                                model_type=prediction.model_type,
                                predicted_pnl_sol=prediction.predicted_pnl_sol,
                                actual_pnl_sol=actual_pnl['total'],
                                prediction_timestamp=prediction.prediction_timestamp,
                                match_timestamp=datetime.utcnow().isoformat(),
                                days_to_match=(datetime.utcnow() - datetime.fromisoformat(prediction.prediction_timestamp)).days,
                                error=actual_pnl['total'] - prediction.predicted_pnl_sol,
                                abs_error=abs(actual_pnl['total'] - prediction.predicted_pnl_sol),
                                direction_correct=self._check_direction_correct(
                                    prediction.predicted_pnl_sol,
                                    actual_pnl['total']
                                )
                            )
                            matched_predictions.append(matched_pred)
                        else:
                            failed_count += 1
                    else:
                        # Dry run - just count
                        matched_count += 1
                else:
                    skipped_count += 1

            except Exception as e:
                logger.warning(f"Failed to match prediction {prediction.id}: {e}")
                failed_count += 1

        processing_time = (datetime.utcnow() - start_time).total_seconds()

        logger.info(
            f"Matching complete: {matched_count} matched, "
            f"{skipped_count} skipped, {failed_count} failed "
            f"({processing_time:.2f}s)"
        )

        return MatchingResults(
            total_pending=len(pending_predictions),
            matched_count=matched_count,
            failed_count=failed_count,
            skipped_count=skipped_count,
            processing_time_seconds=processing_time,
            model_type=model_type,
            lookback_days=lookback_days,
            timestamp=datetime.utcnow().isoformat()
        )

    def _find_best_match(
        self,
        prediction: PredictionRecord,
        wallet_records: List[WqsCorrelationRecord],
        match_window_days: int
    ) -> Optional[WqsCorrelationRecord]:
        """
        Find the best matching correlation record for a prediction.

        Args:
            prediction: The prediction to match
            wallet_records: List of correlation records for the wallet
            match_window_days: Maximum days between prediction and actual

        Returns:
            Best matching record or None
        """
        pred_timestamp = datetime.fromisoformat(prediction.prediction_timestamp)

        best_match = None
        best_days_diff = float('inf')

        for record in wallet_records:
            try:
                promoted_at = datetime.fromisoformat(record.promoted_at)

                # Calculate time difference
                days_diff = abs((promoted_at - pred_timestamp).days)

                # Check if within window
                if days_diff <= match_window_days:
                    # Prefer records with promotion date closest to prediction
                    if days_diff < best_days_diff:
                        best_days_diff = days_diff
                        best_match = record

            except (ValueError, TypeError) as e:
                logger.debug(f"Invalid timestamp in correlation record: {e}")
                continue

        return best_match

    def _select_actual_pnl(
        self,
        correlation_record: WqsCorrelationRecord,
        prediction: PredictionRecord,
        lookback_days: int
    ) -> Dict[str, float]:
        """
        Select the appropriate actual PnL based on time window.

        Args:
            correlation_record: The matched correlation record
            prediction: The prediction being matched
            lookback_days: Target time window

        Returns:
            Dict with 'total', '7d', and '30d' PnL values
        """
        result = {}

        # Use 7d PnL for 7-day window, 30d for 30-day window
        if lookback_days <= 7:
            result['7d'] = correlation_record.actual_copy_pnl_7d_sol or 0.0
            result['total'] = result['7d']
        elif lookback_days <= 30:
            result['30d'] = correlation_record.actual_copy_pnl_30d_sol or 0.0
            result['total'] = result['30d']
        else:
            # Use all-time PnL
            result['total'] = correlation_record.actual_copy_pnl_all_sol or 0.0

        # Fill in other values if available
        if correlation_record.actual_copy_pnl_7d_sol is not None:
            result['7d'] = correlation_record.actual_copy_pnl_7d_sol

        if correlation_record.actual_copy_pnl_30d_sol is not None:
            result['30d'] = correlation_record.actual_copy_pnl_30d_sol

        if correlation_record.actual_copy_pnl_all_sol is not None and 'total' not in result:
            result['total'] = correlation_record.actual_copy_pnl_all_sol

        return result

    @staticmethod
    def _check_direction_correct(predicted: float, actual: float) -> bool:
        """Check if prediction got the direction (sign) correct."""
        return (predicted > 0 and actual > 0) or (predicted < 0 and actual < 0)

    def get_matched_predictions(
        self,
        model_type: Optional[str] = None,
        limit: Optional[int] = None
    ) -> List[MatchedPrediction]:
        """
        Get previously matched predictions.

        Args:
            model_type: Optional model type filter
            limit: Maximum number of records to return

        Returns:
            List of matched predictions
        """
        try:
            conn = get_connection(str(self.db_path))
            cursor = conn.cursor()

            query = """
                SELECT * FROM ml_predictions
                WHERE status = 'MATCHED'
            """
            params = []

            if model_type:
                query += " AND model_type = ?"
                params.append(model_type)

            query += " ORDER BY match_timestamp DESC"

            if limit:
                query += " LIMIT ?"
                params.append(limit)

            cursor.execute(query, params)

            matched_predictions = []
            for row in cursor.fetchall():
                row_dict = dict(row)
                actual_pnl = row_dict.get('actual_pnl_sol', 0.0) or 0.0
                predicted_pnl = row_dict.get('predicted_pnl_sol', 0.0) or 0.0

                matched_predictions.append(MatchedPrediction(
                    prediction_id=row_dict['id'],
                    wallet_address=row_dict['wallet_address'],
                    model_type=row_dict['model_type'],
                    predicted_pnl_sol=predicted_pnl,
                    actual_pnl_sol=actual_pnl,
                    prediction_timestamp=row_dict['prediction_timestamp'],
                    match_timestamp=row_dict.get('match_timestamp', ''),
                    days_to_match=row_dict.get('days_to_match', 0),
                    error=actual_pnl - predicted_pnl,
                    abs_error=abs(actual_pnl - predicted_pnl),
                    direction_correct=(predicted_pnl > 0 and actual_pnl > 0) or (predicted_pnl < 0 and actual_pnl < 0)
                ))

            conn.close()
            return matched_predictions

        except Exception as e:
            logger.error(f"Failed to get matched predictions: {e}")
            return []

    def get_match_summary(
        self,
        model_type: Optional[str] = None
    ) -> Dict[str, Any]:
        """
        Get summary statistics for matched predictions.

        Args:
            model_type: Optional model type filter

        Returns:
            Dictionary with summary statistics
        """
        matched = self.get_matched_predictions(model_type)

        if not matched:
            return {
                'total_matched': 0,
                'mean_error': 0.0,
                'mean_abs_error': 0.0,
                'direction_accuracy': 0.0,
                'positive_predictions': 0,
                'negative_predictions': 0,
            }

        import numpy as np

        errors = [m.error for m in matched]
        abs_errors = [m.abs_error for m in matched]
        direction_correct = [m.direction_correct for m in matched]

        positive_preds = sum(1 for m in matched if m.predicted_pnl_sol > 0)
        negative_preds = sum(1 for m in matched if m.predicted_pnl_sol < 0)

        return {
            'total_matched': len(matched),
            'mean_error': float(np.mean(errors)),
            'mean_abs_error': float(np.mean(abs_errors)),
            'std_error': float(np.std(errors)),
            'direction_accuracy': float(np.mean(direction_correct)),
            'positive_predictions': positive_preds,
            'negative_predictions': negative_preds,
            'mean_predicted_pnl': float(np.mean([m.predicted_pnl_sol for m in matched])),
            'mean_actual_pnl': float(np.mean([m.actual_pnl_sol for m in matched])),
        }


# Global instance
_global_matcher = None


def get_prediction_matcher(db_path: Optional[str] = None) -> PredictionMatcher:
    """Get or create global prediction matcher instance."""
    global _global_matcher
    if _global_matcher is None:
        _global_matcher = PredictionMatcher(db_path)
    return _global_matcher
