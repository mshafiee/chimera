#!/usr/bin/env python3
"""
Train all ML models with historical data.

This script trains XGBoost, LightGBM, and Meta-Learner models
using historical wallet data from the SQLite database.

Usage:
    python -m scout.scripts.train_ml_models --db-path data/chimera.db --output-dir data/models
"""

import argparse
import logging
import sys
from pathlib import Path
from datetime import datetime

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from scout.core.training_data_loader import TrainingDataLoader
from scout.core.archive.gradient_boost_predictor import GradientBoostPredictor
from scout.core.archive.meta_learner import MetaLearner

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


def train_all_models(
    db_path="data/chimera.db",
    output_dir="data/models",
    optimize_hyperparams=False,
    n_trials=50,
    target_column="roi_30d",
    min_trades=5,
    val_split=0.2
):
    """
    Train all ML models with historical data.

    Args:
        db_path: Path to SQLite database
        output_dir: Directory for saving trained models
        optimize_hyperparams: Whether to run hyperparameter optimization
        n_trials: Number of optimization trials
        target_column: Column name for target variable
        min_trades: Minimum number of trades required
        val_split: Validation split ratio

    Returns:
        Dictionary with training results
    """
    logger.info("Starting ML model training pipeline")
    logger.info(f"Database: {db_path}")
    logger.info(f"Output directory: {output_dir}")

    # 1. Load training data
    logger.info("Loading training data...")
    loader = TrainingDataLoader(db_path)

    try:
        X_train, y_train, X_val, y_val, feature_names = loader.create_training_dataset(
            target_column=target_column,
            min_trades=min_trades,
            val_split=val_split
        )
    except ValueError as e:
        logger.error(f"Failed to create training dataset: {e}")
        logger.error("Please ensure the database exists and has sufficient wallet data")
        return {'error': 'data_loading_failed', 'message': str(e)}

    logger.info(f"Loaded {len(X_train)} training samples, {len(X_val)} validation samples")
    logger.info(f"Features: {len(feature_names)}")

    # 2. Create output directory
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    # 3. Train XGBoost model
    logger.info("=" * 60)
    logger.info("Training XGBoost model...")
    logger.info("=" * 60)

    xgb_predictor = GradientBoostPredictor(
        model_type="xgboost",
        model_path=output_dir
    )

    xgb_results = xgb_predictor.train_from_arrays(
        X_train, y_train, X_val, y_val, feature_names
    )

    if 'error' in xgb_results:
        logger.error(f"XGBoost training failed: {xgb_results['error']}")
    else:
        logger.info(f"XGBoost training completed: {xgb_results.get('best_model_type')}")
        logger.info(f"  Training samples: {xgb_results.get('training_samples')}")
        if 'metrics' in xgb_results and 'xgboost' in xgb_results['metrics']:
            metrics = xgb_results['metrics']['xgboost']
            logger.info(f"  Train RMSE: {metrics.get('train_rmse', 'N/A'):.4f}")
            logger.info(f"  Val RMSE: {metrics.get('val_rmse', 'N/A'):.4f}")

    # 4. Train LightGBM model
    logger.info("=" * 60)
    logger.info("Training LightGBM model...")
    logger.info("=" * 60)

    lgb_predictor = GradientBoostPredictor(
        model_type="lightgbm",
        model_path=output_dir
    )

    lgb_results = lgb_predictor.train_from_arrays(
        X_train, y_train, X_val, y_val, feature_names
    )

    if 'error' in lgb_results:
        logger.error(f"LightGBM training failed: {lgb_results['error']}")
    else:
        logger.info(f"LightGBM training completed: {lgb_results.get('best_model_type')}")
        logger.info(f"  Training samples: {lgb_results.get('training_samples')}")
        if 'metrics' in lgb_results and 'lightgbm' in lgb_results['metrics']:
            metrics = lgb_results['metrics']['lightgbm']
            logger.info(f"  Train RMSE: {metrics.get('train_rmse', 'N/A'):.4f}")
            logger.info(f"  Val RMSE: {metrics.get('val_rmse', 'N/A'):.4f}")

    # 5. Train Meta-Learner (if both base models trained successfully)
    logger.info("=" * 60)
    logger.info("Training Meta-Learner...")
    logger.info("=" * 60)

    if 'error' not in xgb_results and 'error' not in lgb_results:
        base_models = {
            "xgboost": xgb_predictor,
            "lightgbm": lgb_predictor,
        }

        try:
            meta_learner = MetaLearner(base_models=base_models)
            meta_results = meta_learner.train_from_arrays(
                X_train, y_train, X_val, y_val, feature_names
            )

            if 'error' in meta_results:
                logger.error(f"Meta-Learner training failed: {meta_results['error']}")
            else:
                logger.info("Meta-Learner training completed")
                logger.info(f"  Training samples: {meta_results.get('training_samples')}")

        except Exception as e:
            logger.error(f"Meta-Learner training failed: {e}")
            meta_results = {'error': str(e)}
    else:
        logger.warning("Skipping Meta-Learner training due to base model failures")
        meta_results = {'error': 'base_models_failed'}

    # 6. Compile results
    results = {
        'timestamp': datetime.utcnow().isoformat(),
        'training_samples': len(X_train) + len(X_val),
        'validation_samples': len(X_val),
        'feature_count': len(feature_names),
        'xgboost': xgb_results,
        'lightgbm': lgb_results,
        'meta_learner': meta_results,
        'model_directory': str(output_path),
    }

    # Determine best model
    best_model = None
    best_score = float('inf')

    for model_name, model_results in [('xgboost', xgb_results), ('lightgbm', lgb_results)]:
        if 'error' not in model_results and 'metrics' in model_results:
            metrics = model_results['metrics'].get(model_name, {})
            val_rmse = metrics.get('val_rmse', float('inf'))
            if val_rmse < best_score:
                best_score = val_rmse
                best_model = model_name

    if best_model:
        results['best_model'] = best_model
        results['best_val_rmse'] = best_score
        logger.info(f"\nBest model: {best_model} (Val RMSE: {best_score:.4f})")

    # 7. Save training summary
    summary_path = output_path / "training_summary.json"
    try:
        import json
        with open(summary_path, 'w') as f:
            json.dump(results, f, indent=2)
        logger.info(f"Training summary saved to {summary_path}")
    except Exception as e:
        logger.error(f"Failed to save training summary: {e}")

    logger.info("=" * 60)
    logger.info("Training pipeline completed")
    logger.info("=" * 60)

    return results


def main():
    """Main entry point for the training script."""
    parser = argparse.ArgumentParser(
        description="Train ML models with historical wallet data",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Train with default settings
    python -m scout.scripts.train_ml_models

    # Train with custom database path
    python -m scout.scripts.train_ml_models --db-path /path/to/chimera.db

    # Train without hyperparameter optimization (faster)
    python -m scout.scripts.train_ml_models --no-optimize

    # Train with specific output directory
    python -m scout.scripts.train_ml_models --output-dir /path/to/models
        """
    )

    parser.add_argument(
        "--db-path",
        default="data/chimera.db",
        help="Path to SQLite database (default: data/chimera.db)"
    )
    parser.add_argument(
        "--output-dir",
        default="data/models",
        help="Output directory for trained models (default: data/models)"
    )
    parser.add_argument(
        "--no-optimize",
        action="store_true",
        help="Skip hyperparameter optimization (faster training)"
    )
    parser.add_argument(
        "--n-trials",
        type=int,
        default=50,
        help="Number of hyperparameter optimization trials (default: 50)"
    )
    parser.add_argument(
        "--target-column",
        default="roi_30d",
        help="Target column name for training (default: roi_30d)"
    )
    parser.add_argument(
        "--min-trades",
        type=int,
        default=5,
        help="Minimum number of trades required (default: 5)"
    )
    parser.add_argument(
        "--val-split",
        type=float,
        default=0.2,
        help="Validation split ratio (default: 0.2)"
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

    # Run training
    try:
        results = train_all_models(
            db_path=args.db_path,
            output_dir=args.output_dir,
            optimize_hyperparams=not args.no_optimize,
            n_trials=args.n_trials,
            target_column=args.target_column,
            min_trades=args.min_trades,
            val_split=args.val_split
        )

        # Print summary
        print("\n" + "=" * 60)
        print("TRAINING SUMMARY")
        print("=" * 60)
        print(f"Training samples: {results.get('training_samples', 'N/A')}")
        print(f"Validation samples: {results.get('validation_samples', 'N/A')}")
        print(f"Feature count: {results.get('feature_count', 'N/A')}")
        print(f"Best model: {results.get('best_model', 'N/A')}")
        print(f"Best Val RMSE: {results.get('best_val_rmse', 'N/A')}")
        print(f"Model directory: {results.get('model_directory', 'N/A')}")
        print("=" * 60)

        if 'error' in results:
            sys.exit(1)

    except KeyboardInterrupt:
        logger.info("Training interrupted by user")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Training failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
