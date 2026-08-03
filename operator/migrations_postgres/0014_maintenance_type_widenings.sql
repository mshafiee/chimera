-- 0014_maintenance_type_widenings.sql
-- Heavy table-rewrite DDL split out of 0013_post_review_fixes.sql.
--
-- MAINTENANCE WINDOW REQUIRED: every statement below acquires an ACCESS
-- EXCLUSIVE lock and REWRITES the table. Postgres cannot do ALTER COLUMN TYPE
-- in place or CONCURRENTLY, and sqlx wraps each migration file in a single
-- transaction (so CREATE INDEX CONCURRENTLY is not usable here either). Apply
-- this file during a low-traffic maintenance slot. sqlx still wraps the file
-- in one transaction, so a mid-way failure rolls everything back; for very
-- large tables, consider staging these by hand outside sqlx.
--
-- Carried as a NEW migration (not yet applied); safe to edit pre-apply.

-- =============================================================================
-- 1. positions.token_amount: widen to cover raw token units
-- =============================================================================
-- token_amount stores RAW token units: (entry_sol / fill_price) * 10^decimals
-- (executor.rs derive_token_amount, u64). NUMERIC(30,18) caps the integer part
-- at 12 digits (~1e12), which overflows for high-supply/high-decimal tokens
-- (e.g. 1e6 tokens at 9 decimals = 1e15 raw units). NUMERIC(38,18) leaves 20
-- integer digits, covering the full u64 range.
ALTER TABLE positions
    ALTER COLUMN token_amount TYPE NUMERIC(38,18) USING token_amount::numeric(38,18);

-- =============================================================================
-- 2. ml_predictions: TIMESTAMP -> TIMESTAMPTZ + updated_at trigger
-- =============================================================================
-- prediction_timestamp / match_timestamp were TIMESTAMP (naive) while
-- created_at / updated_at are TIMESTAMPTZ; scout writes datetime.utcnow()
-- (naive UTC) into them and reads back with fromisoformat treating them as
-- UTC. Postgres stores naive TIMESTAMP in the session timezone, so any
-- server/container TZ skew corrupts days_to_match and time comparisons.
-- Converting with `AT TIME ZONE 'UTC'` preserves the existing values exactly
-- (they were written as UTC) and makes the column types consistent.
ALTER TABLE ml_predictions
    ALTER COLUMN prediction_timestamp TYPE TIMESTAMPTZ
    USING prediction_timestamp AT TIME ZONE 'UTC';
ALTER TABLE ml_predictions
    ALTER COLUMN match_timestamp TYPE TIMESTAMPTZ
    USING match_timestamp AT TIME ZONE 'UTC';

-- Keep updated_at fresh on UPDATE like every other table.
DROP TRIGGER IF EXISTS ml_predictions_updated_at ON ml_predictions;
CREATE TRIGGER ml_predictions_updated_at
    BEFORE UPDATE ON ml_predictions
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
