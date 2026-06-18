-- Migration: Add source attribution columns to trades table
-- Date: 2024-06-19
-- Description: Adds signal_source_id and signal_source_type columns to trades table
--              to enable proper attribution for non-wallet signal sources (Telegram, webhooks)

-- Add source attribution columns to trades table
-- These columns will be populated for non-wallet signals
-- For wallet signals, wallet_address continues to be used as before

ALTER TABLE trades ADD COLUMN signal_source_id INTEGER;
ALTER TABLE trades ADD COLUMN signal_source_type TEXT CHECK(signal_source_type IN ('WALLET', 'TELEGRAM', 'WEBHOOK'));

-- Create indexes for performance
CREATE INDEX idx_trades_source_id ON trades(signal_source_id) WHERE signal_source_id IS NOT NULL;
CREATE INDEX idx_trades_source_type ON trades(signal_source_type) WHERE signal_source_type IS NOT NULL;

-- Note: Foreign key constraint to signal_sources.id is not enforced at the database level
-- to allow flexibility in signal processing. Application logic ensures referential integrity.
