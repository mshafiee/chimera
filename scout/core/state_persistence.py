"""
Cross-Session State Persistence for Learning Continuity

This module implements persistent state management for better learning continuity
across Scout runs, enabling predictive budget manager to use historical patterns.

Features:
- Credit history tracking for forecasting
- Wallet performance persistence
- ROI metrics persistence
- Configuration state persistence
- Cross-session learning

State Schema:
- credit_history: Daily credit usage by category
- wallet_performance_history: Long-term wallet performance
- roi_metrics: ROI by category and wallet band
- configuration_state: Persistent configuration
"""

import time
import logging
from datetime import datetime
from typing import Dict, List, Optional, Any
from dataclasses import dataclass
from enum import Enum
import threading
from pathlib import Path
from contextlib import contextmanager

from .db import get_connection, translate_ddl

# Import for multi-timeframe discovery tracking (avoid circular import)
try:
    from .multitimeframe_discovery import DiscoveryTimeframe, MultiTimeframeResult
except ImportError:
    # Fallback if multitimeframe_discovery not available
    DiscoveryTimeframe = None
    MultiTimeframeResult = None

logger = logging.getLogger(__name__)


class BudgetCategory(Enum):
    """Budget categories for credit tracking."""
    DISCOVERY = "discovery"
    ANALYSIS = "analysis"
    VALIDATION = "validation"
    ENRICHMENT = "enrichment"
    MONITORING = "monitoring"
    RESERVE = "reserve"


@dataclass
class CreditHistory:
    """Daily credit history record."""
    date: str  # YYYY-MM-DD format
    total_credits: int
    credits_by_category: Dict[str, int]
    credits_remaining: int
    day_of_month: int
    timestamp: float


@dataclass
class WalletPerformance:
    """Long-term wallet performance record."""
    wallet_address: str
    wqs_score: float
    total_trades: int
    winning_trades: int
    total_pnl: float
    avg_pnl: float
    win_rate: float
    roi_score: float  # value / credits
    first_seen: float
    last_updated: float


@dataclass
class ROIMetrics:
    """ROI metrics by category."""
    category: str
    credits_consumed: int
    value_generated: float
    roi_score: float
    operations_count: int
    period_start: float
    period_end: float


@dataclass
class PersistenceConfig:
    """Configuration for state persistence."""
    db_path: str = "scout_persistence.db"
    max_history_days: int = 90  # Keep 90 days of history
    backup_enabled: bool = True
    backup_interval_hours: int = 24
    vacuum_interval_days: int = 7


class StatePersistence:
    """
    Cross-session state persistence for Scout.

    Features:
    - SQLite database for persistent storage
    - Credit history tracking
    - Wallet performance persistence
    - ROI metrics tracking
    - Automatic backups
    - Scheduled vacuum
    """

    def __init__(self, config: Optional[PersistenceConfig] = None):
        """Initialize the state persistence manager."""
        self._config = config or PersistenceConfig()
        self._lock = threading.Lock()
        self._db_conn = None

        # Initialize database
        self._init_database()

        logger.info(f"StatePersistence initialized with db: {self._config.db_path}")

    def _get_db_path(self) -> str:
        """Get full database path."""
        # Store in scout directory
        scout_dir = Path(__file__).parent.parent
        return str(scout_dir / self._config.db_path)

    @contextmanager
    def _get_connection(self):
        """Get database connection with context manager."""
        conn = get_connection(self._get_db_path())
        # row_factory is already set by get_connection() for SQLite
        # WAL mode already enabled by get_connection()
        try:
            yield conn
            conn.commit()
        finally:
            conn.close()

    def _init_database(self) -> None:
        """Initialize database schema."""
        with self._get_connection() as conn:
            # Credit history table
            conn.execute(translate_ddl("""
                CREATE TABLE IF NOT EXISTS credit_history (
                    date TEXT PRIMARY KEY,
                    total_credits INTEGER NOT NULL,
                    credits_discovery INTEGER DEFAULT 0,
                    credits_analysis INTEGER DEFAULT 0,
                    credits_validation INTEGER DEFAULT 0,
                    credits_enrichment INTEGER DEFAULT 0,
                    credits_monitoring INTEGER DEFAULT 0,
                    credits_reserve INTEGER DEFAULT 0,
                    credits_remaining INTEGER NOT NULL,
                    day_of_month INTEGER NOT NULL,
                    timestamp REAL NOT NULL
                )
            """))

            # Wallet performance history table
            conn.execute(translate_ddl("""
                CREATE TABLE IF NOT EXISTS wallet_performance_history (
                    wallet_address TEXT PRIMARY KEY,
                    wqs_score REAL NOT NULL,
                    total_trades INTEGER DEFAULT 0,
                    winning_trades INTEGER DEFAULT 0,
                    total_pnl REAL DEFAULT 0.0,
                    avg_pnl REAL DEFAULT 0.0,
                    win_rate REAL DEFAULT 0.0,
                    roi_score REAL DEFAULT 0.0,
                    first_seen REAL NOT NULL,
                    last_updated REAL NOT NULL
                )
            """))

            # ROI metrics table
            conn.execute(translate_ddl("""
                CREATE TABLE IF NOT EXISTS roi_metrics (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    category TEXT NOT NULL,
                    credits_consumed INTEGER NOT NULL,
                    value_generated REAL NOT NULL,
                    roi_score REAL NOT NULL,
                    operations_count INTEGER NOT NULL,
                    period_start REAL NOT NULL,
                    period_end REAL NOT NULL,
                    timestamp REAL DEFAULT (strftime('%s', 'now'))
                )
            """))

            # Multi-timeframe discovery statistics table (Sprint 4)
            conn.execute(translate_ddl("""
                CREATE TABLE IF NOT EXISTS multi_timeframe_discovery_stats (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    discovery_timestamp REAL NOT NULL,
                    total_unique_wallets INTEGER NOT NULL,
                    total_raw_wallets INTEGER NOT NULL,
                    deduplication_ratio REAL NOT NULL,
                    multi_timeframe_wallets INTEGER NOT NULL,
                    total_credits_consumed INTEGER NOT NULL,
                    total_execution_time_seconds REAL NOT NULL,
                    deep_wallets_discovered INTEGER DEFAULT 0,
                    deep_execution_time REAL DEFAULT 0.0,
                    deep_credits_consumed INTEGER DEFAULT 0,
                    fast_wallets_discovered INTEGER DEFAULT 0,
                    fast_execution_time REAL DEFAULT 0.0,
                    fast_credits_consumed INTEGER DEFAULT 0,
                    trending_wallets_discovered INTEGER DEFAULT 0,
                    trending_execution_time REAL DEFAULT 0.0,
                    trending_credits_consumed INTEGER DEFAULT 0,
                    cross_timeframe_quality_avg REAL DEFAULT 0.0,
                    parallel_execution BOOLEAN DEFAULT 0,
                    discovery_goal TEXT DEFAULT 'balanced',
                    created_at REAL DEFAULT (strftime('%s', 'now'))
                )
            """))

            # Create indexes
            conn.execute("CREATE INDEX IF NOT EXISTS idx_credit_history_timestamp ON credit_history(timestamp)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_wallet_performance_updated ON wallet_performance_history(last_updated)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_roi_metrics_category ON roi_metrics(category)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_roi_metrics_timestamp ON roi_metrics(timestamp)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_multi_timeframe_discovery_timestamp ON multi_timeframe_discovery_stats(discovery_timestamp)")

            logger.info("Database schema initialized")

    def save_credit_history(self, history: CreditHistory) -> None:
        """
        Save daily credit history record.

        Args:
            history: Credit history record to save
        """
        with self._lock:
            with self._get_connection() as conn:
                conn.execute("""
                    INSERT OR REPLACE INTO credit_history
                    (date, total_credits, credits_discovery, credits_analysis,
                     credits_validation, credits_enrichment, credits_monitoring,
                     credits_reserve, credits_remaining, day_of_month, timestamp)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, (
                    history.date,
                    history.total_credits,
                    history.credits_by_category.get('discovery', 0),
                    history.credits_by_category.get('analysis', 0),
                    history.credits_by_category.get('validation', 0),
                    history.credits_by_category.get('enrichment', 0),
                    history.credits_by_category.get('monitoring', 0),
                    history.credits_by_category.get('reserve', 0),
                    history.credits_remaining,
                    history.day_of_month,
                    history.timestamp,
                ))

                logger.debug(f"Saved credit history for {history.date}")

    def load_credit_history(self, days: int = 30) -> List[CreditHistory]:
        """
        Load credit history for the last N days.

        Args:
            days: Number of days to load

        Returns:
            List of credit history records
        """
        with self._lock:
            cutoff = time.time() - (days * 86400)

            with self._get_connection() as conn:
                cursor = conn.execute("""
                    SELECT date, total_credits, credits_discovery, credits_analysis,
                           credits_validation, credits_enrichment, credits_monitoring,
                           credits_reserve, credits_remaining, day_of_month, timestamp
                    FROM credit_history
                    WHERE timestamp >= ?
                    ORDER BY timestamp DESC
                """, (cutoff,))

                records = []
                for row in cursor:
                    record = CreditHistory(
                        date=row['date'],
                        total_credits=row['total_credits'],
                        credits_by_category={
                            'discovery': row['credits_discovery'],
                            'analysis': row['credits_analysis'],
                            'validation': row['credits_validation'],
                            'enrichment': row['credits_enrichment'],
                            'monitoring': row['credits_monitoring'],
                            'reserve': row['credits_reserve'],
                        },
                        credits_remaining=row['credits_remaining'],
                        day_of_month=row['day_of_month'],
                        timestamp=row['timestamp'],
                    )
                    records.append(record)

                logger.debug(f"Loaded {len(records)} days of credit history")
                return records

    def save_wallet_performance(self, performance: WalletPerformance) -> None:
        """
        Save wallet performance record.

        Args:
            performance: Wallet performance to save
        """
        with self._lock:
            with self._get_connection() as conn:
                conn.execute("""
                    INSERT OR REPLACE INTO wallet_performance_history
                    (wallet_address, wqs_score, total_trades, winning_trades,
                     total_pnl, avg_pnl, win_rate, roi_score, first_seen, last_updated)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, (
                    performance.wallet_address,
                    performance.wqs_score,
                    performance.total_trades,
                    performance.winning_trades,
                    performance.total_pnl,
                    performance.avg_pnl,
                    performance.win_rate,
                    performance.roi_score,
                    performance.first_seen,
                    performance.last_updated,
                ))

                logger.debug(f"Saved wallet performance for {performance.wallet_address[:8]}...")

    def load_wallet_performance(self, wallet_address: Optional[str] = None) -> Dict[str, WalletPerformance]:
        """
        Load wallet performance records.

        Args:
            wallet_address: Specific wallet to load, or None for all

        Returns:
            Dict mapping wallet address to performance record
        """
        with self._lock:
            with self._get_connection() as conn:
                if wallet_address:
                    cursor = conn.execute("""
                        SELECT wallet_address, wqs_score, total_trades, winning_trades,
                               total_pnl, avg_pnl, win_rate, roi_score, first_seen, last_updated
                        FROM wallet_performance_history
                        WHERE wallet_address = ?
                    """, (wallet_address,))
                else:
                    cursor = conn.execute("""
                        SELECT wallet_address, wqs_score, total_trades, winning_trades,
                               total_pnl, avg_pnl, win_rate, roi_score, first_seen, last_updated
                        FROM wallet_performance_history
                    """)

                records = {}
                for row in cursor:
                    record = WalletPerformance(
                        wallet_address=row['wallet_address'],
                        wqs_score=row['wqs_score'],
                        total_trades=row['total_trades'],
                        winning_trades=row['winning_trades'],
                        total_pnl=row['total_pnl'],
                        avg_pnl=row['avg_pnl'],
                        win_rate=row['win_rate'],
                        roi_score=row['roi_score'],
                        first_seen=row['first_seen'],
                        last_updated=row['last_updated'],
                    )
                    records[record.wallet_address] = record

                logger.debug(f"Loaded {len(records)} wallet performance records")
                return records

    def save_roi_metrics(self, metrics: ROIMetrics) -> None:
        """
        Save ROI metrics record.

        Args:
            metrics: ROI metrics to save
        """
        with self._lock:
            with self._get_connection() as conn:
                conn.execute("""
                    INSERT INTO roi_metrics
                    (category, credits_consumed, value_generated, roi_score,
                     operations_count, period_start, period_end)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                """, (
                    metrics.category,
                    metrics.credits_consumed,
                    metrics.value_generated,
                    metrics.roi_score,
                    metrics.operations_count,
                    metrics.period_start,
                    metrics.period_end,
                ))

                logger.debug(f"Saved ROI metrics for {metrics.category}")

    def load_roi_metrics(
        self, category: Optional[str] = None, days: int = 30
    ) -> List[ROIMetrics]:
        """
        Load ROI metrics.

        Args:
            category: Specific category to load, or None for all
            days: Number of days to look back

        Returns:
            List of ROI metrics records
        """
        with self._lock:
            cutoff = time.time() - (days * 86400)

            with self._get_connection() as conn:
                if category:
                    cursor = conn.execute("""
                        SELECT category, credits_consumed, value_generated, roi_score,
                               operations_count, period_start, period_end, timestamp
                        FROM roi_metrics
                        WHERE category = ? AND timestamp >= ?
                        ORDER BY timestamp DESC
                    """, (category, cutoff))
                else:
                    cursor = conn.execute("""
                        SELECT category, credits_consumed, value_generated, roi_score,
                               operations_count, period_start, period_end, timestamp
                        FROM roi_metrics
                        WHERE timestamp >= ?
                        ORDER BY timestamp DESC
                    """, (cutoff,))

                records = []
                for row in cursor:
                    record = ROIMetrics(
                        category=row['category'],
                        credits_consumed=row['credits_consumed'],
                        value_generated=row['value_generated'],
                        roi_score=row['roi_score'],
                        operations_count=row['operations_count'],
                        period_start=row['period_start'],
                        period_end=row['period_end'],
                    )
                    records.append(record)

                logger.debug(f"Loaded {len(records)} ROI metrics records")
                return records

    def save_multi_timeframe_discovery_stats(
        self,
        result: 'MultiTimeframeResult',  # Type hint string to avoid circular import
        parallel: bool = False,
        discovery_goal: str = 'balanced'
    ) -> None:
        """
        Save multi-timeframe discovery statistics for cross-session learning.

        Args:
            result: MultiTimeframeResult object from discovery
            parallel: Whether parallel execution was used
            discovery_goal: Discovery goal (quality/quantity/balanced/speed)
        """
        with self._lock:
            # Extract timeframe-specific data
            deep_result = result.timeframe_results.get(DiscoveryTimeframe.DEEP) if hasattr(result, 'timeframe_results') else None
            fast_result = result.timeframe_results.get(DiscoveryTimeframe.FAST) if hasattr(result, 'timeframe_results') else None
            trending_result = result.timeframe_results.get(DiscoveryTimeframe.TRENDING) if hasattr(result, 'timeframe_results') else None

            # Calculate cross-timeframe quality average
            if result.combined_quality_scores:
                cross_timeframe_quality_avg = sum(result.combined_quality_scores.values()) / len(result.combined_quality_scores)
            else:
                cross_timeframe_quality_avg = 0.0

            with self._get_connection() as conn:
                conn.execute("""
                    INSERT INTO multi_timeframe_discovery_stats
                    (discovery_timestamp, total_unique_wallets, total_raw_wallets, deduplication_ratio,
                     multi_timeframe_wallets, total_credits_consumed, total_execution_time_seconds,
                     deep_wallets_discovered, deep_execution_time, deep_credits_consumed,
                     fast_wallets_discovered, fast_execution_time, fast_credits_consumed,
                     trending_wallets_discovered, trending_execution_time, trending_credits_consumed,
                     cross_timeframe_quality_avg, parallel_execution, discovery_goal)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """, (
                    result.timestamp,
                    len(result.combined_wallets),
                    result.deduplication_stats.get('total_raw_wallets', 0),
                    result.deduplication_stats.get('deduplication_ratio', 0.0),
                    result.deduplication_stats.get('multi_timeframe_wallets', 0),
                    result.total_credits_consumed,
                    result.total_execution_time_seconds,
                    len(deep_result.wallets_discovered) if deep_result else 0,
                    deep_result.execution_time_seconds if deep_result else 0.0,
                    deep_result.credits_consumed if deep_result else 0,
                    len(fast_result.wallets_discovered) if fast_result else 0,
                    fast_result.execution_time_seconds if fast_result else 0.0,
                    fast_result.credits_consumed if fast_result else 0,
                    len(trending_result.wallets_discovered) if trending_result else 0,
                    trending_result.execution_time_seconds if trending_result else 0.0,
                    trending_result.credits_consumed if trending_result else 0,
                    cross_timeframe_quality_avg,
                    1 if parallel else 0,  # Convert boolean to integer
                    discovery_goal
                ))

                logger.debug(f"Saved multi-timeframe discovery stats: {len(result.combined_wallets)} unique wallets")

    def load_multi_timeframe_discovery_stats(self, days: int = 30) -> List[Dict[str, Any]]:
        """
        Load multi-timeframe discovery statistics for analysis.

        Args:
            days: Number of days to look back

        Returns:
            List of discovery statistics records
        """
        with self._lock:
            cutoff = time.time() - (days * 86400)

            with self._get_connection() as conn:
                cursor = conn.execute("""
                    SELECT discovery_timestamp, total_unique_wallets, total_raw_wallets,
                           deduplication_ratio, multi_timeframe_wallets, total_credits_consumed,
                           total_execution_time_seconds, deep_wallets_discovered, deep_execution_time,
                           deep_credits_consumed, fast_wallets_discovered, fast_execution_time,
                           fast_credits_consumed, trending_wallets_discovered, trending_execution_time,
                           trending_credits_consumed, cross_timeframe_quality_avg, parallel_execution,
                           discovery_goal, created_at
                    FROM multi_timeframe_discovery_stats
                    WHERE discovery_timestamp >= ?
                    ORDER BY discovery_timestamp DESC
                """, (cutoff,))

                records = []
                for row in cursor:
                    records.append({
                        'discovery_timestamp': row[0],
                        'total_unique_wallets': row[1],
                        'total_raw_wallets': row[2],
                        'deduplication_ratio': row[3],
                        'multi_timeframe_wallets': row[4],
                        'total_credits_consumed': row[5],
                        'total_execution_time_seconds': row[6],
                        'deep_wallets_discovered': row[7],
                        'deep_execution_time': row[8],
                        'deep_credits_consumed': row[9],
                        'fast_wallets_discovered': row[10],
                        'fast_execution_time': row[11],
                        'fast_credits_consumed': row[12],
                        'trending_wallets_discovered': row[13],
                        'trending_execution_time': row[14],
                        'trending_credits_consumed': row[15],
                        'cross_timeframe_quality_avg': row[16],
                        'parallel_execution': bool(row[17]),
                        'discovery_goal': row[18],
                        'created_at': row[19],
                    })

                logger.debug(f"Loaded {len(records)} multi-timeframe discovery stats records")
                return records

    def get_multi_timeframe_summary(self, days: int = 30) -> Dict[str, Any]:
        """
        Get summary statistics for multi-timeframe discovery performance.

        Args:
            days: Number of days to summarize

        Returns:
            Summary statistics
        """
        with self._lock:
            cutoff = time.time() - (days * 86400)

            with self._get_connection() as conn:
                cursor = conn.execute("""
                    SELECT COUNT(*) as total_runs,
                           AVG(total_unique_wallets) as avg_unique_wallets,
                           AVG(deduplication_ratio) as avg_deduplication_ratio,
                           AVG(multi_timeframe_wallets) as avg_multi_timeframe_wallets,
                           AVG(total_credits_consumed) as avg_credits,
                           AVG(total_execution_time_seconds) as avg_execution_time,
                           AVG(cross_timeframe_quality_avg) as avg_quality
                    FROM multi_timeframe_discovery_stats
                    WHERE discovery_timestamp >= ?
                """, (cutoff,))

                row = cursor.fetchone()
                if row and row[0] > 0:
                    return {
                        'total_runs': row[0],
                        'avg_unique_wallets': row[1] or 0,
                        'avg_deduplication_ratio': row[2] or 0.0,
                        'avg_multi_timeframe_wallets': row[3] or 0,
                        'avg_credits_consumed': row[4] or 0,
                        'avg_execution_time_seconds': row[5] or 0.0,
                        'avg_cross_timeframe_quality': row[6] or 0.0,
                    }
                else:
                    return {
                        'total_runs': 0,
                        'avg_unique_wallets': 0,
                        'avg_deduplication_ratio': 0.0,
                        'avg_multi_timeframe_wallets': 0,
                        'avg_credits_consumed': 0,
                        'avg_execution_time_seconds': 0.0,
                        'avg_cross_timeframe_quality': 0.0,
                    }

    def get_credit_summary(self, days: int = 7) -> Dict[str, Any]:
        """
        Get summary of credit usage over recent days.

        Args:
            days: Number of days to summarize

        Returns:
            Summary statistics
        """
        with self._lock:
            cutoff = time.time() - (days * 86400)

            with self._get_connection() as conn:
                cursor = conn.execute("""
                    SELECT
                        SUM(total_credits) as total_credits,
                        SUM(credits_discovery) as discovery,
                        SUM(credits_analysis) as analysis,
                        SUM(credits_validation) as validation,
                        SUM(credits_enrichment) as enrichment,
                        SUM(credits_monitoring) as monitoring,
                        AVG(total_credits) as avg_daily,
                        MAX(total_credits) as max_daily,
                        MIN(total_credits) as min_daily
                    FROM credit_history
                    WHERE timestamp >= ?
                """, (cutoff,))

                row = cursor.fetchone()

                return {
                    'period_days': days,
                    'total_credits': row['total_credits'] or 0,
                    'by_category': {
                        'discovery': row['discovery'] or 0,
                        'analysis': row['analysis'] or 0,
                        'validation': row['validation'] or 0,
                        'enrichment': row['enrichment'] or 0,
                        'monitoring': row['monitoring'] or 0,
                    },
                    'avg_daily': row['avg_daily'] or 0,
                    'max_daily': row['max_daily'] or 0,
                    'min_daily': row['min_daily'] or 0,
                }

    def cleanup_old_history(self) -> int:
        """
        Clean up history older than max_history_days.

        Returns:
            Number of records deleted
        """
        with self._lock:
            cutoff = time.time() - (self._config.max_history_days * 86400)

            with self._get_connection() as conn:
                # Clean credit history
                cursor = conn.execute("""
                    DELETE FROM credit_history WHERE timestamp < ?
                """, (cutoff,))
                credit_deleted = cursor.rowcount

                # Clean old ROI metrics
                cursor = conn.execute("""
                    DELETE FROM roi_metrics WHERE timestamp < ?
                """, (cutoff,))
                roi_deleted = cursor.rowcount

                total_deleted = credit_deleted + roi_deleted

                logger.info(f"Cleaned up {total_deleted} old history records")
                return total_deleted

    def vacuum_database(self) -> None:
        """Vacuum database to reclaim space."""
        with self._lock:
            with self._get_connection() as conn:
                conn.execute("VACUUM")
                logger.info("Database vacuumed")

    def backup_database(self, backup_path: Optional[str] = None) -> str:
        """
        Backup database to specified path.

        Args:
            backup_path: Path for backup file, or default auto-generated

        Returns:
            Path to backup file
        """
        if backup_path is None:
            timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
            backup_path = str(Path(self._get_db_path()).parent / f"scout_persistence_backup_{timestamp}.db")

        with self._lock:
            # Read from source
            source = get_connection(self._get_db_path())
            backup = get_connection(backup_path)

            try:
                source.backup(backup)
                logger.info(f"Database backed up to {backup_path}")
                return backup_path
            finally:
                source.close()
                backup.close()

    def get_database_stats(self) -> Dict[str, Any]:
        """Get database statistics."""
        with self._get_connection() as conn:
            # Table row counts
            cursor = conn.execute("SELECT COUNT(*) FROM credit_history")
            credit_count = cursor.fetchone()[0]

            cursor = conn.execute("SELECT COUNT(*) FROM wallet_performance_history")
            wallet_count = cursor.fetchone()[0]

            cursor = conn.execute("SELECT COUNT(*) FROM roi_metrics")
            roi_count = cursor.fetchone()[0]

            # Database size
            db_path = Path(self._get_db_path())
            db_size = db_path.stat().st_size if db_path.exists() else 0

            return {
                'credit_history_records': credit_count,
                'wallet_performance_records': wallet_count,
                'roi_metrics_records': roi_count,
                'database_size_bytes': db_size,
                'database_size_mb': db_size / (1024 * 1024),
            }
