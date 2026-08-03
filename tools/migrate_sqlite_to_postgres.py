#!/usr/bin/env python3
"""
SQLite to PostgreSQL Migration Script for Chimera

This script migrates data from SQLite to PostgreSQL with validation and rollback support.

Usage:
    python migrate_sqlite_to_postgres.py --sqlite-path data/chimera.db --postgres-url "postgresql://user:pass@host:5432/chimera"

Environment Variables:
    CHIMERA_SQLITE_PATH: Path to SQLite database
    CHIMERA_POSTGRES_URL: PostgreSQL connection URL
    CHIMERA_MIGRATION_DRY_RUN: Set to 'true' for dry run
"""

import argparse
import hashlib
import json
import logging
import os
import sqlite3
import sys
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import psycopg2
from psycopg2.extras import RealDictCursor, execute_batch
from psycopg2.extensions import ISOLATION_LEVEL_AUTOCOMMIT

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    handlers=[
        logging.StreamHandler(sys.stdout),
        logging.FileHandler("migration.log"),
    ],
)
logger = logging.getLogger("chimera_migration")


class MigrationConfig:
    """Migration configuration."""

    def __init__(
        self,
        sqlite_path: str,
        postgres_url: str,
        dry_run: bool = False,
        batch_size: int = 1000,
    ):
        self.sqlite_path = sqlite_path
        self.postgres_url = postgres_url
        self.dry_run = dry_run
        self.batch_size = batch_size


class DataValidator:
    """Validate data integrity between SQLite and PostgreSQL."""

    def __init__(self, sqlite_conn: sqlite3.Connection, pg_conn):
        self.sqlite_conn = sqlite_conn
        self.pg_conn = pg_conn
        self.errors: List[str] = []

    def validate_table_counts(self) -> bool:
        """Validate row counts for all tables."""
        logger.info("Validating table row counts...")

        tables = [
            "trades",
            "positions",
            "wallets",
            "dead_letter_queue",
            "config_audit",
            "circuit_breaker_state",
            "kill_switch_state",
            "admin_wallets",
            "schema_migrations",
            "jito_tip_history",
            "reconciliation_log",
            "wallet_monitoring",
            "exit_targets",
            "signal_aggregation",
            "wallet_copy_performance",
            "rate_limit_metrics",
            "wqs_pnl_correlation",
            "historical_liquidity",
            "backups",
        ]

        all_valid = True
        for table in tables:
            try:
                # SQLite count
                sqlite_cursor = self.sqlite_conn.execute(f"SELECT COUNT(*) FROM {table}")
                sqlite_count = sqlite_cursor.fetchone()[0]

                # PostgreSQL count
                pg_cursor = self.pg_conn.cursor()
                pg_cursor.execute(f"SELECT COUNT(*) FROM {table}")
                pg_count = pg_cursor.fetchone()[0]
                pg_cursor.close()

                if sqlite_count == pg_count:
                    logger.info(f"✓ {table}: {sqlite_count} rows")
                else:
                    logger.error(
                        f"✗ {table}: SQLite={sqlite_count}, PostgreSQL={pg_count}"
                    )
                    self.errors.append(
                        f"Row count mismatch for {table}: SQLite={sqlite_count}, PostgreSQL={pg_count}"
                    )
                    all_valid = False
            except Exception as e:
                logger.warning(f"Could not validate table {table}: {e}")

        return all_valid

    @staticmethod
    def _table_checksum(conn, table: str) -> str:
        """Compute a deterministic md5 over the table's rows (Python-side, so
        it works identically for SQLite and PostgreSQL)."""
        hasher = hashlib.md5()
        cursor = conn.cursor()
        cursor.execute(f"SELECT * FROM {table} ORDER BY 1")
        for row in cursor.fetchall():
            hasher.update(repr(tuple(row)).encode("utf-8", "replace"))
        cursor.close()
        return hasher.hexdigest()

    def validate_checksums(self, table: str, key_column: str = None) -> bool:
        """Validate data checksums for a table."""
        logger.info(f"Validating checksums for {table}...")

        try:
            sqlite_checksum = self._table_checksum(self.sqlite_conn, table)
            pg_checksum = self._table_checksum(self.pg_conn, table)

            if sqlite_checksum == pg_checksum:
                logger.info(f"✓ {table} checksums match")
                return True
            else:
                logger.error(
                    f"✗ {table} checksums differ: SQLite={sqlite_checksum}, PostgreSQL={pg_checksum}"
                )
                return False
        except Exception as e:
            logger.warning(f"Could not validate checksums for {table}: {e}")
            return False


class MigrationRunner:
    """Execute migration from SQLite to PostgreSQL."""

    def __init__(self, config: MigrationConfig):
        self.config = config
        self.sqlite_conn: Optional[sqlite3.Connection] = None
        self.pg_conn: Optional[psycopg2.extensions.connection] = None
        self.validator: Optional[DataValidator] = None

        # Statistics
        self.stats = {
            "tables_migrated": 0,
            "rows_migrated": 0,
            "errors": [],
            "start_time": None,
            "end_time": None,
        }

    def connect(self):
        """Establish database connections."""
        logger.info("Connecting to databases...")

        # SQLite
        sqlite_path = Path(self.config.sqlite_path)
        if not sqlite_path.exists():
            raise FileNotFoundError(f"SQLite database not found: {sqlite_path}")

        self.sqlite_conn = sqlite3.connect(str(sqlite_path))
        self.sqlite_conn.row_factory = sqlite3.Row
        logger.info(f"Connected to SQLite: {sqlite_path}")

        # PostgreSQL
        self.pg_conn = psycopg2.connect(self.config.postgres_url)
        # Single transaction: commit only at the end so a mid-migration
        # failure can be rolled back (the module docstring advertises
        # rollback support)
        logger.info("Connected to PostgreSQL")

        self.validator = DataValidator(self.sqlite_conn, self.pg_conn)

    def close(self):
        """Close database connections."""
        if self.sqlite_conn:
            self.sqlite_conn.close()
        if self.pg_conn:
            self.pg_conn.close()

    def backup_postgres(self) -> str:
        """Create PostgreSQL backup before migration."""
        logger.info("Creating PostgreSQL backup...")

        backup_path = f"backup_before_migration_{datetime.now().strftime('%Y%m%d_%H%M%S')}.sql"

        # Use pg_dump for backup (file handle managed by a context manager;
        # a failed backup is fatal since the documented rollback relies on it)
        import subprocess

        try:
            with open(backup_path, "w") as backup_file:
                subprocess.run(
                    ["pg_dump", self.config.postgres_url],
                    stdout=backup_file,
                    check=True,
                )
            logger.info(f"Backup created: {backup_path}")
            return backup_path
        except Exception as e:
            logger.error(f"Could not create backup: {e}")
            raise

    def migrate_table(
        self,
        table_name: str,
        column_mapping: Optional[Dict[str, str]] = None,
        value_transforms: Optional[Dict[str, Any]] = None,
    ) -> int:
        """Migrate a single table from SQLite to PostgreSQL."""
        logger.info(f"Migrating table: {table_name}")

        rows_migrated = 0

        try:
            # Get column names from SQLite
            cursor = self.sqlite_conn.execute(f"SELECT * FROM {table_name} LIMIT 1")
            columns = [desc[0] for desc in cursor.description]

            # Apply column mapping if provided (applied again per row below so
            # the INSERT always uses the mapped names)
            if column_mapping:
                columns = [column_mapping.get(col, col) for col in columns]

            # Get total row count
            count_cursor = self.sqlite_conn.execute(f"SELECT COUNT(*) FROM {table_name}")
            total_rows = count_cursor.fetchone()[0]
            logger.info(f"  Total rows to migrate: {total_rows}")

            # Fetch data in batches
            offset = 0
            while offset < total_rows:
                cursor = self.sqlite_conn.execute(
                    f"SELECT * FROM {table_name} LIMIT {self.config.batch_size} OFFSET {offset}"
                )
                rows = cursor.fetchall()

                if not rows:
                    break

                # Transform and insert into PostgreSQL
                pg_cursor = self.pg_conn.cursor()

                for row in rows:
                    row_dict = dict(row)

                    # Apply value transforms
                    if value_transforms:
                        for col, transform in value_transforms.items():
                            if col in row_dict:
                                row_dict[col] = transform(row_dict[col])

                    # Apply column mapping so the INSERT references mapped names
                    if column_mapping:
                        row_dict = {column_mapping.get(k, k): v for k, v in row_dict.items()}

                    # Build insert query
                    cols = row_dict.keys()
                    values = tuple(row_dict.values())
                    placeholders = ", ".join(["%s"] * len(values))
                    col_names = ", ".join(cols)

                    insert_query = f"INSERT INTO {table_name} ({col_names}) VALUES ({placeholders}) ON CONFLICT DO NOTHING"

                    if not self.config.dry_run:
                        pg_cursor.execute(insert_query, values)

                pg_cursor.close()
                rows_migrated += len(rows)
                offset += self.config.batch_size

                logger.info(f"  Progress: {min(rows_migrated, total_rows)}/{total_rows} rows")

            logger.info(f"  ✓ Migrated {rows_migrated} rows from {table_name}")
            self.stats["tables_migrated"] += 1
            self.stats["rows_migrated"] += rows_migrated

        except Exception as e:
            error_msg = f"Error migrating {table_name}: {e}"
            logger.error(error_msg)
            self.stats["errors"].append(error_msg)
            raise

        return rows_migrated

    def migrate_all(self):
        """Migrate all tables in the correct order."""
        logger.info("Starting migration...")

        if self.config.dry_run:
            logger.info("DRY RUN MODE - No changes will be made")

        self.stats["start_time"] = datetime.now()

        # Migrate tables in dependency order
        migration_plan = [
            # Core tables
            ("wallets", None),
            ("trades", None),
            ("positions", None),
            ("exit_targets", None),

            # System tables
            ("config_audit", None),
            ("circuit_breaker_state", None),
            ("kill_switch_state", None),
            ("admin_wallets", None),
            ("schema_migrations", None),

            # Trading tables
            ("jito_tip_history", None),
            ("signal_aggregation", None),
            ("wallet_copy_performance", None),
            ("wallet_monitoring", None),

            # Support tables
            ("dead_letter_queue", None),
            ("reconciliation_log", None),
            ("rate_limit_metrics", None),
            ("historical_liquidity", None),
            ("backups", None),
            ("wqs_pnl_correlation", None),
        ]

        for table_name, transforms in migration_plan:
            try:
                self.migrate_table(table_name, value_transforms=transforms)
            except Exception as e:
                logger.error(f"Failed to migrate {table_name}: {e}")
                if not self.config.dry_run:
                    raise

        self.stats["end_time"] = datetime.now()

    def validate(self) -> bool:
        """Validate migration results."""
        logger.info("Validating migration...")

        if not self.validator:
            logger.error("Validator not initialized")
            return False

        counts_valid = self.validator.validate_table_counts()

        # Spot-check checksums on the core tables
        checksums_valid = True
        for table in ("trades", "positions", "wallets"):
            if not self.validator.validate_checksums(table):
                checksums_valid = False

        return counts_valid and checksums_valid

    def generate_report(self) -> Dict[str, Any]:
        """Generate migration report."""
        duration = (
            self.stats["end_time"] - self.stats["start_time"]
            if self.stats["end_time"] and self.stats["start_time"]
            else None
        )

        return {
            "status": "success" if not self.stats["errors"] else "partial_failure",
            "dry_run": self.config.dry_run,
            "duration_seconds": duration.total_seconds() if duration else 0,
            "tables_migrated": self.stats["tables_migrated"],
            "rows_migrated": self.stats["rows_migrated"],
            "errors": self.stats["errors"],
            "start_time": self.stats["start_time"].isoformat() if self.stats["start_time"] else None,
            "end_time": self.stats["end_time"].isoformat() if self.stats["end_time"] else None,
        }

    def save_report(self, report: Dict[str, Any]):
        """Save migration report to file."""
        report_path = f"migration_report_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        with open(report_path, "w") as f:
            json.dump(report, f, indent=2)
        logger.info(f"Migration report saved: {report_path}")


def main():
    """Main migration entry point."""
    parser = argparse.ArgumentParser(
        description="Migrate Chimera database from SQLite to PostgreSQL"
    )
    parser.add_argument(
        "--sqlite-path",
        default=os.environ.get("CHIMERA_SQLITE_PATH", "data/chimera.db"),
        help="Path to SQLite database",
    )
    parser.add_argument(
        "--postgres-url",
        default=os.environ.get("CHIMERA_POSTGRES_URL"),
        help="PostgreSQL connection URL",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Perform dry run without making changes",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=1000,
        help="Batch size for data migration",
    )

    args = parser.parse_args()

    if not args.postgres_url:
        logger.error("PostgreSQL URL required (--postgres-url or CHIMERA_POSTGRES_URL)")
        sys.exit(1)

    # Create configuration
    config = MigrationConfig(
        sqlite_path=args.sqlite_path,
        postgres_url=args.postgres_url,
        dry_run=args.dry_run,
        batch_size=args.batch_size,
    )

    # Run migration
    runner = MigrationRunner(config)

    try:
        runner.connect()

        if not config.dry_run:
            runner.backup_postgres()

        runner.migrate_all()
        valid = runner.validate()
        if not valid:
            runner.stats["errors"].append("Validation failed: row counts/checksums differ between SQLite and PostgreSQL")

        # Commit the single transaction (rolled back automatically on error)
        if not config.dry_run and runner.pg_conn:
            runner.pg_conn.commit()

        report = runner.generate_report()
        runner.save_report(report)

        if runner.stats["errors"]:
            logger.error(f"Migration completed with errors: {json.dumps(report, indent=2)}")
            sys.exit(1)

        logger.info("Migration completed successfully!")
        logger.info(f"Report: {json.dumps(report, indent=2)}")

        sys.exit(0)

    except Exception as e:
        logger.error(f"Migration failed: {e}")
        if runner.pg_conn:
            runner.pg_conn.rollback()
        sys.exit(1)
    finally:
        runner.close()


if __name__ == "__main__":
    main()
