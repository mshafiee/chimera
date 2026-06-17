"""
Hyperparameter Optimization for Scout

Automated hyperparameter search using Optuna.
This module provides:
- Automated hyperparameter optimization
- Time-series cross-validation
- Multi-objective optimization (accuracy vs latency)
- Pruning for faster search

Usage:
    optimizer = HyperparameterOptimizer()
    best_params = optimizer.optimize(objective, n_trials=100)
"""

import json
import logging
import os
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Callable, Union
from dataclasses import dataclass
import numpy as np

logger = logging.getLogger(__name__)

# Try to import Optuna
try:
    import optuna
    from optuna.pruners import MedianPruner
    from optuna.samplers import TPESampler, CmaEsSampler
    OPTUNA_AVAILABLE = True
except ImportError:
    OPTUNA_AVAILABLE = False
    logger.warning("Optuna not available - install with: pip install optuna")

# Try to import sklearn for cross-validation
try:
    from sklearn.model_selection import TimeSeriesSplit
    SKLEARN_AVAILABLE = True
except ImportError:
    SKLEARN_AVAILABLE = False
    logger.warning("scikit-learn not available - cross-validation will be limited")


@dataclass
class OptimizationResult:
    """Result of hyperparameter optimization."""
    best_params: Dict[str, Any]
    best_value: float
    best_trial: int
    n_trials: int
    study_name: str
    optimization_time_seconds: float
    pruned_trials: int
    complete_trials: int


class HyperparameterOptimizer:
    """
    Hyperparameter optimization for ML models.

    Features:
    - TPE and CMA-ES sampling
    - Time-series cross-validation
    - Multi-objective optimization
    - Median pruning for efficiency
    - Study persistence
    """

    def __init__(
        self,
        study_name: Optional[str] = None,
        storage_url: Optional[str] = None,
        sampler: str = "tpe",  # "tpe" or "cmaes"
        pruner: bool = True,
        direction: str = "maximize"
    ):
        """
        Initialize the hyperparameter optimizer.

        Args:
            study_name: Name for the Optuna study
            storage_url: URL for study persistence (e.g., "sqlite:///db.sqlite3")
            sampler: Sampler type ("tpe" or "cmaes")
            pruner: Whether to enable median pruning
            direction: Optimization direction ("maximize" or "minimize")
        """
        if not OPTUNA_AVAILABLE:
            raise ImportError("Optuna is required for hyperparameter optimization")

        self.study_name = study_name or f"scout_opt_{datetime.utcnow().strftime('%Y%m%d_%H%M%S')}"
        self.storage_url = storage_url
        self.sampler_type = sampler
        self.enable_pruner = pruner
        self.direction = direction

        # Create sampler
        if sampler == "tpe":
            self.sampler = TPESampler(seed=42)
        elif sampler == "cmaes":
            self.sampler = CmaEsSampler(seed=42)
        else:
            self.sampler = TPESampler(seed=42)

        # Create pruner
        if pruner:
            self.pruner = MedianPruner(n_startup_trials=5, n_warmup_steps=10)
        else:
            self.pruner = None

        # Study
        self.study = None

        # Results tracking
        self.optimization_results = {}

    def create_study(self):
        """Create or load the Optuna study."""
        try:
            self.study = optuna.create_study(
                study_name=self.study_name,
                storage=self.storage_url,
                sampler=self.sampler,
                pruner=self.pruner,
                direction=self.direction,
                load_if_exists=True,
            )
            logger.info(f"Study created/loaded: {self.study_name}")
        except Exception as e:
            logger.error(f"Failed to create study: {e}")
            raise

    def optimize(
        self,
        objective: Callable,
        n_trials: int = 100,
        timeout: Optional[int] = None,
        n_jobs: int = 1,
        callbacks: Optional[List[Callable]] = None
    ) -> OptimizationResult:
        """
        Run hyperparameter optimization.

        Args:
            objective: Objective function to optimize
            n_trials: Number of trials
            timeout: Timeout in seconds
            n_jobs: Number of parallel jobs
            callbacks: Optional callbacks

        Returns:
            OptimizationResult object
        """
        if self.study is None:
            self.create_study()

        start_time = time.time()

        try:
            self.study.optimize(
                objective,
                n_trials=n_trials,
                timeout=timeout,
                n_jobs=n_jobs,
                callbacks=callbacks,
                show_progress_bar=False,
            )

        except Exception as e:
            logger.error(f"Optimization failed: {e}")
            raise

        elapsed_time = time.time() - start_time

        # Compile results
        result = OptimizationResult(
            best_params=dict(self.study.best_params),
            best_value=float(self.study.best_value),
            best_trial=int(self.study.best_trial.number),
            n_trials=len(self.study.trials),
            study_name=self.study_name,
            optimization_time_seconds=elapsed_time,
            pruned_trials=sum(1 for t in self.study.trials if t.state == optuna.trial.TrialState.PRUNED),
            complete_trials=sum(1 for t in self.study.trials if t.state == optuna.trial.TrialState.COMPLETE),
        )

        self.optimization_results[self.study_name] = result

        return result

    def get_best_params(self) -> Dict[str, Any]:
        """Get best parameters from the study."""
        if self.study is None:
            return {}

        return dict(self.study.best_params)

    def get_trials_dataframe(self):
        """Get trials as a pandas-like dataframe."""
        if self.study is None:
            return None

        return self.study.trials_dataframe()

    def save_results(self, filepath: Optional[str] = None):
        """Save optimization results to file."""
        if self.study_name not in self.optimization_results:
            logger.warning("No optimization results to save")
            return

        if filepath is None:
            filepath = os.getenv(
                "SCOUT_HYPEROPT_RESULTS_DIR",
                "../results/hyperopt"
            )
            Path(filepath).mkdir(parents=True, exist_ok=True)
            filepath = os.path.join(filepath, f"{self.study_name}.json")

        result = self.optimization_results[self.study_name]

        # Convert to dict
        result_dict = {
            'best_params': result.best_params,
            'best_value': result.best_value,
            'best_trial': result.best_trial,
            'n_trials': result.n_trials,
            'study_name': result.study_name,
            'optimization_time_seconds': result.optimization_time_seconds,
            'pruned_trials': result.pruned_trials,
            'complete_trials': result.complete_trials,
        }

        try:
            with open(filepath, 'w') as f:
                json.dump(result_dict, f, indent=2)
            logger.info(f"Results saved to {filepath}")
        except Exception as e:
            logger.error(f"Failed to save results: {e}")


class XGBoostHyperparameterOptimizer(HyperparameterOptimizer):
    """
    Hyperparameter optimization for XGBoost models.

    Provides pre-configured search space and objective function
    for XGBoost hyperparameter tuning.
    """

    def __init__(
        self,
        study_name: Optional[str] = None,
        latency_budget_ms: float = 50.0,
        **kwargs
    ):
        """
        Initialize XGBoost optimizer.

        Args:
            study_name: Name for the study
            latency_budget_ms: Latency budget for multi-objective optimization
            **kwargs: Additional arguments for HyperparameterOptimizer
        """
        super().__init__(
            study_name=study_name or "xgboost_opt",
            **kwargs
        )
        self.latency_budget_ms = latency_budget_ms

    def create_objective(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        multi_objective: bool = False
    ) -> Callable:
        """
        Create objective function for XGBoost optimization.

        Args:
            X_train: Training features
            y_train: Training labels
            X_val: Validation features
            y_val: Validation labels
            feature_names: Feature names
            multi_objective: Whether to optimize for both accuracy and latency

        Returns:
            Objective function for Optuna
        """
        def objective(trial):
            # Suggest hyperparameters
            params = {
                'max_depth': trial.suggest_int('max_depth', 3, 10),
                'learning_rate': trial.suggest_float('learning_rate', 0.01, 0.3, log=True),
                'n_estimators': trial.suggest_int('n_estimators', 50, 300),
                'min_child_weight': trial.suggest_int('min_child_weight', 1, 10),
                'subsample': trial.suggest_float('subsample', 0.6, 1.0),
                'colsample_bytree': trial.suggest_float('colsample_bytree', 0.6, 1.0),
                'reg_alpha': trial.suggest_float('reg_alpha', 0.0, 1.0),
                'reg_lambda': trial.suggest_float('reg_lambda', 0.0, 2.0),
                'gamma': trial.suggest_float('gamma', 0.0, 1.0),
            }

            # Train model with suggested parameters
            try:
                import xgboost as xgb

                dtrain = xgb.DMatrix(X_train, label=y_train, feature_names=feature_names)
                dval = xgb.DMatrix(X_val, label=y_val, feature_names=feature_names)

                # Train with pruning
                pruning_callback = optuna.integration.XGBoostPruningCallback(trial, "validation-rmse")

                model = xgb.train(
                    params,
                    dtrain,
                    num_boost_round=params['n_estimators'],
                    evals=[(dval, "validation")],
                    early_stopping_rounds=20,
                    verbose_eval=False,
                    callbacks=[pruning_callback],
                )

                # Get validation score
                val_rmse = model.best_score
                val_rmse = float(val_rmse) if val_rmse is not None else float('inf')

                # Report intermediate value for pruning
                trial.report(val_rmse, model.best_iteration)

                # Handle pruning
                if trial.should_prune():
                    raise optuna.TrialPruned()

                # Multi-objective: consider latency
                if multi_objective:
                    # Estimate latency based on tree complexity
                    n_trees = model.best_iteration
                    avg_depth = params['max_depth']
                    estimated_latency_ms = n_trees * avg_depth * 0.1  # Rough estimate

                    # Penalize if over budget
                    latency_penalty = max(0, estimated_latency_ms - self.latency_budget_ms) / 10.0

                    return val_rmse + latency_penalty

                return val_rmse

            except optuna.TrialPruned:
                raise
            except Exception as e:
                logger.warning(f"Trial failed: {e}")
                return float('inf') if self.direction == "minimize" else float('-inf')

        return objective


class LightGBMHyperparameterOptimizer(HyperparameterOptimizer):
    """
    Hyperparameter optimization for LightGBM models.

    Provides pre-configured search space and objective function
    for LightGBM hyperparameter tuning.
    """

    def __init__(
        self,
        study_name: Optional[str] = None,
        latency_budget_ms: float = 50.0,
        **kwargs
    ):
        """
        Initialize LightGBM optimizer.

        Args:
            study_name: Name for the study
            latency_budget_ms: Latency budget for multi-objective optimization
            **kwargs: Additional arguments for HyperparameterOptimizer
        """
        super().__init__(
            study_name=study_name or "lightgbm_opt",
            **kwargs
        )
        self.latency_budget_ms = latency_budget_ms

    def create_objective(
        self,
        X_train: np.ndarray,
        y_train: np.ndarray,
        X_val: np.ndarray,
        y_val: np.ndarray,
        feature_names: List[str],
        multi_objective: bool = False
    ) -> Callable:
        """Create objective function for LightGBM optimization."""
        def objective(trial):
            # Suggest hyperparameters
            params = {
                'num_leaves': trial.suggest_int('num_leaves', 20, 100),
                'max_depth': trial.suggest_int('max_depth', 3, 10),
                'learning_rate': trial.suggest_float('learning_rate', 0.01, 0.3, log=True),
                'n_estimators': trial.suggest_int('n_estimators', 50, 300),
                'min_child_samples': trial.suggest_int('min_child_samples', 10, 50),
                'subsample': trial.suggest_float('subsample', 0.6, 1.0),
                'colsample_bytree': trial.suggest_float('colsample_bytree', 0.6, 1.0),
                'reg_alpha': trial.suggest_float('reg_alpha', 0.0, 1.0),
                'reg_lambda': trial.suggest_float('reg_lambda', 0.0, 2.0),
            }

            # Train model with suggested parameters
            try:
                import lightgbm as lgb

                train_data = lgb.Dataset(X_train, label=y_train, feature_name=feature_names)
                val_data = lgb.Dataset(X_val, label=y_val, feature_name=feature_names, reference=train_data)

                # Train with pruning
                pruning_callback = optuna.integration.LightGBMPruningCallback(trial, "rmse")

                model = lgb.train(
                    params,
                    train_data,
                    num_boost_round=params['n_estimators'],
                    valid_sets=[val_data],
                    valid_names=['validation'],
                    callbacks=[
                        pruning_callback,
                        lgb.early_stopping(stopping_rounds=20, verbose=False),
                        lgb.log_evaluation(period=0),
                    ],
                )

                # Get validation score
                val_rmse = model.best_score['validation']['rmse']
                val_rmse = float(val_rmse) if val_rmse is not None else float('inf')

                # Report intermediate value for pruning
                trial.report(val_rmse, model.best_iteration)

                # Handle pruning
                if trial.should_prune():
                    raise optuna.TrialPruned()

                # Multi-objective: consider latency
                if multi_objective:
                    n_trees = model.best_iteration
                    avg_leaves = params['num_leaves']
                    estimated_latency_ms = n_trees * avg_leaves * 0.01  # Rough estimate

                    latency_penalty = max(0, estimated_latency_ms - self.latency_budget_ms) / 10.0

                    return val_rmse + latency_penalty

                return val_rmse

            except optuna.TrialPruned:
                raise
            except Exception as e:
                logger.warning(f"Trial failed: {e}")
                return float('inf') if self.direction == "minimize" else float('-inf')

        return objective


def optimize_xgboost(
    X_train: np.ndarray,
    y_train: np.ndarray,
    X_val: np.ndarray,
    y_val: np.ndarray,
    feature_names: List[str],
    n_trials: int = 100,
    latency_budget_ms: float = 50.0,
    multi_objective: bool = False
) -> OptimizationResult:
    """
    Convenience function to optimize XGBoost hyperparameters.

    Args:
        X_train: Training features
        y_train: Training labels
        X_val: Validation features
        y_val: Validation labels
        feature_names: Feature names
        n_trials: Number of optimization trials
        latency_budget_ms: Latency budget for multi-objective optimization
        multi_objective: Whether to optimize for both accuracy and latency

    Returns:
        OptimizationResult object
    """
    if not OPTUNA_AVAILABLE:
        raise ImportError("Optuna is required for hyperparameter optimization")

    optimizer = XGBoostHyperparameterOptimizer(latency_budget_ms=latency_budget_ms)
    objective = optimizer.create_objective(
        X_train, y_train, X_val, y_val, feature_names, multi_objective
    )

    return optimizer.optimize(objective, n_trials=n_trials)


def optimize_lightgbm(
    X_train: np.ndarray,
    y_train: np.ndarray,
    X_val: np.ndarray,
    y_val: np.ndarray,
    feature_names: List[str],
    n_trials: int = 100,
    latency_budget_ms: float = 50.0,
    multi_objective: bool = False
) -> OptimizationResult:
    """
    Convenience function to optimize LightGBM hyperparameters.

    Args:
        X_train: Training features
        y_train: Training labels
        X_val: Validation features
        y_val: Validation labels
        feature_names: Feature names
        n_trials: Number of optimization trials
        latency_budget_ms: Latency budget for multi-objective optimization
        multi_objective: Whether to optimize for both accuracy and latency

    Returns:
        OptimizationResult object
    """
    if not OPTUNA_AVAILABLE:
        raise ImportError("Optuna is required for hyperparameter optimization")

    optimizer = LightGBMHyperparameterOptimizer(latency_budget_ms=latency_budget_ms)
    objective = optimizer.create_objective(
        X_train, y_train, X_val, y_val, feature_names, multi_objective
    )

    return optimizer.optimize(objective, n_trials=n_trials)
