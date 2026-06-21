"""
Validation Reporter for Scout ML Models

Generates comprehensive validation reports and alerts.
This module provides the reporting layer for model validation.

Usage:
    reporter = ValidationReporter(db_path="data/chimera.db")
    report = reporter.generate_report(model_types=['xgboost'], time_window='7d')
    if report.get('high_error_rate'):
        reporter.send_alert('high_error', report)
"""

import json
import logging
import os
from datetime import datetime
from pathlib import Path
from typing import Dict, Any, Optional, List
from dataclasses import dataclass

from scout.core.validation_metrics import ValidationMetricsCalculator, ValidationMetrics
from scout.core.prediction_matcher import PredictionMatcher

logger = logging.getLogger(__name__)


@dataclass
class AlertConfig:
    """Configuration for alerting."""
    webhook_url: Optional[str] = None
    high_error_threshold: float = 0.5  # SOL
    drift_threshold: float = 0.15  # 15% degradation
    low_accuracy_threshold: float = 0.5  # 50% direction accuracy
    alert_dir: str = "data/alerts"


class ValidationReporter:
    """
    Generates comprehensive validation reports and alerts.

    This class:
    - Aggregates metrics from all models
    - Compares model performance
    - Detects drift and anomalies
    - Generates human-readable reports
    - Sends alerts when thresholds are exceeded
    """

    def __init__(
        self,
        db_path: Optional[str] = None,
        alert_config: Optional[AlertConfig] = None
    ):
        """
        Initialize the validation reporter.

        Args:
            db_path: Path to SQLite database
            alert_config: Optional alert configuration
        """
        if db_path is None:
            db_path = "data/chimera.db"

        self.db_path = Path(db_path)
        self.metrics_calculator = ValidationMetricsCalculator(db_path)
        self.prediction_matcher = PredictionMatcher(db_path)
        self.alert_config = alert_config or AlertConfig()

        # Load config from environment
        self._load_env_config()

    def _load_env_config(self):
        """Load alert configuration from environment variables."""
        self.alert_config.webhook_url = os.getenv('SCOUT_ALERT_WEBHOOK_URL', self.alert_config.webhook_url)
        self.alert_config.high_error_threshold = float(os.getenv('SCOUT_ALERT_HIGH_ERROR_THRESHOLD', self.alert_config.high_error_threshold))
        self.alert_config.drift_threshold = float(os.getenv('SCOUT_ALERT_DRIFT_THRESHOLD', self.alert_config.drift_threshold))
        self.alert_config.low_accuracy_threshold = float(os.getenv('SCOUT_ALERT_LOW_ACCURACY_THRESHOLD', self.alert_config.low_accuracy_threshold))
        self.alert_config.alert_dir = os.getenv('SCOUT_ALERT_DIR', self.alert_config.alert_dir)

    def generate_report(
        self,
        model_types: Optional[List[str]] = None,
        time_window: str = '7d',
        output_format: str = 'dict',
        include_recommendations: bool = True
    ) -> Any:
        """
        Generate comprehensive validation report.

        Args:
            model_types: Optional list of model types to include
            time_window: Time window for analysis ('7d', '30d', 'all')
            output_format: Output format ('dict', 'json')
            include_recommendations: Whether to include recommendations

        Returns:
            Report in requested format
        """
        logger.info(f"Generating validation report (time_window: {time_window})")

        # Get all model types if not specified
        if model_types is None:
            try:
                import sqlite3
                conn = sqlite3.connect(str(self.db_path))
                cursor = conn.cursor()
                cursor.execute("SELECT DISTINCT model_type FROM ml_predictions WHERE status = 'MATCHED'")
                model_types = [row[0] for row in cursor.fetchall()]
                conn.close()
            except Exception as e:
                logger.error(f"Failed to get model types: {e}")
                model_types = []

        # Calculate metrics for each model
        model_metrics = {}
        for model_type in model_types:
            metrics = self.metrics_calculator.calculate_metrics(
                model_type=model_type,
                time_window=time_window,
                min_predictions=1
            )
            if metrics:
                model_metrics[model_type] = metrics

        # Build report
        report = {
            'generated_at': datetime.utcnow().isoformat(),
            'time_window': time_window,
            'model_types_analyzed': list(model_types),
            'models_with_data': list(model_metrics.keys()),
            'summary': self._generate_summary(model_metrics),
            'model_metrics': {
                name: metrics.to_dict()
                for name, metrics in model_metrics.items()
            },
            'comparison': self._generate_comparison(model_metrics),
            'issues': self._detect_issues(model_metrics),
        }

        # Add recent errors
        report['recent_errors'] = self._get_recent_errors(model_types, limit=10)

        # Add recommendations
        if include_recommendations:
            report['recommendations'] = self._generate_recommendations(report)

        # Format output
        if output_format == 'json':
            return json.dumps(report, indent=2)
        return report

    def _generate_summary(self, model_metrics: Dict[str, ValidationMetrics]) -> Dict[str, Any]:
        """Generate summary statistics across all models."""
        if not model_metrics:
            return {
                'total_models': 0,
                'total_predictions': 0,
                'total_matched': 0,
                'avg_rmse': 0.0,
                'avg_correlation': 0.0,
                'avg_direction_accuracy': 0.0,
            }

        total_predictions = sum(m.total_predictions for m in model_metrics.values())
        total_matched = sum(m.matched_predictions for m in model_metrics.values())

        # Weighted average by matched predictions
        total_matched_weight = sum(m.matched_predictions for m in model_metrics.values())
        avg_rmse = sum(m.rmse * m.matched_predictions for m in model_metrics.values()) / total_matched_weight if total_matched_weight > 0 else 0.0
        avg_correlation = sum(m.correlation * m.matched_predictions for m in model_metrics.values()) / total_matched_weight if total_matched_weight > 0 else 0.0
        avg_direction_accuracy = sum(m.direction_accuracy * m.matched_predictions for m in model_metrics.values()) / total_matched_weight if total_matched_weight > 0 else 0.0

        return {
            'total_models': len(model_metrics),
            'total_predictions': total_predictions,
            'total_matched': total_matched,
            'total_pending': sum(m.pending_predictions for m in model_metrics.values()),
            'total_expired': sum(m.expired_predictions for m in model_metrics.values()),
            'avg_rmse': float(avg_rmse),
            'avg_correlation': float(avg_correlation),
            'avg_direction_accuracy': float(avg_direction_accuracy),
            'best_model_by_rmse': min(model_metrics.items(), key=lambda x: x[1].rmse)[0] if model_metrics else None,
            'best_model_by_correlation': max(model_metrics.items(), key=lambda x: x[1].correlation)[0] if model_metrics else None,
        }

    def _generate_comparison(self, model_metrics: Dict[str, ValidationMetrics]) -> Dict[str, Any]:
        """Generate model comparison metrics."""
        if len(model_metrics) < 2:
            return {'note': 'Need at least 2 models for comparison'}

        comparison = {
            'rmse_ranking': [],
            'correlation_ranking': [],
            'direction_accuracy_ranking': [],
        }

        for model_type, metrics in model_metrics.items():
            comparison['rmse_ranking'].append({
                'model': model_type,
                'rmse': metrics.rmse,
            })
            comparison['correlation_ranking'].append({
                'model': model_type,
                'correlation': metrics.correlation,
            })
            comparison['direction_accuracy_ranking'].append({
                'model': model_type,
                'direction_accuracy': metrics.direction_accuracy,
            })

        # Sort rankings
        comparison['rmse_ranking'].sort(key=lambda x: x['rmse'])
        comparison['correlation_ranking'].sort(key=lambda x: x['correlation'], reverse=True)
        comparison['direction_accuracy_ranking'].sort(key=lambda x: x['direction_accuracy'], reverse=True)

        return comparison

    def _detect_issues(self, model_metrics: Dict[str, ValidationMetrics]) -> List[Dict[str, Any]]:
        """Detect issues in model performance."""
        issues = []

        for model_type, metrics in model_metrics.items():
            # High error rate
            if metrics.rmse > self.alert_config.high_error_threshold:
                issues.append({
                    'severity': 'high',
                    'type': 'high_error_rate',
                    'model': model_type,
                    'message': f"RMSE ({metrics.rmse:.4f}) exceeds threshold ({self.alert_config.high_error_threshold:.4f})",
                    'value': metrics.rmse,
                    'threshold': self.alert_config.high_error_threshold,
                })

            # Low direction accuracy
            if metrics.direction_accuracy < self.alert_config.low_accuracy_threshold:
                issues.append({
                    'severity': 'medium',
                    'type': 'low_direction_accuracy',
                    'model': model_type,
                    'message': f"Direction accuracy ({metrics.direction_accuracy:.2%}) below threshold ({self.alert_config.low_accuracy_threshold:.2%})",
                    'value': metrics.direction_accuracy,
                    'threshold': self.alert_config.low_accuracy_threshold,
                })

            # High missing actual rate (too many pending predictions)
            if metrics.missing_actual_rate > 0.5:
                issues.append({
                    'severity': 'low',
                    'type': 'high_pending_rate',
                    'model': model_type,
                    'message': f"{metrics.missing_actual_rate:.1%} of predictions still pending (may need more time for results)",
                    'value': metrics.missing_actual_rate,
                })

        return issues

    def _get_recent_errors(
        self,
        model_types: List[str],
        limit: int = 10
    ) -> List[Dict[str, Any]]:
        """Get recent prediction errors."""
        errors = []

        for model_type in model_types:
            matched = self.prediction_matcher.get_matched_predictions(
                model_type=model_type,
                limit=limit
            )

            for m in matched:
                errors.append({
                    'model': model_type,
                    'wallet_address': m.wallet_address,
                    'predicted_pnl_sol': m.predicted_pnl_sol,
                    'actual_pnl_sol': m.actual_pnl_sol,
                    'error': m.error,
                    'abs_error': m.abs_error,
                    'direction_correct': m.direction_correct,
                    'prediction_timestamp': m.prediction_timestamp,
                })

        # Sort by absolute error (descending)
        errors.sort(key=lambda x: x['abs_error'], reverse=True)
        return errors[:limit]

    def _generate_recommendations(self, report: Dict[str, Any]) -> List[str]:
        """Generate actionable recommendations."""
        recommendations = []

        summary = report.get('summary', {})
        report.get('issues', [])

        # Analyze RMSE
        avg_rmse = summary.get('avg_rmse', 0)
        if avg_rmse > self.alert_config.high_error_threshold:
            recommendations.append(
                f"Consider retraining models - average RMSE ({avg_rmse:.4f} SOL) exceeds threshold"
            )

        # Analyze direction accuracy
        avg_direction_acc = summary.get('avg_direction_accuracy', 0)
        if avg_direction_acc < self.alert_config.low_accuracy_threshold:
            recommendations.append(
                f"Review feature engineering - direction accuracy ({avg_direction_acc:.2%}) is below threshold"
            )

        # Check for best model
        best_by_rmse = summary.get('best_model_by_rmse')
        if best_by_rmse and len(report.get('models_with_data', [])) > 1:
            recommendations.append(
                f"Consider using {best_by_rmse} as primary model (lowest RMSE)"
            )

        # Check pending rate
        total_pending = summary.get('total_pending', 0)
        if total_pending > 100:
            recommendations.append(
                f"High number of pending predictions ({total_pending}) - ensure matching is running regularly"
            )

        # Model-specific recommendations
        model_metrics = report.get('model_metrics', {})
        for model_type, metrics in model_metrics.items():
            # Check correlation
            if metrics.get('correlation', 0) < 0.3:
                recommendations.append(
                    f"{model_type}: Low correlation ({metrics.get('correlation', 0):.3f}) between predictions and actuals"
                )

            # Check profit prediction
            if metrics.get('mean_predicted_profit', 0) > 0 and metrics.get('mean_actual_profit', 0) < 0:
                recommendations.append(
                    f"{model_type}: Systematically overestimating profitability"
                )

        if not recommendations:
            recommendations.append("Model performance looks healthy - continue monitoring")

        return recommendations

    def send_alert(
        self,
        condition: str,
        details: Dict[str, Any],
        alert_level: str = "warning"
    ):
        """
        Send alert for validation issues.

        Implements full alerting strategy:
        1. Log to validation log file
        2. Save alert JSON to alerts/ directory
        3. Send webhook notification (Discord/Slack) if configured

        Args:
            condition: Alert condition type
            details: Alert details
            alert_level: Alert level (info, warning, error)
        """
        logger.warning(f"Alert triggered: {condition} - {details.get('message', '')}")

        # 1. Log to file
        log_path = Path(self.alert_config.alert_dir) / "validation_alerts.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)

        with open(log_path, 'a') as f:
            f.write(f"{datetime.utcnow().isoformat()} [{alert_level.upper()}] {condition}: {details.get('message', '')}\n")

        # 2. Save alert JSON
        alert_data = {
            'timestamp': datetime.utcnow().isoformat(),
            'condition': condition,
            'level': alert_level,
            'details': details,
        }

        alert_file = Path(self.alert_config.alert_dir) / f"alert_{datetime.utcnow().strftime('%Y%m%d_%H%M%S')}_{condition}.json"
        alert_file.parent.mkdir(parents=True, exist_ok=True)

        with open(alert_file, 'w') as f:
            json.dump(alert_data, f, indent=2)

        logger.info(f"Alert saved to {alert_file}")

        # 3. Send webhook if configured
        if self.alert_config.webhook_url:
            self._send_webhook(alert_data)

    def _send_webhook(self, alert_data: Dict[str, Any]):
        """Send webhook notification."""
        import urllib.request
        import json

        try:
            # Format message for Discord/Slack
            condition = alert_data.get('condition', 'unknown')
            level = alert_data.get('level', 'info').upper()
            message = alert_data.get('details', {}).get('message', 'No message')
            timestamp = alert_data.get('timestamp', '')

            # Discord-compatible format
            payload = {
                'content': f"🚨 **[{level}] Scout Validation Alert: {condition}**\n\n"
                           f"{message}\n\n"
                           f"Timestamp: {timestamp}"
            }

            data = json.dumps(payload).encode('utf-8')
            req = urllib.request.Request(
                self.alert_config.webhook_url,
                data=data,
                headers={'Content-Type': 'application/json'}
            )

            with urllib.request.urlopen(req, timeout=5) as response:
                if response.status == 200:
                    logger.info("Webhook notification sent successfully")
                else:
                    logger.warning(f"Webhook returned status {response.status}")

        except Exception as e:
            logger.error(f"Failed to send webhook: {e}")

    def save_report(
        self,
        report: Dict[str, Any],
        output_path: Optional[str] = None
    ) -> str:
        """
        Save report to file.

        Args:
            report: Report dictionary
            output_path: Optional output path

        Returns:
            Path to saved report
        """
        if output_path is None:
            report_dir = Path("data/validation_reports")
            report_dir.mkdir(parents=True, exist_ok=True)
            timestamp = datetime.utcnow().strftime('%Y%m%d_%H%M%S')
            output_path = report_dir / f"validation_report_{timestamp}.json"
        else:
            output_path = Path(output_path)
            output_path.parent.mkdir(parents=True, exist_ok=True)

        with open(output_path, 'w') as f:
            json.dump(report, f, indent=2)

        logger.info(f"Report saved to {output_path}")
        return str(output_path)

    def generate_html_report(
        self,
        output_path: str,
        model_types: Optional[List[str]] = None,
        time_window: str = '7d'
    ):
        """
        Generate HTML report with basic charts.

        Args:
            output_path: Path to save HTML report
            model_types: Optional list of model types
            time_window: Time window for analysis
        """
        # Generate the report
        report = self.generate_report(
            model_types=model_types,
            time_window=time_window,
            output_format='dict'
        )

        # Convert to HTML
        html = self._report_to_html(report)

        # Save
        output_path = Path(output_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)

        with open(output_path, 'w') as f:
            f.write(html)

        logger.info(f"HTML report saved to {output_path}")

    def _report_to_html(self, report: Dict[str, Any]) -> str:
        """Convert report to HTML."""
        summary = report.get('summary', {})
        model_metrics = report.get('model_metrics', {})
        issues = report.get('issues', [])
        recommendations = report.get('recommendations', [])

        html = f"""
<!DOCTYPE html>
<html>
<head>
    <title>Scout Validation Report</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; background: #f5f5f5; }}
        .container {{ max-width: 1200px; margin: 0 auto; background: white; padding: 20px; border-radius: 8px; }}
        h1 {{ color: #333; border-bottom: 2px solid #007bff; padding-bottom: 10px; }}
        h2 {{ color: #555; margin-top: 30px; }}
        .summary {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 15px; margin: 20px 0; }}
        .metric {{ background: #f8f9fa; padding: 15px; border-radius: 5px; border-left: 4px solid #007bff; }}
        .metric-label {{ font-size: 12px; color: #666; }}
        .metric-value {{ font-size: 24px; font-weight: bold; color: #333; }}
        table {{ width: 100%; border-collapse: collapse; margin: 20px 0; }}
        th, td {{ padding: 12px; text-align: left; border-bottom: 1px solid #ddd; }}
        th {{ background: #007bff; color: white; }}
        tr:hover {{ background: #f8f9fa; }}
        .issue {{ padding: 10px; margin: 10px 0; border-radius: 5px; }}
        .issue.high {{ background: #f8d7da; border-left: 4px solid #dc3545; }}
        .issue.medium {{ background: #fff3cd; border-left: 4px solid #ffc107; }}
        .issue.low {{ background: #d1ecf1; border-left: 4px solid #17a2b8; }}
        .recommendations {{ background: #d4edda; padding: 15px; border-radius: 5px; }}
        .recommendations li {{ margin: 5px 0; }}
        .timestamp {{ color: #666; font-size: 14px; }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Scout ML Validation Report</h1>
        <p class="timestamp">Generated: {report.get('generated_at', 'N/A')} | Time Window: {report.get('time_window', 'N/A')}</p>

        <h2>Summary</h2>
        <div class="summary">
            <div class="metric">
                <div class="metric-label">Total Models</div>
                <div class="metric-value">{summary.get('total_models', 0)}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Total Predictions</div>
                <div class="metric-value">{summary.get('total_predictions', 0)}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Matched</div>
                <div class="metric-value">{summary.get('total_matched', 0)}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Avg RMSE</div>
                <div class="metric-value">{summary.get('avg_rmse', 0):.4f}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Avg Correlation</div>
                <div class="metric-value">{summary.get('avg_correlation', 0):.3f}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Avg Direction Accuracy</div>
                <div class="metric-value">{summary.get('avg_direction_accuracy', 0):.1%}</div>
            </div>
        </div>

        <h2>Model Metrics</h2>
        <table>
            <tr>
                <th>Model</th>
                <th>Predictions</th>
                <th>Matched</th>
                <th>RMSE</th>
                <th>Correlation</th>
                <th>Direction Accuracy</th>
                <th>Mean Days to Match</th>
            </tr>
"""

        for model_type, metrics in model_metrics.items():
            html += f"""
            <tr>
                <td>{model_type}</td>
                <td>{metrics.get('total_predictions', 0)}</td>
                <td>{metrics.get('matched_predictions', 0)}</td>
                <td>{metrics.get('rmse', 0):.4f}</td>
                <td>{metrics.get('correlation', 0):.3f}</td>
                <td>{metrics.get('direction_accuracy', 0):.1%}</td>
                <td>{metrics.get('mean_days_to_match', 0):.1f}</td>
            </tr>
"""

        html += """
        </table>
"""

        if issues:
            html += "<h2>Issues Detected</h2>"
            for issue in issues:
                severity = issue.get('severity', 'low')
                html += f"""
                <div class="issue {severity}">
                    <strong>{severity.upper()}:</strong> {issue.get('message', 'No message')}
                    <br><small>Model: {issue.get('model', 'N/A')} | Value: {issue.get('value', 'N/A')}</small>
                </div>
                """

        if recommendations:
            html += "<h2>Recommendations</h2>"
            html += '<div class="recommendations"><ul>'
            for rec in recommendations:
                html += f"<li>{rec}</li>"
            html += "</ul></div>"

        html += """
    </div>
</body>
</html>
"""
        return html


# Global instance
_global_reporter = None


def get_validation_reporter(db_path: Optional[str] = None) -> ValidationReporter:
    """Get or create global validation reporter instance."""
    global _global_reporter
    if _global_reporter is None:
        _global_reporter = ValidationReporter(db_path)
    return _global_reporter
