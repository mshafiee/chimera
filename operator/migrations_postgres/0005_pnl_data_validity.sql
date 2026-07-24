-- 0005_pnl_data_validity.sql
-- A3: Quarantine historical profitability records produced by the pre-fix
-- accounting model (impossible PnL artifacts, e.g. net returns near -1000%).
--
-- Rows that existed before this migration first runs are marked
-- pnl_data_valid = FALSE and excluded from ALL decision metrics and
-- dashboards. Rows are retained for audit; reconstruction is out of scope.
-- New rows default to pnl_data_valid = TRUE via the column default.
--
-- Idempotent: the quarantine UPDATE only touches rows still marked valid,
-- so re-running this script manually is a no-op.

ALTER TABLE trades
    ADD COLUMN IF NOT EXISTS pnl_data_valid BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS pnl_invalidated_at TIMESTAMPTZ;

ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS pnl_data_valid BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS pnl_invalidated_at TIMESTAMPTZ;

-- Quarantine every row that existed before the accounting fix deployed.
-- The pnl_invalidated_at timestamp doubles as the pnl_epoch marker.
UPDATE trades
SET pnl_data_valid = FALSE, pnl_invalidated_at = NOW()
WHERE pnl_data_valid = TRUE;

UPDATE positions
SET pnl_data_valid = FALSE, pnl_invalidated_at = NOW()
WHERE pnl_data_valid = TRUE;

-- Indexes to keep the filtered metric queries fast.
CREATE INDEX IF NOT EXISTS idx_trades_pnl_data_valid ON trades (pnl_data_valid);
CREATE INDEX IF NOT EXISTS idx_positions_pnl_data_valid ON positions (pnl_data_valid);
