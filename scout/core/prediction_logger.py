"""
Prediction Logger for Scout ML Models

Persists ML predictions to database for later validation against actual results.
This module provides the persistence layer for the model validation pipeline.

Usage:
    logger = PredictionLogger(db_path="data/chimera.db")
    prediction_id = logger.log_prediction(
        wallet_address="wallet123",
        predicted_pnl_sol=0.15,
        model_type="xgboost",
        features={"roi_7d": 0.05, "win_rate": 0.6},
        confidence=0.85,
        strategy="SHIELD",
        wqs_score=75.0,
        wqs_components={"roi": 20, "consistency": 15}
    )
"""

import json
import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, Any, Optional, List

from .db import get_connection

logger = logging.getLogger(__name__)


@dataclass
class PredictionRecord:
    """A single prediction record from the database."""
    id: int
    wallet_address: str
    prediction_timestamp: str
    model_type: str
    predicted_pnl_sol: float
    predicted_class: Optional[str]
    confidence: Optional[float]
    features_json: Optional[str]
    strategy: Optional[str]
    wqs_score_at_prediction: Optional[float]
    wqs_components_json: Optional[str]
    actual_pnl_sol: Optional[float]
    actual_pnl_7d_sol: Optional[float]
    actual_pnl_30d_sol: Optional[float]
    match_timestamp: Optional[str]
    days_to_match: Optional[int]
    status: str
    created_at: str
    updated_at: str

    @property
    def features(self) -> Dict[str, Any]:
        """Parse features_json into dict."""
        if self.features_json:
            try:
                return json.loads(self.features_json)
            except json.JSONDecodeError:
                return {}
        return {}

    @property
    def wqs_components(self) -> Dict[str, float]:
        """Parse wqs_components_json into dict."""
        if self.wqs_components_json:
            try:
                return json.loads(self.wqs_components_json)
            except json.JSONDecodeError:
                return {}
        return {}


@dataclass
class MatchingStats:
    """Statistics from a matching operation."""
    total_pending: int
    matched: int
    expired: int
    failed: int
    processing_time_seconds: float


class PredictionLogger:
    """
    Logs ML predictions to database for later validation.

    This class provides methods to:
    - Log new predictions with full context
    - Retrieve predictions by status
    - Update predictions with actual results
    - Manage prediction lifecycle (expire, clean up)
    """

    def __init__(
        self,
        db_path: Optional[str] = None,
        auto_expire_days: int = 90
    ):
        """
        Initialize the prediction logger.

        Args:
            db_path: Path to SQLite database
            auto_expire_days: Days after which predictions are auto-expired
        """
        if db_path is None:
            db_path = "data/chimera.db"

        self.db_path = Path(db_path)
        self.auto_expire_days = auto_expire_days

        # Ensure schema exists
        self._ensure_schema()

    def _get_connection(self):
        """Get a database connection with row factory."""
        if not self.db_path.exists():
            logger.warning(f"Database not found at {self.db_path}")

        conn = get_connection(str(self.db_path))
        return conn

    def _ensure_schema(self):
        """Ensure the ml_predictions table exists.

        The canonical home for this table is the operator's PostgreSQL
        migration (operator/migrations_postgres/0004_ml_predictions.sql),
        which creates it server-side at startup. This method is a best-effort
        fallback for SQLite dev mode and searches several candidate paths so
        it works both in the repo root and when scout is copied into /app
        (where parent.parent.parent resolves to / instead of the repo root).
        """
        try:
            # Search candidate locations: repo root, /app, and the directory
            # two levels up from this file (covers both repo-layout and the
            # container's /app/core/prediction_logger.py layout).
            candidates = [
                Path(__file__).resolve().parent.parent.parent / "database" / "schema" / "ml_predictions.sql",
                Path("/app/database/schema/ml_predictions.sql"),
                Path.cwd() / "database" / "schema" / "ml_predictions.sql",
            ]
            schema_path = next((p for p in candidates if p.exists()), None)

            if schema_path is not None:
                with open(schema_path, 'r') as f:
                    schema_sql = f.read()

                conn = self._get_connection()
                cursor = conn.cursor()

                from .db import translate_ddl
                # Execute schema - split by semicolon and filter out comments
                for statement in schema_sql.split(';'):
                    statement = statement.strip()
                    if statement:
                        # Remove inline comments but keep SQL statements
                        lines = []
                        for line in statement.split('\n'):
                            # Skip full-line comments but keep SQL lines
                            stripped = line.strip()
                            if stripped and not stripped.startswith('--'):
                                lines.append(line)
                        cleaned = '\n'.join(lines).strip()
                        if cleaned:
                            # Translate SQLite DDL (AUTOINCREMENT, etc.) to
                            # PostgreSQL when running on the postgres backend.
                            cursor.execute(translate_ddl(cleaned))

                conn.commit()
                conn.close()
                logger.info("ML predictions schema verified/created")
            else:
                # The operator's PostgreSQL migration creates this table at
                # startup, so a missing schema file is informational only.
                logger.debug(
                    "ml_predictions.sql not found in any candidate path — "
                    "assuming the operator migration created the table"
                )

        except Exception as e:
            logger.error(f"Failed to ensure schema: {e}")

    def log_prediction(
        self,
        wallet_address: str,
        predicted_pnl_sol: float,
        model_type: str,
        features: Dict[str, Any],
        confidence: float,
        strategy: str,
        wqs_score: float,
        wqs_components: Dict[str, float],
        predicted_class: Optional[str] = None
    ) -> Optional[int]:
        """
        Store a prediction in the database.

        Args:
            wallet_address: Wallet identifier
            predicted_pnl_sol: Predicted PnL in SOL
            model_type: Model type (xgboost, lightgbm, meta_learner, etc.)
            features: Feature dictionary used for prediction
            confidence: Prediction confidence score (0-1)
            strategy: Trading strategy (SHIELD, SPEAR)
            wqs_score: WQS score at time of prediction
            wqs_components: WQS component scores
            predicted_class: Optional predicted class label

        Returns:
            Prediction ID if successful, None otherwise
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            now = datetime.utcnow().isoformat()

            cursor.execute(
                """
                INSERT INTO ml_predictions (
                    wallet_address, prediction_timestamp, model_type,
                    predicted_pnl_sol, predicted_class, confidence,
                    features_json, strategy, wqs_score_at_prediction,
                    wqs_components_json, status, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'PENDING', ?, ?)
                """,
                (
                    wallet_address,
                    now,
                    model_type,
                    float(predicted_pnl_sol),
                    predicted_class,
                    float(confidence),
                    json.dumps(features),
                    strategy,
                    float(wqs_score),
                    json.dumps(wqs_components),
                    now,
                    now
                )
            )

            conn.commit()
            prediction_id = cursor.lastrowid
            conn.close()

            logger.debug(f"Logged prediction {prediction_id} for {wallet_address} using {model_type}")
            return prediction_id

        except sqlite3.IntegrityError:
            logger.warning(f"Duplicate prediction for {wallet_address} at {now} with model {model_type}")
            return None
        except Exception as e:
            logger.error(f"Failed to log prediction: {e}")
            return None

    def get_pending_predictions(
        self,
        model_type: Optional[str] = None,
        max_age_days: int = 90,
        limit: Optional[int] = None
    ) -> List[PredictionRecord]:
        """
        Get predictions awaiting actual results.

        Args:
            model_type: Filter by model type
            max_age_days: Maximum age in days
            limit: Maximum number of records to return

        Returns:
            List of pending prediction records
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            # Calculate time threshold
            threshold = (datetime.utcnow() - timedelta(days=max_age_days)).isoformat()

            query = """
                SELECT * FROM ml_predictions
                WHERE status = 'PENDING'
                AND prediction_timestamp >= %s
            """
            params = [threshold]

            if model_type:
                query += " AND model_type = %s"
                params.append(model_type)

            query += " ORDER BY prediction_timestamp DESC"

            if limit:
                query += " LIMIT %s"
                params.append(limit)

            cursor.execute(query, params)

            records = []
            for row in cursor.fetchall():
                records.append(PredictionRecord(**dict(row)))

            conn.close()
            return records

        except Exception as e:
            logger.error(f"Failed to get pending predictions: {e}")
            return []

    def mark_matched(
        self,
        prediction_id: int,
        actual_pnl_sol: float,
        actual_pnl_7d_sol: Optional[float] = None,
        actual_pnl_30d_sol: Optional[float] = None
    ) -> bool:
        """
        Update a prediction with actual results.

        Args:
            prediction_id: Prediction record ID
            actual_pnl_sol: Actual realized PnL in SOL
            actual_pnl_7d_sol: Actual 7-day PnL
            actual_pnl_30d_sol: Actual 30-day PnL

        Returns:
            True if successful, False otherwise
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            # Get prediction timestamp to calculate days_to_match
            cursor.execute(
                "SELECT prediction_timestamp FROM ml_predictions WHERE id = ?",
                (prediction_id,)
            )
            row = cursor.fetchone()

            if not row:
                logger.warning(f"Prediction {prediction_id} not found")
                conn.close()
                return False

            pred_timestamp = datetime.fromisoformat(row['prediction_timestamp'])
            now = datetime.utcnow()
            days_to_match = (now - pred_timestamp).days

            now_iso = now.isoformat()

            cursor.execute(
                """
                UPDATE ml_predictions
                SET actual_pnl_sol = ?,
                    actual_pnl_7d_sol = ?,
                    actual_pnl_30d_sol = ?,
                    match_timestamp = ?,
                    days_to_match = ?,
                    status = 'MATCHED',
                    updated_at = ?
                WHERE id = ?
                """,
                (
                    float(actual_pnl_sol),
                    float(actual_pnl_7d_sol) if actual_pnl_7d_sol is not None else None,
                    float(actual_pnl_30d_sol) if actual_pnl_30d_sol is not None else None,
                    now_iso,
                    days_to_match,
                    now_iso,
                    prediction_id
                )
            )

            conn.commit()
            conn.close()

            logger.debug(f"Marked prediction {prediction_id} as matched")
            return True

        except Exception as e:
            logger.error(f"Failed to mark prediction as matched: {e}")
            return False

    def mark_matched_by_address(
        self,
        wallet_address: str,
        model_type: Optional[str] = None,
        actual_pnl_sol: Optional[float] = None,
        actual_pnl_7d_sol: Optional[float] = None,
        actual_pnl_30d_sol: Optional[float] = None,
        prediction_timestamp: Optional[str] = None
    ) -> int:
        """
        Update predictions by wallet address (useful for batch updates).

        Args:
            wallet_address: Wallet identifier
            model_type: Optional model type filter
            actual_pnl_sol: Actual PnL value
            actual_pnl_7d_sol: Actual 7-day PnL
            actual_pnl_30d_sol: Actual 30-day PnL
            prediction_timestamp: Optional specific prediction timestamp

        Returns:
            Number of predictions updated
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            now = datetime.utcnow().isoformat()

            query = """
                UPDATE ml_predictions
                SET actual_pnl_sol = COALESCE(?, actual_pnl_sol),
                    actual_pnl_7d_sol = COALESCE(?, actual_pnl_7d_sol),
                    actual_pnl_30d_sol = COALESCE(?, actual_pnl_30d_sol),
                    match_timestamp = ?,
                    status = 'MATCHED',
                    updated_at = ?
                WHERE wallet_address = ?
                AND status = 'PENDING'
            """
            params = [
                actual_pnl_sol,
                actual_pnl_7d_sol,
                actual_pnl_30d_sol,
                now,
                now,
                wallet_address
            ]

            if model_type:
                query += " AND model_type = ?"
                params.append(model_type)

            if prediction_timestamp:
                query += " AND prediction_timestamp = ?"
                params.append(prediction_timestamp)

            cursor.execute(query, params)

            updated_count = cursor.rowcount
            conn.commit()
            conn.close()

            logger.debug(f"Marked {updated_count} predictions for {wallet_address} as matched")
            return updated_count

        except Exception as e:
            logger.error(f"Failed to mark predictions as matched by address: {e}")
            return 0

    def mark_expired(self, max_age_days: Optional[int] = None) -> int:
        """
        Mark old predictions as expired.

        Args:
            max_age_days: Age threshold in days (uses self.auto_expire_days if not specified)

        Returns:
            Number of predictions marked as expired
        """
        try:
            if max_age_days is None:
                max_age_days = self.auto_expire_days

            threshold = (datetime.utcnow() - timedelta(days=max_age_days)).isoformat()
            now = datetime.utcnow().isoformat()

            conn = self._get_connection()
            cursor = conn.cursor()

            cursor.execute(
                """
                UPDATE ml_predictions
                SET status = 'EXPIRED', updated_at = ?
                WHERE status = 'PENDING'
                AND prediction_timestamp < ?
                """,
                (now, threshold)
            )

            expired_count = cursor.rowcount
            conn.commit()
            conn.close()

            logger.info(f"Marked {expired_count} predictions as expired (>{max_age_days} days old)")
            return expired_count

        except Exception as e:
            logger.error(f"Failed to mark predictions as expired: {e}")
            return 0

    def get_statistics(self) -> Dict[str, Any]:
        """
        Get summary statistics about predictions.

        Returns:
            Dictionary with prediction statistics
        """
        try:
            conn = self._get_connection()
            cursor = conn.cursor()

            # Count by status
            cursor.execute(
                """
                SELECT status, COUNT(*) as count
                FROM ml_predictions
                GROUP BY status
                """
            )

            status_counts = {row['status']: row['count'] for row in cursor.fetchall()}

            # Count by model type
            cursor.execute(
                """
                SELECT model_type, COUNT(*) as count
                FROM ml_predictions
                GROUP BY model_type
                """
            )

            model_counts = {row['model_type']: row['count'] for row in cursor.fetchall()}

            # Total predictions
            cursor.execute("SELECT COUNT(*) as total FROM ml_predictions")
            total = cursor.fetchone()['total']

            # Matched prediction stats
            cursor.execute(
                """
                SELECT
                    COUNT(*) as matched_count,
                    AVG(actual_pnl_sol) as avg_actual_pnl,
                    AVG(predicted_pnl_sol) as avg_predicted_pnl,
                    AVG(days_to_match) as avg_days_to_match
                FROM ml_predictions
                WHERE status = 'MATCHED'
                """
            )

            matched_stats = cursor.fetchone()

            conn.close()

            return {
                'total_predictions': total,
                'by_status': status_counts,
                'by_model': model_counts,
                'matched_stats': {
                    'count': matched_stats['matched_count'] if matched_stats else 0,
                    'avg_actual_pnl': float(matched_stats['avg_actual_pnl']) if matched_stats and matched_stats['avg_actual_pnl'] else 0.0,
                    'avg_predicted_pnl': float(matched_stats['avg_predicted_pnl']) if matched_stats and matched_stats['avg_predicted_pnl'] else 0.0,
                    'avg_days_to_match': float(matched_stats['avg_days_to_match']) if matched_stats and matched_stats['avg_days_to_match'] else 0.0,
                } if matched_stats else {}
            }

        except Exception as e:
            logger.error(f"Failed to get statistics: {e}")
            return {}

    def cleanup_old_records(self, keep_days: int = 180) -> int:
        """
        Delete very old prediction records.

        Args:
            keep_days: Keep records newer than this many days

        Returns:
            Number of records deleted
        """
        try:
            threshold = (datetime.utcnow() - timedelta(days=keep_days)).isoformat()

            conn = self._get_connection()
            cursor = conn.cursor()

            cursor.execute(
                """
                DELETE FROM ml_predictions
                WHERE prediction_timestamp < ?
                AND status IN ('EXPIRED', 'MATCHED')
                """,
                (threshold,)
            )

            deleted_count = cursor.rowcount
            conn.commit()
            conn.close()

            logger.info(f"Cleaned up {deleted_count} old prediction records")
            return deleted_count

        except Exception as e:
            logger.error(f"Failed to cleanup old records: {e}")
            return 0


# Global instance
_global_logger = None


def get_prediction_logger(db_path: Optional[str] = None) -> PredictionLogger:
    """Get or create global prediction logger instance."""
    global _global_logger
    if _global_logger is None:
        _global_logger = PredictionLogger(db_path)
    return _global_logger


def log_prediction(*args, **kwargs) -> Optional[int]:
    """Log prediction using global logger."""
    logger = get_prediction_logger()
    return logger.log_prediction(*args, **kwargs)
