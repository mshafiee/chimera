#!/usr/bin/env python3
"""
Run Model Validation for Scout ML Models

This script runs the scheduled validation workflow that:
1. Matches predictions to actual results
2. Calculates validation metrics
3. Generates validation reports
4. Sends alerts if issues are detected

Usage:
    # Run full validation
    python -m scout.scripts.run_validation

    # Match predictions only
    python -m scout.scripts.run_validation --match-only

    # Generate report only
    python -m scout.scripts.run_validation --report-only

    # Save report to specific path
    python -m scout.scripts.run_validation --output /path/to/report.json

    # Generate HTML report
    python -m scout.scripts.run_validation --html --output report.html
"""

import argparse
import json
import logging
import os
import sys
from datetime import datetime
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from scout.config import ScoutConfig
from scout.core.prediction_matcher import PredictionMatcher, get_prediction_matcher
from scout.core.validation_metrics import ValidationMetricsCalculator, get_metrics_calculator
from scout.core.validation_reporter import ValidationReporter, get_validation_reporter

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


def setup_environment():
    """Setup environment and validate configuration."""
    logger.info("Setting up validation environment")

    # Validate config
    is_valid, warnings = ScoutConfig.validate_config()
    if warnings:
        for warning in warnings:
            logger.warning(f"Config warning: {warning}")

    # Check if validation is enabled
    if not ScoutConfig.get_validation_enabled():
        logger.warning("Validation is disabled via SCOUT_VALIDATION_ENABLED")
        return False

    return True


def run_matching(
    db_path: str,
    lookback_days: int = 7,
    model_type: Optional[str] = None
) -> dict:
    """
    Run prediction matching.

    Args:
        db_path: Path to database
        lookback_days: Lookback window in days
        model_type: Optional model type filter

    Returns:
        Dictionary with matching results
    """
    logger.info(f"Starting prediction matching (lookback: {lookback_days}d)")

    matcher = get_prediction_matcher(db_path)
    results = matcher.match_predictions_to_actuals(
        lookback_days=lookback_days,
        model_type=model_type,
        dry_run=False
    )

    logger.info(
        f"Matching complete: {results.matched_count} matched, "
        f"{results.skipped_count} skipped, {results.failed_count} failed "
        f"({results.processing_time_seconds:.2f}s)"
    )

    return results.to_dict()


def calculate_all_metrics(
    db_path: str,
    time_window: str = '7d',
    model_types: Optional[list] = None
) -> dict:
    """
    Calculate metrics for all models.

    Args:
        db_path: Path to database
        time_window: Time window for analysis
        model_types: Optional list of model types

    Returns:
        Dictionary with metrics for all models
    """
    logger.info(f"Calculating metrics (time_window: {time_window})")

    calculator = get_metrics_calculator(db_path)

    if model_types is None:
        # Get all model types from database
        try:
            import sqlite3
            conn = sqlite3.connect(db_path)
            cursor = conn.cursor()
            cursor.execute("SELECT DISTINCT model_type FROM ml_predictions WHERE status = 'MATCHED'")
            model_types = [row[0] for row in cursor.fetchall()]
            conn.close()
        except Exception as e:
            logger.error(f"Failed to get model types: {e}")
            model_types = ['xgboost', 'lightgbm', 'meta_learner']

    results = {}
    for model_type in model_types:
        metrics = calculator.calculate_metrics(
            model_type=model_type,
            time_window=time_window,
            min_predictions=1
        )
        if metrics:
            results[model_type] = metrics.to_dict()
            logger.info(
                f"{model_type}: RMSE={metrics.rmse:.4f}, "
                f"Correlation={metrics.correlation:.3f}, "
                f"Direction Acc={metrics.direction_accuracy:.1%}"
            )
        else:
            logger.warning(f"No metrics calculated for {model_type}")

    return results


def generate_report(
    db_path: str,
    time_window: str = '7d',
    model_types: Optional[list] = None,
    output_format: str = 'dict'
) -> dict:
    """
    Generate validation report.

    Args:
        db_path: Path to database
        time_window: Time window for analysis
        model_types: Optional list of model types
        output_format: Output format ('dict', 'json')

    Returns:
        Validation report
    """
    logger.info("Generating validation report")

    reporter = get_validation_reporter(db_path)

    if model_types is None:
        try:
            import sqlite3
            conn = sqlite3.connect(db_path)
            cursor = conn.cursor()
            cursor.execute("SELECT DISTINCT model_type FROM ml_predictions WHERE status = 'MATCHED'")
            model_types = [row[0] for row in cursor.fetchall()]
            conn.close()
        except Exception as e:
            logger.error(f"Failed to get model types: {e}")
            model_types = []

    report = reporter.generate_report(
        model_types=model_types,
        time_window=time_window,
        output_format=output_format,
        include_recommendations=True
    )

    if isinstance(report, str):
        report = json.loads(report)

    # Log summary
    summary = report.get('summary', {})
    logger.info(
        f"Report summary: {summary.get('total_models', 0)} models, "
        f"{summary.get('total_matched', 0)} matched predictions, "
        f"avg RMSE={summary.get('avg_rmse', 0):.4f}, "
        f"avg correlation={summary.get('avg_correlation', 0):.3f}"
    )

    # Log issues
    issues = report.get('issues', [])
    if issues:
        logger.warning(f"Found {len(issues)} issues:")
        for issue in issues:
            logger.warning(f"  - [{issue.get('severity', 'unknown')}] {issue.get('message', '')}")

    # Log recommendations
    recommendations = report.get('recommendations', [])
    if recommendations:
        logger.info(f"Recommendations ({len(recommendations)}):")
        for rec in recommendations[:3]:  # Log first 3
            logger.info(f"  - {rec}")

    return report


def save_report(
    reporter: ValidationReporter,
    report: dict,
    output_path: Optional[str] = None
) -> str:
    """
    Save report to file.

    Args:
        reporter: ValidationReporter instance
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

    saved_path = reporter.save_report(report, output_path)
    logger.info(f"Report saved to {saved_path}")
    return saved_path


def check_alerts(
    reporter: ValidationReporter,
    report: dict
) -> list:
    """
    Check for alerts and send notifications.

    Args:
        reporter: ValidationReporter instance
        report: Report dictionary

    Returns:
        List of sent alerts
    """
    alerts_sent = []

    # Check for high error rate
    summary = report.get('summary', {})
    avg_rmse = summary.get('avg_rmse', 0)
    high_error_threshold = ScoutConfig.get_alert_high_error_threshold()

    if avg_rmse > high_error_threshold:
        alert_details = {
            'message': f"Average RMSE ({avg_rmse:.4f}) exceeds threshold ({high_error_threshold:.4f})",
            'avg_rmse': avg_rmse,
            'threshold': high_error_threshold,
        }
        reporter.send_alert('high_error_rate', alert_details, alert_level='warning')
        alerts_sent.append('high_error_rate')

    # Check for low direction accuracy
    avg_direction_acc = summary.get('avg_direction_accuracy', 0)
    low_accuracy_threshold = ScoutConfig.get_alert_low_accuracy_threshold()

    if avg_direction_acc < low_accuracy_threshold:
        alert_details = {
            'message': f"Direction accuracy ({avg_direction_acc:.1%}) below threshold ({low_accuracy_threshold:.1%})",
            'direction_accuracy': avg_direction_acc,
            'threshold': low_accuracy_threshold,
        }
        reporter.send_alert('low_direction_accuracy', alert_details, alert_level='warning')
        alerts_sent.append('low_direction_accuracy')

    # Check for drift (significant issues)
    issues = report.get('issues', [])
    high_severity_issues = [i for i in issues if i.get('severity') == 'high']

    if high_severity_issues:
        alert_details = {
            'message': f"{len(high_severity_issues)} high-severity issues detected",
            'issues': high_severity_issues,
        }
        reporter.send_alert('drift_detected', alert_details, alert_level='error')
        alerts_sent.append('drift_detected')

    if alerts_sent:
        logger.info(f"Sent {len(alerts_sent)} alerts: {', '.join(alerts_sent)}")

    return alerts_sent


def main():
    """Main entry point for validation script."""
    parser = argparse.ArgumentParser(
        description="Run model validation for Scout ML models",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Run full validation
    python -m scout.scripts.run_validation

    # Match predictions only
    python -m scout.scripts.run_validation --match-only

    # Generate report only
    python -m scout.scripts.run_validation --report-only

    # Save report to specific path
    python -m scout.scripts.run_validation --output /path/to/report.json

    # Generate HTML report
    python -m scout.scripts.run_validation --html --output report.html

    # Custom time window
    python -m scout.scripts.run_validation --time-window 30d
        """
    )

    parser.add_argument(
        '--db-path',
        default=os.getenv('CHIMERA_DB_PATH', 'data/chimera.db'),
        help='Path to SQLite database'
    )
    parser.add_argument(
        '--time-window',
        default='7d',
        choices=['7d', '30d', 'all'],
        help='Time window for validation (default: 7d)'
    )
    parser.add_argument(
        '--lookback-days',
        type=int,
        default=7,
        help='Lookback days for prediction matching (default: 7)'
    )
    parser.add_argument(
        '--model-type',
        help='Filter by specific model type'
    )
    parser.add_argument(
        '--match-only',
        action='store_true',
        help='Only run prediction matching, skip metrics and report'
    )
    parser.add_argument(
        '--report-only',
        action='store_true',
        help='Only generate report, skip matching'
    )
    parser.add_argument(
        '--output',
        help='Path to save report output'
    )
    parser.add_argument(
        '--html',
        action='store_true',
        help='Generate HTML report instead of JSON'
    )
    parser.add_argument(
        '--no-alerts',
        action='store_true',
        help='Skip alert checking'
    )
    parser.add_argument(
        '--verbose',
        action='store_true',
        help='Enable verbose logging'
    )

    args = parser.parse_args()

    # Set logging level
    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    logger.info("=" * 60)
    logger.info("Scout Model Validation")
    logger.info("=" * 60)

    # Setup
    if not setup_environment():
        logger.error("Validation setup failed")
        sys.exit(1)

    db_path = args.db_path
    time_window_days = int(args.time_window.replace('d', '')) if args.time_window != 'all' else 90

    # Run matching (unless report-only)
    if not args.report_only:
        try:
            matching_results = run_matching(
                db_path=db_path,
                lookback_days=args.lookback_days,
                model_type=args.model_type
            )

            if args.match_only:
                logger.info("Matching complete (match-only mode)")
                sys.exit(0)

        except Exception as e:
            logger.error(f"Matching failed: {e}")
            import traceback
            traceback.print_exc()
            if args.match_only:
                sys.exit(1)

    # Generate report
    try:
        report = generate_report(
            db_path=db_path,
            time_window=args.time_window,
            model_type=[args.model_type] if args.model_type else None
        )

        # Save report
        reporter = get_validation_reporter(db_path)

        if args.html:
            html_path = args.output or f"data/validation_reports/validation_{datetime.utcnow().strftime('%Y%m%d_%H%M%S')}.html"
            reporter.generate_html_report(
                output_path=html_path,
                model_types=[args.model_type] if args.model_type else None,
                time_window=args.time_window
            )
            logger.info(f"HTML report saved to {html_path}")
        elif args.output:
            save_report(reporter, report, args.output)
        else:
            # Auto-save if there are issues
            issues = report.get('issues', [])
            if issues:
                saved_path = save_report(reporter, report)
                logger.info(f"Report auto-saved due to {len(issues)} issues")

        # Check alerts
        if not args.no_alerts:
            check_alerts(reporter, report)

        logger.info("Validation complete")

        # Exit with error code if there are high-severity issues
        high_severity_issues = [i for i in report.get('issues', []) if i.get('severity') == 'high']
        if high_severity_issues:
            logger.warning(f"Exiting with error due to {len(high_severity_issues)} high-severity issues")
            sys.exit(1)

        sys.exit(0)

    except Exception as e:
        logger.error(f"Validation failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
