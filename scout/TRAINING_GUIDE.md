# Scout ML Model Training Guide

## Overview

The training pipeline provides a complete system for training ML models with historical wallet data from the Scout module.

## Components

### 1. TrainingDataLoader
Loads wallet features and PnL targets from the SQLite database.

```python
from scout.core.training_data_loader import TrainingDataLoader

loader = TrainingDataLoader(db_path="data/chimera.db")
X_train, y_train, X_val, y_val, feature_names = loader.create_training_dataset()
```

### 2. TrainingFeatureExtractor
Enriches wallet features with all available feature types.

```python
from scout.core.training_feature_extractor import FeatureEnricher

enricher = FeatureEnricher()
enriched_features = enricher.enrich_wallet_metrics(wallet_metrics, trade_history)
```

### 3. Training Scripts
- `train_ml_models.py` - Main training script
- `evaluate_models.py` - Model evaluation script

## Usage

### Quick Start

1. **Train all models:**
```bash
cd scout
python -m scripts.train_ml_models --db-path ../data/chimera.db
```

2. **Evaluate models:**
```bash
python -m scripts.evaluate_models --model-dir ../data/models
```

### Advanced Options

**Training with custom settings:**
```bash
python -m scripts.train_ml_models \
    --db-path ../data/chimera.db \
    --output-dir ../data/models \
    --target-column roi_30d \
    --min-trades 5 \
    --val-split 0.2
```

**Evaluation with output:**
```bash
python -m scripts.evaluate_models \
    --model-dir ../data/models \
    --db-path ../data/chimera.db \
    --output results/evaluation.json
```

## Programmatic Usage

### Training Models Directly

```python
from scout.core.training_data_loader import TrainingDataLoader
from scout.core.gradient_boost_predictor import GradientBoostPredictor
from scout.core.meta_learner import MetaLearner

# Load data
loader = TrainingDataLoader("data/chimera.db")
X_train, y_train, X_val, y_val, feature_names = loader.create_training_dataset()

# Train XGBoost
xgb_model = GradientBoostPredictor(model_type="xgboost")
xgb_results = xgb_model.train_from_arrays(
    X_train, y_train, X_val, y_val, feature_names
)

# Train LightGBM
lgb_model = GradientBoostPredictor(model_type="lightgbm")
lgb_results = lgb_model.train_from_arrays(
    X_train, y_train, X_val, y_val, feature_names
)

# Train Meta-Learner
base_models = {"xgboost": xgb_model, "lightgbm": lgb_model}
meta_learner = MetaLearner()
meta_results = meta_learner.train_from_arrays(
    X_train, y_train, X_val, y_val, feature_names,
    base_model_predictors=base_models
)
```

### Using Trained Models for Predictions

```python
from scout.core.ml_integration import enhance_wqs_with_ml

# Enhance WQS with ML predictions
result = enhance_wqs_with_ml(
    wallet_metrics=wallet_metrics,
    strategy="SHIELD",
    trade_history=trade_history
)

print(f"Enhanced WQS: {result['adjusted_wqs']}")
print(f"ML Prediction: {result['ml_predictions'].get('predicted_pnl_sol')}")
```

## Data Requirements

### Minimum Requirements
- **Wallets table** with historical wallet metrics
- **Minimum trades**: 5 trades per wallet (configurable)
- **Target column**: `roi_30d` (or other metric)

### Optional Data
- **Trade history** for time-series features
- **SOL price history** for market context features
- **Transaction graph** for network features

## Model Outputs

Trained models are saved to the specified output directory (default: `data/models/`):

```
data/models/
├── xgboost_profitability.json          # XGBoost model
├── lightgbm_profitability.txt          # LightGBM model
├── gradient_boost_metadata.json        # Model metadata
├── meta_learner.pkl                    # Meta-learner model
├── meta_weights.json                   # Model weights
└── training_summary.json               # Training results
```

## Success Metrics

- **RMSE**: Root Mean Squared Error (lower is better)
- **MAE**: Mean Absolute Error (lower is better)
- **R²**: Coefficient of determination (higher is better, max 1.0)
- **Correlation**: Pearson correlation (higher is better, max 1.0)
- **Direction Accuracy**: % of correct direction predictions

## Troubleshooting

### No Data Available
```
Error: No wallet features loaded
Solution: Ensure chimera.db exists and has wallet data
```

### Insufficient Training Samples
```
Error: Insufficient training data
Solution: Lower --min-trades threshold or accumulate more data
```

### Model Dependencies Missing
```
Warning: XGBoost not available
Solution: pip install xgboost lightgbm scikit-learn
```

## Integration with Scout

The trained models are automatically used by the Scout module when ML is enabled:

1. Set `SCOUT_ML_ENABLED=true` in environment or config
2. Models are loaded on Scout startup
3. WQS calculations are enhanced with ML predictions
4. Predictions are logged to monitoring system

## Next Steps

1. **Train initial models** with available historical data
2. **Evaluate performance** on holdout test set
3. **Monitor predictions** in production
4. **Retrain periodically** as new data accumulates
5. **Experiment with hyperparameters** using Optuna integration

## References

- Training Plan: `CLAUDE.md` - Model Training Plan section
- ML Architecture: `scout/core/gradient_boost_predictor.py`
- Feature Extraction: `scout/core/training_feature_extractor.py`
- Model Registry: `scout/core/model_registry.py`
- Monitoring: `scout/core/model_monitoring.py`
