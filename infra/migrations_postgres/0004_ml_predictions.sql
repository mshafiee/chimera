-- Migration: Create ml_predictions table for Scout model validation
--
-- BACKGROUND: Scout's prediction_logger / prediction_matcher / validation_metrics
-- modules all read and write `ml_predictions`, but the table was only ever
-- defined in a SQLite-dialect schema file (database/schema/ml_predictions.sql)
-- that the scout container doesn't ship and whose path is computed wrong inside
-- the container (`parent.parent.parent` resolves to `/` instead of `/app`).
-- Result: `relation "ml_predictions" does not exist` on every scout run,
-- blocking prediction logging and match-driven wallet validation.
--
-- This migration creates the table server-side so scout works regardless of
-- whether the schema file is present in the container.
--
-- Financial PnL values are stored as NUMERIC (not TEXT like the SQLite schema)
-- for correct arithmetic in validation queries.

CREATE TABLE IF NOT EXISTS ml_predictions (
    id                              SERIAL PRIMARY KEY,

    -- Prediction identifiers
    wallet_address                  TEXT NOT NULL,
    prediction_timestamp            TIMESTAMP NOT NULL,
    model_type                      TEXT NOT NULL,

    -- Prediction values
    predicted_pnl_sol               NUMERIC(30,18) NOT NULL,
    predicted_class                 TEXT,
    confidence                      DOUBLE PRECISION,

    -- Feature context
    features_json                   TEXT,
    strategy                        TEXT,
    wqs_score_at_prediction         DOUBLE PRECISION,
    wqs_components_json             TEXT,

    -- Actual results (filled when matched)
    actual_pnl_sol                  NUMERIC(30,18),
    actual_pnl_7d_sol               NUMERIC(30,18),
    actual_pnl_30d_sol              NUMERIC(30,18),
    match_timestamp                 TIMESTAMP,
    days_to_match                   INTEGER,

    -- Status tracking
    status                          TEXT DEFAULT 'PENDING',

    -- Audit timestamps
    created_at                      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at                      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- Constraint: unique prediction per wallet/model/timestamp
    UNIQUE(wallet_address, prediction_timestamp, model_type)
);

-- Indexes for efficient queries
CREATE INDEX IF NOT EXISTS idx_ml_predictions_wallet     ON ml_predictions(wallet_address);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_status     ON ml_predictions(status);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_timestamp  ON ml_predictions(prediction_timestamp);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_model      ON ml_predictions(model_type);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_match_sts  ON ml_predictions(status, prediction_timestamp);
