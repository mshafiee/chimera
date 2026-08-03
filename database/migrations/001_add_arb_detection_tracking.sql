-- Migration: Add arb detection tracking to wallets table
-- This adds support for ARBITRAGE wallet classification and re-analysis cooldown

-- Add last_arb_check_at column to track when ARBITRAGE detection was last run
ALTER TABLE wallets ADD COLUMN IF NOT EXISTS last_arb_check_at TIMESTAMPTZ;

-- Add index for faster queries on arb-check timestamps (partial: only wallets that have been checked)
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_wallets_last_arb_check_at ON wallets(last_arb_check_at) WHERE last_arb_check_at IS NOT NULL;

-- Comment on the new column
COMMENT ON COLUMN wallets.last_arb_check_at IS 'Timestamp of last ARBITRAGE classification check - used for re-analysis cooldown (default 24 hours)';