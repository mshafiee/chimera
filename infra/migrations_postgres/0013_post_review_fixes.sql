-- 0013_post_review_fixes.sql
-- Post-scan schema hardening.
--
-- Carried as a NEW migration instead of editing already-applied files:
-- sqlx verifies the checksum of every applied migration (_sqlx_migrations) and
-- errors with VersionMismatch on startup if a previously-applied file changed.
--
-- Every constraint below was verified against the current application write
-- paths so it can never reject a legitimate production write.

-- =============================================================================
-- 1. circuit_breaker_state: canonical 'Active' casing + singleton guard
-- =============================================================================
-- 0002 seeded 'ACTIVE' (all caps), but restore_from_db compares state_str !=
-- "Active" case-sensitively, so a re-inserted row would be treated as a
-- tripped breaker and trigger a spurious re-evaluation. ON CONFLICT DO NOTHING
-- is intentional: a legitimately tripped record must never be clobbered.
INSERT INTO circuit_breaker_state (id, state, updated_at)
VALUES (1, 'Active', NOW())
ON CONFLICT (id) DO NOTHING;

-- Mirror kill_switch_state's CHECK (id = 1): the startup restore reads exactly
-- one row; a stray second row would be read as an unexpected state.
ALTER TABLE circuit_breaker_state
    ADD CONSTRAINT chk_circuit_breaker_state_singleton CHECK (id = 1);

-- =============================================================================
-- 2. exit_targets.remaining_fraction: partial-exit sizing in (0, 1]
-- =============================================================================
-- The engine (profit_targets) only ever writes fractions in (0, 1]:
-- remaining_fraction *= (1 - tiered_exit_percent/100)^k with 0 < percent < 100.
-- A value of 0 or > 1 would silently corrupt partial-exit position sizing.
ALTER TABLE exit_targets
    ADD CONSTRAINT chk_exit_targets_remaining_fraction
    CHECK (remaining_fraction > 0 AND remaining_fraction <= 1);

-- =============================================================================
-- 3. toxic_wallets: counter + toxicity-consistency guards
-- =============================================================================
-- Counters are loaded as u32 and used in demotion arithmetic
-- (local_top_entries * 2 >= total_entries); a negative value written by any
-- path would corrupt that logic. local_top_entries is always a subset of
-- total_entries (toxic.rs increments total first, local_top conditionally).
-- Every toxic row records when it was detected (toxic.rs sets detected_at
-- together with is_toxic).
ALTER TABLE toxic_wallets
    ADD CONSTRAINT chk_toxic_wallets_counters
    CHECK (local_top_entries >= 0 AND total_entries >= 0
           AND total_entries >= local_top_entries);
ALTER TABLE toxic_wallets
    ADD CONSTRAINT chk_toxic_wallets_toxic_state
    CHECK (NOT is_toxic OR detected_at IS NOT NULL);

-- Keep updated_at fresh on UPDATE like every other table in 0001.
CREATE TRIGGER toxic_wallets_updated_at
    BEFORE UPDATE ON toxic_wallets
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- =============================================================================
-- 4. promotion_episodes: append-only audit trail invariants
-- =============================================================================
-- No application code writes to this table yet; it is an audit trail by
-- design (0009 header), so block rewrite/erase at the database level and
-- constrain decision to the documented values.
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

ALTER TABLE promotion_episodes
    ADD CONSTRAINT chk_promotion_episodes_decision
    CHECK (decision IN ('promoted', 'shadow'));

CREATE OR REPLACE FUNCTION block_promotion_episodes_modification()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'promotion_episodes is an append-only audit trail; UPDATE/DELETE is not allowed';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS promotion_episodes_no_modification ON promotion_episodes;
CREATE TRIGGER promotion_episodes_no_modification
    BEFORE UPDATE OR DELETE ON promotion_episodes
    FOR EACH ROW
    EXECUTE FUNCTION block_promotion_episodes_modification();

-- =============================================================================
-- 5. decision_records: enum-like value guards
-- =============================================================================
-- Verified against decision_recorder.rs: ingress is 'webhook' | 'helius',
-- action 'BUY' | 'SELL', strategy 'SHIELD' | 'SPEAR' | 'EXIT' (or NULL).
-- NOTE: no CHECK ties admitted to trade_uuid/quote_json: those columns are
-- filled asynchronously after insert (link_trade/update_quote are
-- fire-and-forget), so rejected AND admitted rows can transiently carry NULLs.
ALTER TABLE decision_records
    ADD CONSTRAINT chk_decision_records_ingress
    CHECK (ingress IN ('webhook', 'helius'));
ALTER TABLE decision_records
    ADD CONSTRAINT chk_decision_records_action
    CHECK (action IN ('BUY', 'SELL'));
ALTER TABLE decision_records
    ADD CONSTRAINT chk_decision_records_strategy
    CHECK (strategy IS NULL OR strategy IN ('SHIELD', 'SPEAR', 'EXIT'));

-- =============================================================================
-- 6. trades: admit CANCELLED + guarded stale seed-trade cleanup
-- =============================================================================
-- 0011 sets status = 'CANCELLED', but the status CHECK (0001/0006) does not
-- include it, so that UPDATE aborts with a constraint violation whenever it
-- matches a row. Extend the CHECK, then re-run a guarded version of the
-- cleanup (backup first; only PENDING seed-init trades older than 30 days with
-- no ACTIVE position reference; fail loudly otherwise).

CREATE TABLE IF NOT EXISTS stale_seed_trade_backup AS
SELECT trade_uuid, wallet_address, token_address, amount_sol, created_at
FROM trades
WHERE status = 'PENDING'
  AND tx_signature = 'seed-trade-init';

ALTER TABLE trades DROP CONSTRAINT IF EXISTS trades_status_check;
ALTER TABLE trades ADD CONSTRAINT trades_status_check
    CHECK (status IN (
        'PENDING',
        'QUEUED',
        'EXECUTING',
        'ACTIVE',
        'EXITING',
        'CLOSED',
        'FAILED',
        'RETRY',
        'DEAD_LETTER',
        'REJECTED',
        'PENDING_CONFIRMATION',
        'CANCELLED'
    ));

DO $$
DECLARE
    active_refs INTEGER;
BEGIN
    SELECT COUNT(*) INTO active_refs
    FROM positions p
    JOIN trades t ON t.trade_uuid = p.trade_uuid
    WHERE p.state = 'ACTIVE'
      AND t.tx_signature = 'seed-trade-init';
    IF active_refs > 0 THEN
        RAISE EXCEPTION 'Cannot cancel seed-trade-init trades: % ACTIVE position(s) reference them', active_refs;
    END IF;
END $$;

UPDATE trades
SET status = 'CANCELLED',
    updated_at = NOW()
WHERE status = 'PENDING'
  AND tx_signature = 'seed-trade-init'
  AND created_at < NOW() - INTERVAL '30 days'
  AND NOT EXISTS (
      SELECT 1 FROM positions
      WHERE positions.trade_uuid = trades.trade_uuid
        AND positions.state = 'ACTIVE'
  );

-- =============================================================================
-- 7. ml_predictions: status CHECK + drop redundant status index
-- =============================================================================
-- Scout's prediction_logger only ever writes 'PENDING' / 'MATCHED' / 'EXPIRED'
-- (verified in scout/core/prediction_logger.py). idx_ml_predictions_status is
-- a leftmost prefix of idx_ml_predictions_match_sts(status, prediction_timestamp),
-- so status-only queries are already served by the composite index.
ALTER TABLE ml_predictions
    ADD CONSTRAINT chk_ml_predictions_status
    CHECK (status IN ('PENDING', 'MATCHED', 'EXPIRED'));

DROP INDEX IF EXISTS idx_ml_predictions_status;

-- NOTE: positions.token_amount widening (NUMERIC(30,18) -> NUMERIC(38,18)) and
-- the ml_predictions TIMESTAMP -> TIMESTAMPTZ conversions (formerly sections
-- 8 and 10) are heavy table-rewrite ALTER COLUMN TYPE statements that hold an
-- ACCESS EXCLUSIVE lock for the full rewrite duration and cannot be made
-- CONCURRENTLY. They were split into 0014_maintenance_type_widenings.sql so
-- this migration can apply without that lock window; apply 0014 during a
-- low-traffic maintenance slot. See 0014 for full rationale.

-- =============================================================================
-- 9. trades.closed_at: close time of the exit (SELL) trade
-- =============================================================================
-- The profitability outcome query (handlers/profitability.rs fetch_outcomes)
-- reads trades.closed_at as the point at which PnL was realized; the column
-- never existed (closed_at lives on positions). Add it so the query runs.
-- Callers that need it must set it explicitly; the drawdown ordering falls
-- back to decided_at when it is NULL.
ALTER TABLE trades ADD COLUMN IF NOT EXISTS closed_at TIMESTAMPTZ;

-- =============================================================================
-- 11. wallet_copy_performance.signal_success_rate: range guard
-- =============================================================================
-- wallet_performance.rs stores signal_success_rate as a PERCENTAGE
-- (winning_trades / total_trades * 100.0) and compares it against 50/60/70,
-- so the valid domain is [0, 100] — NOT [0, 1]. NUMERIC(10,6) precision came
-- from 0003; this pins the range at the database level.
ALTER TABLE wallet_copy_performance
    ADD CONSTRAINT chk_wallet_copy_signal_success_rate
    CHECK (signal_success_rate >= 0 AND signal_success_rate <= 100);

-- =============================================================================
-- 12. pnl_data_valid: partial indexes (0005 hardening)
-- =============================================================================
-- The metric/dashboard queries filter on pnl_data_valid = TRUE (the sparse
-- subset after the quarantine), so replace the low-selectivity plain btree
-- with a partial index that is far smaller and actually used.
-- NOTE: no CHECK tying pnl_data_valid to pnl_invalidated_at: rows can be
-- flagged invalid (pnl_data_valid = FALSE) without a quarantine timestamp
-- (e.g. hand-flagged corrupted rows), so the strict equality invariant would
-- reject legitimate writes.
DROP INDEX IF EXISTS idx_trades_pnl_data_valid;
CREATE INDEX idx_trades_pnl_data_valid ON trades (pnl_data_valid) WHERE pnl_data_valid;
DROP INDEX IF EXISTS idx_positions_pnl_data_valid;
CREATE INDEX idx_positions_pnl_data_valid ON positions (pnl_data_valid) WHERE pnl_data_valid;

-- =============================================================================
-- 13. toxic_wallets: run-scoped partial index
-- =============================================================================
-- Persistence is keyed by (wallet_address, run_id) and run-based monitoring
-- queries filter on is_toxic = TRUE, so add a composite partial index serving
-- `WHERE is_toxic AND run_id = $1` (and detected_at windows) instead of the
-- bare boolean index.
DROP INDEX IF EXISTS idx_toxic_wallets_is_toxic;
CREATE INDEX idx_toxic_wallets_is_toxic
    ON toxic_wallets (run_id, detected_at)
    WHERE is_toxic;
