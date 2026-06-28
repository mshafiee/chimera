#!/usr/bin/env python3
"""
process-evaluation-metrics.py - Process collected metrics and store in evaluation database

This script processes the raw metrics collected by collect-evaluation-data.sh
and stores them in the evaluation database for systematic analysis.

Usage:
    python3 process-evaluation-metrics.py \
        --day 1 \
        --hour 0 \
        --metrics-dir /opt/chimera/evaluation/day-1 \
        --database /opt/chimera/evaluation/evaluation.db \
        --timestamp "2026-06-28T00:00:00Z"
"""

import argparse
import json
import os
import re
import sqlite3
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional


class EvaluationMetricsProcessor:
    """Process and store evaluation metrics in the evaluation database."""

    def __init__(self, db_path: str):
        """Initialize the metrics processor.

        Args:
            db_path: Path to the evaluation database
        """
        self.db_path = Path(db_path)
        self.conn = None
        self.cursor = None

    def connect(self):
        """Connect to the evaluation database."""
        try:
            self.conn = sqlite3.connect(str(self.db_path))
            self.cursor = self.conn.cursor()
            print(f"Connected to evaluation database: {self.db_path}")
        except sqlite3.Error as e:
            print(f"Failed to connect to database: {e}")
            sys.exit(1)

    def close(self):
        """Close the database connection."""
        if self.conn:
            self.conn.close()

    def parse_prometheus_metrics(self, metrics_file: Path) -> Dict[str, Any]:
        """Parse Prometheus metrics text format into structured data.

        Args:
            metrics_file: Path to the Prometheus metrics file

        Returns:
            Dictionary with metric names and values
        """
        metrics = {}

        try:
            with open(metrics_file, 'r') as f:
                for line in f:
                    line = line.strip()
                    # Skip comments and empty lines
                    if not line or line.startswith('#'):
                        continue

                    # Parse metric line
                    # Format: metric_name{labels} value
                    match = re.match(r'^(\w+)(\{.*?\})?\s+(.+)$', line)
                    if match:
                        metric_name = match.group(1)
                        value = match.group(3)

                        # Store the numeric value
                        try:
                            metrics[metric_name] = float(value)
                        except ValueError:
                            continue

        except Exception as e:
            print(f"Error parsing metrics file {metrics_file}: {e}")

        return metrics

    def extract_container_stats(self, stats_file: Path) -> Dict[str, Any]:
        """Extract container statistics from docker stats output.

        Args:
            stats_file: Path to the docker stats file

        Returns:
            Dictionary with container statistics
        """
        containers = {}

        try:
            with open(stats_file, 'r') as f:
                lines = f.readlines()
                # Skip header line
                for line in lines[1:]:
                    parts = line.split()
                    if len(parts) >= 5:
                        container_name = parts[0]
                        cpu_percent = float(parts[1].rstrip('%'))
                        memory_usage = parts[2]
                        memory_percent = float(parts[3].rstrip('%'))

                        containers[container_name] = {
                            'cpu_percent': cpu_percent,
                            'memory_usage': memory_usage,
                            'memory_percent': memory_percent
                        }
        except Exception as e:
            print(f"Error parsing docker stats {stats_file}: {e}")

        return containers

    def store_evaluation_snapshot(
        self,
        day_number: int,
        hour_number: int,
        timestamp: str,
        operator_metrics: Dict[str, Any],
        scout_metrics: Optional[Dict[str, Any]] = None,
        system_stats: Optional[Dict[str, Any]] = None,
        health_status: Optional[Dict[str, Any]] = None
    ) -> int:
        """Store evaluation snapshot in the database.

        Args:
            day_number: Day number of the evaluation
            hour_number: Hour number of the day (0-23)
            timestamp: ISO timestamp of the snapshot
            operator_metrics: Prometheus metrics from operator
            scout_metrics: Optional Prometheus metrics from scout
            system_stats: Optional system statistics
            health_status: Optional health status data

        Returns:
            ID of the inserted snapshot
        """
        try:
            # Extract key metrics from operator
            cpu_usage = operator_metrics.get('chimera_cpu_usage_percent', 0.0)
            memory_usage = operator_metrics.get('chimera_memory_usage_percent', 0.0)
            active_positions = int(operator_metrics.get('chimera_active_positions', 0))
            queue_depth = int(operator_metrics.get('chimera_queue_depth', 0))
            total_trades = int(operator_metrics.get('chimera_trades_total', 0))
            avg_latency = operator_metrics.get('chimera_trade_latency_avg_ms', 0.0)
            p95_latency = operator_metrics.get('chimera_trade_latency_p95_ms', 0.0)
            p99_latency = operator_metrics.get('chimera_trade_latency_p99_ms', 0.0)
            rpc_latency = operator_metrics.get('chimera_rpc_latency_avg_ms', 0.0)
            total_pnl = operator_metrics.get('chimera_total_pnl_sol', 0.0)
            circuit_breaker = int(operator_metrics.get('chimera_circuit_breaker_state', 0))

            # Calculate additional metrics
            successful_trades = int(operator_metrics.get('chimera_successful_trades_total', 0))
            failed_trades = total_trades - successful_trades

            snapshot_data = {
                'operator_metrics': operator_metrics,
                'scout_metrics': scout_metrics or {},
                'system_stats': system_stats or {},
                'health_status': health_status or {}
            }

            snapshot_time = datetime.fromisoformat(timestamp.replace('Z', '+00:00'))

            self.cursor.execute('''
                INSERT INTO evaluation_snapshots (
                    snapshot_time, day_number, hour_number,
                    cpu_usage_percent, memory_usage_percent,
                    active_positions_count, queue_depth,
                    total_trades_today, successful_trades_today, failed_trades_today,
                    avg_trade_latency_ms, p95_trade_latency_ms, p99_trade_latency_ms,
                    rpc_latency_avg_ms, total_pnl_sol,
                    circuit_breaker_state, snapshot_data
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ''', (
                snapshot_time, day_number, hour_number,
                cpu_usage, memory_usage,
                active_positions, queue_depth,
                total_trades, successful_trades, failed_trades,
                avg_latency, p95_latency, p99_latency,
                rpc_latency, total_pnl,
                circuit_breaker, json.dumps(snapshot_data)
            ))

            self.conn.commit()
            snapshot_id = self.cursor.lastrowid
            print(f"Stored evaluation snapshot {snapshot_id}")
            return snapshot_id

        except sqlite3.Error as e:
            print(f"Error storing evaluation snapshot: {e}")
            self.conn.rollback()
            return 0

    def store_system_resources(
        self,
        snapshot_time: str,
        container_stats: Dict[str, Any]
    ) -> int:
        """Store system resource metrics for each container.

        Args:
            snapshot_time: Timestamp of the snapshot
            container_stats: Dictionary of container statistics

        Returns:
            Number of resources stored
        """
        stored_count = 0

        try:
            snapshot_dt = datetime.fromisoformat(snapshot_time.replace('Z', '+00:00'))

            for container_name, stats in container_stats.items():
                self.cursor.execute('''
                    INSERT INTO system_resources (
                        snapshot_time, container_name,
                        cpu_percent, memory_percent, resource_data
                    ) VALUES (?, ?, ?, ?, ?)
                ''', (
                    snapshot_dt,
                    container_name,
                    stats.get('cpu_percent', 0.0),
                    stats.get('memory_percent', 0.0),
                    json.dumps(stats)
                ))

                stored_count += 1

            self.conn.commit()
            print(f"Stored {stored_count} system resource entries")

        except sqlite3.Error as e:
            print(f"Error storing system resources: {e}")
            self.conn.rollback()

        return stored_count

    def detect_and_store_anomalies(
        self,
        snapshot_id: int,
        day_number: int,
        hour_number: int,
        operator_metrics: Dict[str, Any]
    ) -> int:
        """Detect anomalies based on threshold checking and store them.

        Args:
            snapshot_id: ID of the related snapshot
            day_number: Day number
            hour_number: Hour number
            operator_metrics: Operator metrics to check

        Returns:
            Number of anomalies detected
        """
        anomalies = []

        # Define anomaly thresholds
        thresholds = {
            'chimera_trade_latency_p99_ms': ('WARNING', 2000, 'CRITICAL', 5000),
            'chimera_rpc_latency_avg_ms': ('WARNING', 100, 'CRITICAL', 500),
            'chimera_queue_depth': ('WARNING', 800, 'CRITICAL', 1000),
            'chimera_memory_usage_percent': ('WARNING', 85, 'CRITICAL', 95),
            'chimera_cpu_usage_percent': ('WARNING', 90, 'CRITICAL', 98),
            'chimera_circuit_breaker_state': ('CRITICAL', 1, 'CRITICAL', 1)
        }

        for metric_name, (warn_sev, warn_thresh, crit_sev, crit_thresh) in thresholds.items():
            if metric_name in operator_metrics:
                value = operator_metrics[metric_name]
                severity = None
                threshold = None

                # Check critical threshold first
                if value >= crit_thresh:
                    severity = crit_sev
                    threshold = crit_thresh
                # Then warning threshold
                elif value >= warn_thresh:
                    severity = warn_sev
                    threshold = warn_thresh

                if severity:
                    deviation_percent = ((value - threshold) / threshold) * 100 if threshold > 0 else 0

                    anomaly = {
                        'metric_name': metric_name,
                        'value': value,
                        'threshold': threshold,
                        'severity': severity,
                        'deviation_percent': deviation_percent
                    }
                    anomalies.append(anomaly)

        # Store detected anomalies
        stored_count = 0
        for anomaly in anomalies:
            try:
                self.cursor.execute('''
                    INSERT INTO evaluation_anomalies (
                        anomaly_time, day_number, hour_number,
                        anomaly_type, severity, metric_name, metric_value,
                        threshold_value, deviation_percent, related_snapshot_id
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ''', (
                    datetime.now().isoformat(),
                    day_number, hour_number,
                    'threshold_exceeded', anomaly['severity'],
                    anomaly['metric_name'], anomaly['value'],
                    anomaly['threshold'], anomaly['deviation_percent'],
                    snapshot_id
                ))

                stored_count += 1

            except sqlite3.Error as e:
                print(f"Error storing anomaly: {e}")

        if stored_count > 0:
            self.conn.commit()
            print(f"Detected and stored {stored_count} anomalies")

        return stored_count

    def process_metrics_directory(
        self,
        day_number: int,
        hour_number: int,
        metrics_dir: Path,
        timestamp: str
    ) -> bool:
        """Process all metrics in the specified directory.

        Args:
            day_number: Day number
            hour_number: Hour number
            metrics_dir: Path to the metrics directory
            timestamp: Collection timestamp

        Returns:
            True if processing succeeded
        """
        success = True

        try:
            # Find operator metrics file
            operator_metrics_file = None
            scout_metrics_file = None
            docker_stats_file = None
            health_status_file = None

            for file in metrics_dir.glob("*.txt"):
                if 'operator-metrics' in file.name:
                    operator_metrics_file = file
                elif 'scout-metrics' in file.name:
                    scout_metrics_file = file
                elif 'docker-stats' in file.name:
                    docker_stats_file = file

            # Look for health status JSON
            for file in metrics_dir.glob("*.json"):
                if 'health-status' in file.name:
                    health_status_file = file

            # Parse operator metrics
            operator_metrics = {}
            if operator_metrics_file:
                operator_metrics = self.parse_prometheus_metrics(operator_metrics_file)
                print(f"Parsed {len(operator_metrics)} operator metrics")
            else:
                print("Warning: Operator metrics file not found")

            # Parse scout metrics
            scout_metrics = {}
            if scout_metrics_file:
                scout_metrics = self.parse_prometheus_metrics(scout_metrics_file)
                print(f"Parsed {len(scout_metrics)} scout metrics")

            # Parse docker stats
            system_stats = {}
            if docker_stats_file:
                system_stats = self.extract_container_stats(docker_stats_file)
                print(f"Parsed stats for {len(system_stats)} containers")

            # Parse health status
            health_status = {}
            if health_status_file:
                with open(health_status_file, 'r') as f:
                    health_status = json.load(f)
                print("Parsed health status")

            # Store evaluation snapshot
            snapshot_id = self.store_evaluation_snapshot(
                day_number=day_number,
                hour_number=hour_number,
                timestamp=timestamp,
                operator_metrics=operator_metrics,
                scout_metrics=scout_metrics,
                system_stats=system_stats,
                health_status=health_status
            )

            if snapshot_id > 0:
                # Store system resources
                if system_stats:
                    self.store_system_resources(timestamp, system_stats)

                # Detect and store anomalies
                if operator_metrics:
                    self.detect_and_store_anomalies(
                        snapshot_id=snapshot_id,
                        day_number=day_number,
                        hour_number=hour_number,
                        operator_metrics=operator_metrics
                    )
            else:
                print("Warning: Failed to store evaluation snapshot")
                success = False

        except Exception as e:
            print(f"Error processing metrics directory: {e}")
            success = False

        return success


def main():
    """Main entry point for the metrics processor."""
    parser = argparse.ArgumentParser(
        description='Process evaluation metrics and store in database'
    )
    parser.add_argument('--day', type=int, required=True, help='Day number')
    parser.add_argument('--hour', type=int, required=True, help='Hour number')
    parser.add_argument('--metrics-dir', type=str, required=True, help='Metrics directory path')
    parser.add_argument('--database', type=str, required=True, help='Evaluation database path')
    parser.add_argument('--timestamp', type=str, required=True, help='Collection timestamp')

    args = parser.parse_args()

    # Validate inputs
    metrics_dir = Path(args.metrics_dir)
    if not metrics_dir.exists():
        print(f"Error: Metrics directory does not exist: {metrics_dir}")
        sys.exit(1)

    # Create database if it doesn't exist
    db_path = Path(args.database)
    db_path.parent.mkdir(parents=True, exist_ok=True)

    # Initialize processor
    processor = EvaluationMetricsProcessor(str(db_path))
    processor.connect()

    try:
        # Process metrics
        success = processor.process_metrics_directory(
            day_number=args.day,
            hour_number=args.hour,
            metrics_dir=metrics_dir,
            timestamp=args.timestamp
        )

        if success:
            print("Metrics processing completed successfully")
            sys.exit(0)
        else:
            print("Metrics processing completed with errors")
            sys.exit(1)

    finally:
        processor.close()


if __name__ == '__main__':
    main()