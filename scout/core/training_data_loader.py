"""
Training Data Loader for Scout ML Models

Loads and prepares training data from multiple sources including:
- SQLite database (wallets, trades, positions tables)
- Feature store CSV files
- Correlation reader for actual PnL data

Usage:
    loader = TrainingDataLoader(db_path="data/chimera.db")
    X_train, y_train, X_val, y_val, feature_names = loader.create_training_dataset()
"""

import logging
import sqlite3
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any

import numpy as np

from .db import get_connection

logger = logging.getLogger(__name__)


class TrainingDataLoader:
    """
    Loads and prepares training data from multiple sources.

    This class provides methods to:
    - Load wallet features from the database
    - Load actual PnL values for training targets
    - Load time-series features from FeatureStore
    - Create complete training datasets with features and targets
    """

    def __init__(
        self,
        db_path: Optional[str] = None,
        feature_store_path: Optional[str] = None
    ):
        """
        Initialize the training data loader.

        Args:
            db_path: Path to SQLite database (default: data/chimera.db)
            feature_store_path: Path to feature store CSV (default: data/features/wallet_features.csv)
        """
        if db_path is None:
            db_path = "data/chimera.db"
        if feature_store_path is None:
            feature_store_path = "data/features/wallet_features.csv"

        self.db_path = Path(db_path)
        self.feature_store_path = Path(feature_store_path)

        # Validate database exists
        if not self.db_path.exists():
            logger.warning(f"Database not found at {self.db_path}")

    def load_wallet_features(
        self,
        min_trades: int = 5,
        status: str = "ACTIVE",
        time_window_days: int = 30
    ) -> List[Dict[str, Any]]:
        """
        Load wallet features from the database.

        Args:
            min_trades: Minimum number of trades required
            status: Wallet status filter (ACTIVE, CANDIDATE, REJECTED, or None for all)
            time_window_days: Time window for feature calculation

        Returns:
            List of wallet feature dictionaries
        """
        if not self.db_path.exists():
            logger.error(f"Database not found at {self.db_path}")
            return []

        try:
            conn = get_connection(self.db_path)
            conn.row_factory = sqlite3.Row
            cursor = conn.cursor()

            # Build query
            query = """
                SELECT
                    address,
                    status,
                    wqs_score,
                    roi_7d,
                    roi_30d,
                    trade_count_30d,
                    win_rate,
                    max_drawdown_30d,
                    avg_trade_size_sol,
                    profit_factor,
                    sortino_ratio,
                    avg_entry_delay_seconds,
                    archetype,
                    realized_pnl_30d_sol,
                    last_trade_at,
                    created_at
                FROM wallets
                WHERE trade_count_30d >= ?
            """
            params = [min_trades]

            if status:
                query += " AND status = ?"
                params.append(status)

            query += " ORDER BY created_at DESC"

            cursor.execute(query, params)
            rows = cursor.fetchall()

            # Convert to list of dicts
            wallets = []
            for row in rows:
                wallet = dict(row)
                # Convert to float where needed
                for key, value in wallet.items():
                    if value is not None and isinstance(value, (int, float)):
                        wallet[key] = float(value)
                wallets.append(wallet)

            conn.close()

            logger.info(f"Loaded {len(wallets)} wallet features from database")
            return wallets

        except Exception as e:
            logger.error(f"Failed to load wallet features: {e}")
            return []

    def load_trade_targets(
        self,
        wallet_addresses: List[str],
        time_window_days: int = 30
    ) -> Dict[str, float]:
        """
        Load actual PnL values for training targets.

        Args:
            wallet_addresses: List of wallet addresses
            time_window_days: Time window for PnL calculation

        Returns:
            Dictionary mapping wallet_address -> actual_pnl
        """
        if not self.db_path.exists():
            logger.error(f"Database not found at {self.db_path}")
            return {}

        try:
            conn = get_connection(self.db_path)
            cursor = conn.cursor()

            # Calculate time threshold
            threshold = datetime.utcnow() - timedelta(days=time_window_days)

            # Query trades for each wallet
            targets = {}
            for address in wallet_addresses:
                cursor.execute("""
                    SELECT
                        SUM(net_pnl_sol) as total_pnl,
                        COUNT(*) as trade_count
                    FROM trades
                    WHERE wallet_address = ?
                        AND created_at >= ?
                        AND status IN ('CLOSED', 'EXITED')
                """, (address, threshold.isoformat()))

                row = cursor.fetchone()
                if row and row[1] > 0:  # Has closed trades
                    targets[address] = float(row[0]) if row[0] else 0.0

            conn.close()

            logger.info(f"Loaded PnL targets for {len(targets)} wallets")
            return targets

        except Exception as e:
            logger.error(f"Failed to load trade targets: {e}")
            return {}

    def load_feature_store_history(self) -> Optional[np.ndarray]:
        """
        Load time-series features from FeatureStore CSV.

        Returns:
            NumPy array of historical features or None if file doesn't exist
        """
        if not self.feature_store_path.exists():
            logger.warning(f"Feature store not found at {self.feature_store_path}")
            return None

        try:
            import pandas as pd

            df = pd.read_csv(self.feature_store_path)
            logger.info(f"Loaded {len(df)} feature vectors from feature store")
            return df.to_dict('records')

        except ImportError:
            logger.warning("pandas not available for feature store loading")
            return None
        except Exception as e:
            logger.error(f"Failed to load feature store: {e}")
            return None

    def create_training_dataset(
        self,
        target_column: str = "roi_30d",
        min_trades: int = 5,
        time_window_days: int = 30,
        val_split: float = 0.2,
        random_state: int = 42
    ) -> Tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray, List[str]]:
        """
        Create complete training dataset with features and targets.

        Args:
            target_column: Column name for target variable (roi_30d, realized_pnl_30d_sol, etc.)
            min_trades: Minimum number of trades required
            time_window_days: Time window for data
            val_split: Validation split ratio
            random_state: Random seed for reproducibility

        Returns:
            Tuple of (X_train, y_train, X_val, y_val, feature_names)
        """
        # Load wallet features
        wallets = self.load_wallet_features(
            min_trades=min_trades,
            time_window_days=time_window_days
        )

        if not wallets:
            raise ValueError("No wallet features loaded. Check database exists and has data.")

        # Extract features and targets
        feature_names, X, y = self._extract_features_and_targets(
            wallets,
            target_column=target_column
        )

        if len(X) == 0:
            raise ValueError("No valid samples extracted. Check target column exists and has data.")

        # Convert to numpy arrays
        X = np.array(X)
        y = np.array(y)

        # Split into train and validation sets (time-based split)
        # Use last 20% for validation (more realistic for time-series)
        split_idx = int(len(X) * (1 - val_split))

        X_train, X_val = X[:split_idx], X[split_idx:]
        y_train, y_val = y[:split_idx], y[split_idx:]

        logger.info(
            f"Created training dataset: {len(X_train)} train samples, "
            f"{len(X_val)} validation samples, {len(feature_names)} features"
        )

        return X_train, y_train, X_val, y_val, feature_names

    def create_test_dataset(
        self,
        target_column: str = "roi_30d",
        min_trades: int = 5
    ) -> Tuple[np.ndarray, np.ndarray, List[str]]:
        """
        Create test dataset (alias for create_training_dataset without split).

        Args:
            target_column: Column name for target variable
            min_trades: Minimum number of trades required

        Returns:
            Tuple of (X_test, y_test, feature_names)
        """
        X_train, y_train, X_val, y_val, feature_names = self.create_training_dataset(
            target_column=target_column,
            min_trades=min_trades,
            val_split=0.0
        )

        # Combine train and val for full test set
        X_test = np.vstack([X_train, X_val]) if len(X_val) > 0 else X_train
        y_test = np.concatenate([y_train, y_val]) if len(y_val) > 0 else y_train

        return X_test, y_test, feature_names

    def _extract_features_and_targets(
        self,
        wallets: List[Dict[str, Any]],
        target_column: str = "roi_30d"
    ) -> Tuple[List[str], List[List[float]], List[float]]:
        """
        Extract features and targets from wallet data.

        Args:
            wallets: List of wallet dictionaries
            target_column: Name of target column

        Returns:
            Tuple of (feature_names, X, y)
        """
        # Define feature columns (exclude target and non-feature columns)
        exclude_columns = {
            'address', 'status', 'archetype', 'last_trade_at', 'created_at',
            target_column
        }

        # Get feature names from first wallet
        if wallets:
            feature_names = [
                k for k in wallets[0].keys()
                if k not in exclude_columns and wallets[0][k] is not None
            ]
        else:
            feature_names = []

        # Extract features and targets
        X = []
        y = []

        for wallet in wallets:
            # Extract target
            target = wallet.get(target_column)
            if target is None or np.isnan(target):
                continue

            # Extract features
            features = []
            for name in feature_names:
                value = wallet.get(name, 0.0)
                if value is None or np.isnan(value):
                    value = 0.0
                features.append(float(value))

            X.append(features)
            y.append(float(target))

        return feature_names, X, y

    def get_feature_statistics(self) -> Dict[str, Any]:
        """
        Get statistics about available features.

        Returns:
            Dictionary with feature statistics
        """
        wallets = self.load_wallet_features(min_trades=1)

        if not wallets:
            return {}

        # Calculate statistics
        stats = {
            'total_wallets': len(wallets),
            'feature_count': len(wallets[0]) if wallets else 0,
            'sample_wallet': wallets[0] if wallets else None,
        }

        # Count by status
        status_counts = {}
        for wallet in wallets:
            status = wallet.get('status', 'UNKNOWN')
            status_counts[status] = status_counts.get(status, 0) + 1

        stats['status_distribution'] = status_counts

        return stats


# Convenience functions
def load_training_data(
    db_path: str = "data/chimera.db",
    target_column: str = "roi_30d",
    min_trades: int = 5
) -> Tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray, List[str]]:
    """
    Convenience function to load training data.

    Args:
        db_path: Path to database
        target_column: Target column name
        min_trades: Minimum trades required

    Returns:
        Tuple of (X_train, y_train, X_val, y_val, feature_names)
    """
    loader = TrainingDataLoader(db_path)
    return loader.create_training_dataset(
        target_column=target_column,
        min_trades=min_trades
    )


def get_available_wallets(
    db_path: str = "data/chimera.db",
    min_trades: int = 5
) -> List[str]:
    """
    Get list of wallet addresses available for training.

    Args:
        db_path: Path to database
        min_trades: Minimum trades required

    Returns:
        List of wallet addresses
    """
    loader = TrainingDataLoader(db_path)
    wallets = loader.load_wallet_features(min_trades=min_trades)
    return [w['address'] for w in wallets if 'address' in w]
