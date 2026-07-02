#!/usr/bin/env python3
"""
detect-anomalies.py - Real-time anomaly detection for Chimera evaluation

This script monitors Prometheus metrics and detects anomalies based on
thresholds and statistical analysis. It sends alerts to configured channels
when anomalies are detected.

Usage:
    python3 detect-anomalies.py
    python3 detect-anomalies.py --check-once
"""

import argparse
import json
import os
import sys
import time
import requests
from datetime import datetime, timedelta
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
from enum import Enum


class Severity(Enum):
    """Anomaly severity levels."""
    WARNING = "WARNING"
    CRITICAL = "CRITICAL"


@dataclass
class Anomaly:
    """Represents a detected anomaly."""
    metric_name: str
    value: float
    threshold: float
    severity: Severity
    deviation_percent: float
    timestamp: str
    description: str
    affected_component: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert anomaly to dictionary."""
        return {
            'metric_name': self.metric_name,
            'value': self.value,
            'threshold': self.threshold,
            'severity': self.severity.value,
            'deviation_percent': self.deviation_percent,
            'timestamp': self.timestamp,
            'description': self.description,
            'affected_component': self.affected_component
        }


class AnomalyDetector:
    """Real-time anomaly detection for Chimera evaluation."""

    # Anomaly thresholds for key metrics
    THRESHOLDS = {
        # Trade latency thresholds (milliseconds)
        'chimera_trade_latency_avg_ms': {
            'warning': 500,
            'critical': 1000,
            'description': 'Average trade execution latency'
        },
        'chimera_trade_latency_p95_ms': {
            'warning': 1000,
            'critical': 2000,
            'description': 'P95 trade execution latency'
        },
        'chimera_trade_latency_p99_ms': {
            'warning': 2000,
            'critical': 5000,
            'description': 'P99 trade execution latency'
        },

        # RPC latency thresholds
        'chimera_rpc_latency_avg_ms': {
            'warning': 50,
            'critical': 100,
            'description': 'Average RPC call latency'
        },
        'chimera_rpc_latency_p95_ms': {
            'warning': 100,
            'critical': 200,
            'description': 'P95 RPC call latency'
        },

        # Queue depth thresholds
        'chimera_queue_depth': {
            'warning': 800,
            'critical': 1000,
            'description': 'Current queue depth'
        },

        # Resource usage thresholds
        'chimera_memory_usage_percent': {
            'warning': 80,
            'critical': 90,
            'description': 'Memory usage percentage'
        },
        'chimera_cpu_usage_percent': {
            'warning': 85,
            'critical': 95,
            'description': 'CPU usage percentage'
        },
        'chimera_disk_usage_percent': {
            'warning': 85,
            'critical': 95,
            'description': 'Disk usage percentage'
        },

        # Error rate thresholds
        'chimera_error_rate_per_minute': {
            'warning': 5,
            'critical': 10,
            'description': 'Error rate per minute'
        },
        'chimera_rpc_error_rate': {
            'warning': 0.01,  # 1%
            'critical': 0.05,  # 5%
            'description': 'RPC error rate'
        },

        # Circuit breaker state (2=Active, 1=Cooldown, 0=Tripped)
        'chimera_circuit_breaker_state': {
            'warning': 1,
            'critical': 0,
            'description': 'Circuit breaker state (2=Active, 1=Cooldown, 0=Tripped)'
        },

        # Position limits
        'chimera_active_positions': {
            'warning': 8,
            'critical': 10,
            'description': 'Number of active positions'
        },

        # Database performance
        'chimera_db_query_latency_avg_ms': {
            'warning': 100,
            'critical': 500,
            'description': 'Average database query latency'
        },
        'chimera_db_lock_contention': {
            'warning': 10,
            'critical': 20,
            'description': 'Database lock contention count'
        }
    }

    def __init__(self):
        """Initialize the anomaly detector."""
        self.operator_url = os.getenv('OPERATOR_METRICS_URL', 'http://chimera-operator:8080/metrics')
        self.scout_url = os.getenv('SCOUT_METRICS_URL', '').strip()  # Allow empty scout URL
        self.eval_db_path = os.getenv('EVAL_DB_PATH', '/evaluation/evaluation.db')

        # Alert configuration
        self.telegram_token = os.getenv('TELEGRAM_BOT_TOKEN')
        self.telegram_chat_id = os.getenv('TELEGRAM_CHAT_ID')
        self.discord_webhook = os.getenv('DISCORD_WEBHOOK_URL')

        # Monitoring state
        self.previous_metrics = {}
        self.anomaly_history = []

    def fetch_metrics(self, url: str) -> Dict[str, float]:
        """Fetch Prometheus metrics from the specified URL.

        Args:
            url: Prometheus metrics endpoint URL

        Returns:
            Dictionary of metric names to values
        """
        metrics = {}

        try:
            response = requests.get(url, timeout=10)
            response.raise_for_status()

            for line in response.text.split('\n'):
                line = line.strip()
                # Skip comments and empty lines
                if not line or line.startswith('#'):
                    continue

                # Parse metric line
                parts = line.split()
                if len(parts) >= 2:
                    metric_name = parts[0]
                    try:
                        value = float(parts[1])
                        metrics[metric_name] = value
                    except ValueError:
                        continue

        except Exception as e:
            print(f"Error fetching metrics from {url}: {e}")

        return metrics

    def check_thresholds(self, metrics: Dict[str, float]) -> List[Anomaly]:
        """Check metrics against defined thresholds.

        Args:
            metrics: Dictionary of metric names to values

        Returns:
            List of detected anomalies
        """
        anomalies = []
        timestamp = datetime.now().isoformat()

        for metric_name, threshold_config in self.THRESHOLDS.items():
            if metric_name not in metrics:
                continue

            value = metrics[metric_name]
            warning_threshold = threshold_config['warning']
            critical_threshold = threshold_config['critical']
            description = threshold_config['description']

            # Determine severity and threshold
            severity = None
            threshold = None

            # Special handling for circuit breaker state (lower values = worse)
            # 2=Active, 1=Cooldown, 0=Tripped
            if metric_name == 'chimera_circuit_breaker_state':
                if value <= critical_threshold:
                    severity = Severity.CRITICAL
                    threshold = critical_threshold
                elif value <= warning_threshold:
                    severity = Severity.WARNING
                    threshold = warning_threshold
            else:
                # Standard threshold checking (higher values = worse)
                if value >= critical_threshold:
                    severity = Severity.CRITICAL
                    threshold = critical_threshold
                elif value >= warning_threshold:
                    severity = Severity.WARNING
                    threshold = warning_threshold

            if severity:
                # Calculate deviation percentage
                if threshold > 0:
                    deviation_percent = ((value - threshold) / threshold) * 100
                else:
                    deviation_percent = 0.0

                # Determine affected component
                component = self._get_component_for_metric(metric_name)

                anomaly = Anomaly(
                    metric_name=metric_name,
                    value=value,
                    threshold=threshold,
                    severity=severity,
                    deviation_percent=deviation_percent,
                    timestamp=timestamp,
                    description=f"{description}: {value:.2f} exceeds {threshold}",
                    affected_component=component
                )
                anomalies.append(anomaly)

        return anomalies

    def _get_component_for_metric(self, metric_name: str) -> Optional[str]:
        """Determine the affected component based on metric name.

        Args:
            metric_name: Name of the metric

        Returns:
            Component name or None
        """
        if 'trade' in metric_name or 'position' in metric_name:
            return 'trading_engine'
        elif 'rpc' in metric_name:
            return 'rpc_layer'
        elif 'db' in metric_name or 'database' in metric_name:
            return 'database'
        elif 'memory' in metric_name or 'cpu' in metric_name or 'disk' in metric_name:
            return 'system_resources'
        elif 'queue' in metric_name:
            return 'message_queue'
        elif 'circuit_breaker' in metric_name:
            return 'risk_management'
        else:
            return 'unknown'

    def check_trend_anomalies(self, metrics: Dict[str, float]) -> List[Anomaly]:
        """Check for trend-based anomalies (sudden changes).

        Args:
            metrics: Current metrics

        Returns:
            List of trend anomalies
        """
        anomalies = []
        timestamp = datetime.now().isoformat()

        if not self.previous_metrics:
            self.previous_metrics = metrics.copy()
            return anomalies

        # Check for sudden changes in key metrics
        trend_thresholds = {
            'chimera_trade_latency_avg_ms': 0.5,  # 50% increase
            'chimera_rpc_latency_avg_ms': 0.5,
            'chimera_memory_usage_percent': 0.3,  # 30% increase
            'chimera_error_rate_per_minute': 1.0,  # 100% increase (doubling)
        }

        for metric_name, threshold in trend_thresholds.items():
            if metric_name not in metrics or metric_name not in self.previous_metrics:
                continue

            current_value = metrics[metric_name]
            previous_value = self.previous_metrics[metric_name]

            if previous_value == 0:
                continue

            percent_change = ((current_value - previous_value) / previous_value)

            if percent_change > threshold:
                severity = Severity.CRITICAL if percent_change > threshold * 2 else Severity.WARNING

                anomaly = Anomaly(
                    metric_name=metric_name,
                    value=current_value,
                    threshold=previous_value * (1 + threshold),
                    severity=severity,
                    deviation_percent=percent_change * 100,
                    timestamp=timestamp,
                    description=f"Sudden increase in {metric_name}: {previous_value:.2f} → {current_value:.2f} ({percent_change*100:.1f}% increase)",
                    affected_component=self._get_component_for_metric(metric_name)
                )
                anomalies.append(anomaly)

        self.previous_metrics = metrics.copy()
        return anomalies

    def send_telegram_alert(self, anomaly: Anomaly):
        """Send anomaly alert to Telegram.

        Args:
            anomaly: The anomaly to report
        """
        if not self.telegram_token or not self.telegram_chat_id:
            return

        emoji = "🔴" if anomaly.severity == Severity.CRITICAL else "🟡"

        message = f"""
{emoji} Chimera Anomaly Detected

Severity: {anomaly.severity.value}
Metric: {anomaly.metric_name}
Value: {anomaly.value:.2f}
Threshold: {anomaly.threshold:.2f}
Deviation: {anomaly.deviation_percent:.1f}%

{anomaly.description}
Component: {anomaly.affected_component}
Time: {anomaly.timestamp}
"""

        try:
            url = f"https://api.telegram.org/bot{self.telegram_token}/sendMessage"
            data = {
                'chat_id': self.telegram_chat_id,
                'text': message.strip(),
                'parse_mode': 'HTML'
            }
            requests.post(url, json=data, timeout=10)
            print(f"Telegram alert sent for {anomaly.metric_name}")

        except Exception as e:
            print(f"Failed to send Telegram alert: {e}")

    def send_discord_alert(self, anomaly: Anomaly):
        """Send anomaly alert to Discord.

        Args:
            anomaly: The anomaly to report
        """
        if not self.discord_webhook:
            return

        emoji = "🔴" if anomaly.severity == Severity.CRITICAL else "🟡"

        embed = {
            "title": f"{emoji} Chimera Anomaly Detected",
            "color": 0xFF0000 if anomaly.severity == Severity.CRITICAL else 0xFFFF00,
            "fields": [
                {"name": "Severity", "value": anomaly.severity.value, "inline": True},
                {"name": "Metric", "value": anomaly.metric_name, "inline": True},
                {"name": "Value", "value": f"{anomaly.value:.2f}", "inline": True},
                {"name": "Threshold", "value": f"{anomaly.threshold:.2f}", "inline": True},
                {"name": "Deviation", "value": f"{anomaly.deviation_percent:.1f}%", "inline": True},
                {"name": "Component", "value": str(anomaly.affected_component), "inline": True},
            ],
            "description": anomaly.description,
            "timestamp": anomaly.timestamp
        }

        try:
            requests.post(self.discord_webhook, json={'embeds': [embed]}, timeout=10)
            print(f"Discord alert sent for {anomaly.metric_name}")

        except Exception as e:
            print(f"Failed to send Discord alert: {e}")

    def store_anomaly(self, anomaly: Anomaly, day_number: int, hour_number: int):
        """Store anomaly in evaluation database.

        Args:
            anomaly: The anomaly to store
            day_number: Current day number
            hour_number: Current hour number
        """
        try:
            import sqlite3

            conn = sqlite3.connect(self.eval_db_path)
            cursor = conn.cursor()

            # Create table if not exists
            cursor.execute('''
                CREATE TABLE IF NOT EXISTS evaluation_anomalies (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    anomaly_time TEXT NOT NULL,
                    day_number INTEGER NOT NULL,
                    hour_number INTEGER NOT NULL,
                    anomaly_type TEXT NOT NULL,
                    severity TEXT DEFAULT 'WARNING',
                    metric_name TEXT NOT NULL,
                    metric_value REAL,
                    threshold_value REAL,
                    deviation_percent REAL,
                    description TEXT,
                    affected_component TEXT,
                    alert_sent BOOLEAN DEFAULT FALSE,
                    acknowledged BOOLEAN DEFAULT FALSE,
                    resolved BOOLEAN DEFAULT FALSE,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )
            ''')

            # Insert anomaly
            cursor.execute('''
                INSERT INTO evaluation_anomalies (
                    anomaly_time, day_number, hour_number,
                    anomaly_type, severity, metric_name,
                    metric_value, threshold_value, deviation_percent,
                    description, affected_component, alert_sent
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ''', (
                anomaly.timestamp,
                day_number,
                hour_number,
                'threshold_exceeded',
                anomaly.severity.value,
                anomaly.metric_name,
                anomaly.value,
                anomaly.threshold,
                anomaly.deviation_percent,
                anomaly.description,
                anomaly.affected_component,
                True
            ))

            conn.commit()
            conn.close()
            print(f"Stored anomaly in database: {anomaly.metric_name}")

        except Exception as e:
            print(f"Failed to store anomaly in database: {e}")

    def check_anomalies(self) -> List[Anomaly]:
        """Perform comprehensive anomaly check.

        Returns:
            List of detected anomalies
        """
        all_anomalies = []

        # Fetch metrics from all sources
        operator_metrics = self.fetch_metrics(self.operator_url)
        all_metrics = {**operator_metrics}

        # Only fetch scout metrics if URL is configured
        if self.scout_url:
            scout_metrics = self.fetch_metrics(self.scout_url)
            all_metrics.update(scout_metrics)
        else:
            print("Scout metrics URL not configured, skipping Scout metrics")

        if not all_metrics:
            print("Warning: No metrics retrieved")
            return all_anomalies

        print(f"Retrieved {len(all_metrics)} metrics")

        # Check threshold-based anomalies
        threshold_anomalies = self.check_thresholds(all_metrics)
        all_anomalies.extend(threshold_anomalies)

        # Check trend-based anomalies
        trend_anomalies = self.check_trend_anomalies(all_metrics)
        all_anomalies.extend(trend_anomalies)

        return all_anomalies

    def process_anomalies(self, anomalies: List[Anomaly]):
        """Process detected anomalies - send alerts and store in database.

        Args:
            anomalies: List of detected anomalies
        """
        if not anomalies:
            print("No anomalies detected")
            return

        print(f"Processing {len(anomalies)} anomalies")

        # Get current day/hour for storage
        now = datetime.now()
        day_number = getattr(now, 'day', 1)  # Simplified for evaluation
        hour_number = now.hour

        for anomaly in anomalies:
            # Send alerts
            if anomaly.severity == Severity.CRITICAL:
                self.send_telegram_alert(anomaly)
                self.send_discord_alert(anomaly)

            # Store in database
            self.store_anomaly(anomaly, day_number, hour_number)

            # Add to history
            self.anomaly_history.append(anomaly)

    def run_once(self):
        """Run a single anomaly check."""
        print("=" * 50)
        print("Anomaly Detection Check")
        print("=" * 50)
        print(f"Time: {datetime.now().isoformat()}")

        anomalies = self.check_anomalies()

        if anomalies:
            print(f"\n🚨 Detected {len(anomalies)} anomalies:")
            for anomaly in anomalies:
                print(f"  - [{anomaly.severity.value}] {anomaly.metric_name}: {anomaly.value:.2f} (threshold: {anomaly.threshold:.2f})")

            self.process_anomalies(anomalies)
        else:
            print("\n✅ No anomalies detected")

        print("=" * 50)

    def run_continuous(self, interval_seconds: int = 60):
        """Run continuous anomaly detection.

        Args:
            interval_seconds: Check interval in seconds
        """
        print(f"Starting continuous anomaly detection (interval: {interval_seconds}s)")
        print("Press Ctrl+C to stop")

        try:
            while True:
                self.run_once()
                time.sleep(interval_seconds)

        except KeyboardInterrupt:
            print("\nAnomaly detection stopped")


def main():
    """Main entry point for anomaly detection."""
    parser = argparse.ArgumentParser(description='Real-time anomaly detection')
    parser.add_argument('--check-once', action='store_true', help='Run single check and exit')
    parser.add_argument('--interval', type=int, default=60, help='Check interval in seconds')

    args = parser.parse_args()

    detector = AnomalyDetector()

    if args.check_once:
        detector.run_once()
    else:
        detector.run_continuous(args.interval)


if __name__ == '__main__':
    main()