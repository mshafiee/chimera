-- ML Predictions Table for Model Validation
-- Stores all ML predictions for later validation against actual results
-- Part of Scout module model validation infrastructure
-- Financial PnL values stored as TEXT (Decimal strings).

CREATE TABLE IF NOT EXISTS ml_predictions (
    -- Primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Prediction identifiers
    wallet_address TEXT NOT NULL,
    prediction_timestamp TIMESTAMP NOT NULL,
    model_type TEXT NOT NULL,

    -- Prediction values
    predicted_pnl_sol TEXT NOT NULL
        CHECK (predicted_pnl_sol = CAST(predicted_pnl_sol AS NUMERIC)),
    predicted_class TEXT,
    confidence REAL CHECK (confidence IS NULL OR confidence BETWEEN 0 AND 1),

    -- Feature context
    features_json TEXT,
    strategy TEXT,
    wqs_score_at_prediction REAL,
    wqs_components_json TEXT,

    -- Actual results (filled when matched)
    actual_pnl_sol TEXT
        CHECK (actual_pnl_sol IS NULL OR actual_pnl_sol = CAST(actual_pnl_sol AS NUMERIC)),
    actual_pnl_7d_sol TEXT
        CHECK (actual_pnl_7d_sol IS NULL OR actual_pnl_7d_sol = CAST(actual_pnl_7d_sol AS NUMERIC)),
    actual_pnl_30d_sol TEXT
        CHECK (actual_pnl_30d_sol IS NULL OR actual_pnl_30d_sol = CAST(actual_pnl_30d_sol AS NUMERIC)),
    match_timestamp TIMESTAMP,
    days_to_match INTEGER CHECK (days_to_match IS NULL OR days_to_match >= 0),

    -- Status tracking
    status TEXT DEFAULT 'PENDING' CHECK (status IN ('PENDING', 'MATCHED', 'EXPIRED', 'INVALID')),

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

-- Keep updated_at fresh on rows whose status/PnL is backfilled after insert.
-- Fires only when the writer did not set updated_at itself (WHEN guard also
-- prevents recursive trigger firing).
CREATE TRIGGER IF NOT EXISTS trg_ml_predictions_updated_at
AFTER UPDATE ON ml_predictions
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
 AND NEW.updated_at IS NOT strftime('%Y-%m-%dT%H:%M:%f', 'now')
BEGIN
    UPDATE ml_predictions SET updated_at = strftime('%Y-%m-%dT%H:%M:%f', 'now') WHERE id = OLD.id;
END;

-- Status values:
--   PENDING - Prediction made, awaiting actual results
--   MATCHED - Actual PnL matched to prediction
--   EXPIRED - Prediction too old to match (configurable threshold, default 90 days)
--   INVALID - Prediction data invalid or wallet not found
