#!/usr/bin/env python3
"""
Evaluate trained ML models on holdout data.

This script evaluates trained models on test data and calculates
performance metrics including RMSE, MAE, R², and correlation.

Usage:
    python -m scout.scripts.evaluate_models --model-dir data/models --db-path data/chimera.db
"""

import argparse
import json
import logging
import sys
from pathlib import Path
from datetime import datetime

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

import numpy as np

try:
    from sklearn.metrics import r2_score
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False
    logging.warning("scikit-learn not available - some metrics will be limited")

from scout.core.training_data_loader import TrainingDataLoader
from scout.core.gradient_boost_predictor import GradientBoostPredictor

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


def calculate_evaluation_metrics(y_true, y_pred):
    """
    Calculate evaluation metrics for model performance.

    Args:
        y_true: True target values
        y_pred: Predicted values

    Returns:
        Dictionary of metrics
    """
    metrics = {}

    # Ensure numpy arrays
    y_true = np.array(y_true)
    y_pred = np.array(y_pred)

    # Basic metrics
    metrics['rmse'] = float(np.sqrt(np.mean((y_true - y_pred) ** 2)))
    metrics['mae'] = float(np.mean(np.abs(y_true - y_pred)))

    # R² and correlation
    if SKLEARN_AVAILABLE:
        metrics['r2'] = float(r2_score(y_true, y_pred))
    else:
        # Manual R² calculation
        ss_res = np.sum((y_true - y_pred) ** 2)
        ss_tot = np.sum((y_true - np.mean(y_true)) ** 2)
        metrics['r2'] = float(1 - (ss_res / ss_tot)) if ss_tot > 0 else 0.0

    # Correlation
    if len(y_true) > 1:
        correlation_matrix = np.corrcoef(y_true, y_pred)
        if correlation_matrix.shape == (2, 2):
            metrics['correlation'] = float(correlation_matrix[0, 1])
        else:
            metrics['correlation'] = 1.0
    else:
        metrics['correlation'] = 0.0

    # Additional metrics
    metrics['mean_absolute_percentage_error'] = float(
        np.mean(np.abs((y_true - y_pred) / (y_true + 1e-8))) * 100
    )

    # Direction accuracy (for financial predictions)
    if len(y_true) > 1:
        true_direction = np.sign(y_true)
        pred_direction = np.sign(y_pred)
        metrics['direction_accuracy'] = float(
            np.mean(true_direction == pred_direction)
        )
    else:
        metrics['direction_accuracy'] = 0.0

    return metrics


def evaluate_models(
    model_dir="data/models",
    db_path="data/chimera.db",
    target_column="roi_30d",
    min_trades=5,
    output_file=None
):
    """
    Evaluate all trained models on test data.

    Args:
        model_dir: Directory containing trained models
        db_path: Path to SQLite database
        target_column: Target column name
        min_trades: Minimum number of trades required
        output_file: Optional path to save evaluation results

    Returns:
        Dictionary with evaluation results
    """
    logger.info("Starting model evaluation")
    logger.info(f"Model directory: {model_dir}")
    logger.info(f"Database: {db_path}")

    # 1. Load test data
    logger.info("Loading test data...")
    loader = TrainingDataLoader(db_path)

    try:
        X_test, y_test, feature_names = loader.create_test_dataset(
            target_column=target_column,
            min_trades=min_trades
        )
    except ValueError as e:
        logger.error(f"Failed to load test data: {e}")
        return {'error': 'data_loading_failed', 'message': str(e)}

    logger.info(f"Loaded {len(X_test)} test samples with {len(feature_names)} features")

    # 2. Load and evaluate models
    results = {}

    # Load XGBoost model
    xgb_path = Path(model_dir) / "xgboost_profitability.json"
    if xgb_path.exists():
        logger.info("Evaluating XGBoost model...")
        try:
            xgb_model = GradientBoostPredictor(
                model_type="xgboost",
                model_path=model_dir
            )

            # Make predictions
            predictions = []
            for i in range(len(X_test)):
                features = dict(zip(feature_names, X_test[i]))
                pred = xgb_model.predict_profitability(features)
                predictions.append(pred.get('predicted_pnl_sol', 0.0))

            # Calculate metrics
            metrics = calculate_evaluation_metrics(y_test, predictions)
            results['xgboost'] = {
                'status': 'success',
                'metrics': metrics,
                'model_path': str(xgb_path),
            }

            logger.info(f"XGBoost RMSE: {metrics['rmse']:.4f}, R²: {metrics['r2']:.4f}")

        except Exception as e:
            logger.error(f"XGBoost evaluation failed: {e}")
            results['xgboost'] = {'status': 'failed', 'error': str(e)}
    else:
        logger.warning(f"XGBoost model not found at {xgb_path}")
        results['xgboost'] = {'status': 'not_found'}

    # Load LightGBM model
    lgb_path = Path(model_dir) / "lightgbm_profitability.txt"
    if lgb_path.exists():
        logger.info("Evaluating LightGBM model...")
        try:
            lgb_model = GradientBoostPredictor(
                model_type="lightgbm",
                model_path=model_dir
            )

            # Make predictions
            predictions = []
            for i in range(len(X_test)):
                features = dict(zip(feature_names, X_test[i]))
                pred = lgb_model.predict_profitability(features)
                predictions.append(pred.get('predicted_pnl_sol', 0.0))

            # Calculate metrics
            metrics = calculate_evaluation_metrics(y_test, predictions)
            results['lightgbm'] = {
                'status': 'success',
                'metrics': metrics,
                'model_path': str(lgb_path),
            }

            logger.info(f"LightGBM RMSE: {metrics['rmse']:.4f}, R²: {metrics['r2']:.4f}")

        except Exception as e:
            logger.error(f"LightGBM evaluation failed: {e}")
            results['lightgbm'] = {'status': 'failed', 'error': str(e)}
    else:
        logger.warning(f"LightGBM model not found at {lgb_path}")
        results['lightgbm'] = {'status': 'not_found'}

    # 3. Compile summary
    evaluation_summary = {
        'timestamp': datetime.utcnow().isoformat(),
        'test_samples': len(X_test),
        'feature_count': len(feature_names),
        'target_column': target_column,
        'results': results,
        'model_directory': model_dir,
    }

    # Find best model
    best_model = None
    best_rmse = float('inf')

    for model_name, model_results in results.items():
        if model_results.get('status') == 'success' and 'metrics' in model_results:
            rmse = model_results['metrics'].get('rmse', float('inf'))
            if rmse < best_rmse:
                best_rmse = rmse
                best_model = model_name

    if best_model:
        evaluation_summary['best_model'] = best_model
        evaluation_summary['best_rmse'] = best_rmse
        logger.info(f"\nBest model: {best_model} (Test RMSE: {best_rmse:.4f})")

    # 4. Save results
    if output_file:
        output_path = Path(output_file)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            with open(output_path, 'w') as f:
                json.dump(evaluation_summary, f, indent=2)
            logger.info(f"Evaluation results saved to {output_path}")
        except Exception as e:
            logger.error(f"Failed to save evaluation results: {e}")

    return evaluation_summary


def main():
    """Main entry point for the evaluation script."""
    parser = argparse.ArgumentParser(
        description="Evaluate trained ML models on test data",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Evaluate models with default settings
    python -m scout.scripts.evaluate_models

    # Evaluate with custom model directory
    python -m scout.scripts.evaluate_models --model-dir /path/to/models

    # Save evaluation results
    python -m scout.scripts.evaluate_models --output results/evaluation.json
        """
    )

    parser.add_argument(
        "--model-dir",
        default="data/models",
        help="Directory containing trained models (default: data/models)"
    )
    parser.add_argument(
        "--db-path",
        default="data/chimera.db",
        help="Path to SQLite database (default: data/chimera.db)"
    )
    parser.add_argument(
        "--target-column",
        default="roi_30d",
        help="Target column name (default: roi_30d)"
    )
    parser.add_argument(
        "--min-trades",
        type=int,
        default=5,
        help="Minimum number of trades required (default: 5)"
    )
    parser.add_argument(
        "--output",
        help="Path to save evaluation results (JSON format)"
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Enable verbose logging"
    )

    args = parser.parse_args()

    # Set logging level
    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    # Run evaluation
    try:
        results = evaluate_models(
            model_dir=args.model_dir,
            db_path=args.db_path,
            target_column=args.target_column,
            min_trades=args.min_trades,
            output_file=args.output
        )

        # Print summary
        print("\n" + "=" * 60)
        print("EVALUATION SUMMARY")
        print("=" * 60)
        print(f"Test samples: {results.get('test_samples', 'N/A')}")
        print(f"Feature count: {results.get('feature_count', 'N/A')}")
        print(f"Best model: {results.get('best_model', 'N/A')}")
        print(f"Best Test RMSE: {results.get('best_rmse', 'N/A')}")

        print("\nModel Metrics:")
        for model_name, model_results in results.get('results', {}).items():
            if model_results.get('status') == 'success' and 'metrics' in model_results:
                metrics = model_results['metrics']
                print(f"\n{model_name.upper()}:")
                print(f"  RMSE: {metrics.get('rmse', 'N/A'):.4f}")
                print(f"  MAE: {metrics.get('mae', 'N/A'):.4f}")
                print(f"  R²: {metrics.get('r2', 'N/A'):.4f}")
                print(f"  Correlation: {metrics.get('correlation', 'N/A'):.4f}")
                print(f"  Direction Accuracy: {metrics.get('direction_accuracy', 'N/A'):.2%}")

        print("=" * 60)

    except KeyboardInterrupt:
        logger.info("Evaluation interrupted by user")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Evaluation failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
