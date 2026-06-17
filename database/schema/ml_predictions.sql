-- ML Predictions Table for Model Validation
-- Stores all ML predictions for later validation against actual results
-- Part of Scout module model validation infrastructure

CREATE TABLE IF NOT EXISTS ml_predictions (
    -- Primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Prediction identifiers
    wallet_address TEXT NOT NULL,
    prediction_timestamp TIMESTAMP NOT NULL,
    model_type TEXT NOT NULL,

    -- Prediction values
    predicted_pnl_sol REAL NOT NULL,
    predicted_class TEXT,
    confidence REAL,

    -- Feature context
    features_json TEXT,
    strategy TEXT,
    wqs_score_at_prediction REAL,
    wqs_components_json TEXT,

    -- Actual results (filled when matched)
    actual_pnl_sol REAL,
    actual_pnl_7d_sol REAL,
    actual_pnl_30d_sol REAL,
    match_timestamp TIMESTAMP,
    days_to_match INTEGER,

    -- Status tracking
    status TEXT DEFAULT 'PENDING',

    -- Audit timestamps
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Constraint: unique prediction per wallet/model/timestamp
    UNIQUE(wallet_address, prediction_timestamp, model_type)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_ml_predictions_wallet ON ml_predictions(wallet_address);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_status ON ml_predictions(status);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_timestamp ON ml_predictions(prediction_timestamp);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_model ON ml_predictions(model_type);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_match_status ON ml_predictions(status, prediction_timestamp);

-- Status values:
--   PENDING - Prediction made, awaiting actual results
--   MATCHED - Actual PnL matched to prediction
--   EXPIRED - Prediction too old to match (configurable threshold, default 90 days)
--   INVALID - Prediction data invalid or wallet not found
