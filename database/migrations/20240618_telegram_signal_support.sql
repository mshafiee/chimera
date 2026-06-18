-- Migration: Telegram Signal Support
-- Version: 20240618000000
-- Description: Add support for Telegram channels as virtual wallet signal sources

-- =============================================================================
-- WALLET TABLE EXTENSIONS
-- =============================================================================

-- Add wallet_type to distinguish on-chain from virtual wallets
ALTER TABLE wallets ADD COLUMN wallet_type TEXT DEFAULT 'ON_CHAIN'
CHECK(wallet_type IN ('ON_CHAIN', 'TELEGRAM', 'WEBHOOK'));

-- Add channel-specific tracking for telegram wallets
ALTER TABLE wallets ADD COLUMN channel_username TEXT;

-- Signal frequency tracking (signals per day)
ALTER TABLE wallets ADD COLUMN signal_frequency REAL DEFAULT 0.0;

-- Parse success rate for channels
ALTER TABLE wallets ADD COLUMN parse_success_rate REAL DEFAULT 0.0;

-- Create indexes for virtual wallet queries
CREATE INDEX IF NOT EXISTS idx_wallets_type ON wallets(wallet_type);
CREATE INDEX IF NOT EXISTS idx_wallets_channel ON wallets(channel_username) WHERE wallet_type = 'TELEGRAM';

-- =============================================================================
-- TRADES TABLE EXTENSIONS
-- =============================================================================

-- Track signal source in trades for attribution
ALTER TABLE trades ADD COLUMN signal_source TEXT DEFAULT 'WALLET';

-- Create index for signal source queries
CREATE INDEX IF NOT EXISTS idx_trades_source ON trades(signal_source);

-- =============================================================================
-- SCHEMA MIGATION TRACKING
-- =============================================================================

INSERT INTO schema_migrations (version)
VALUES ('20240618_telegram_signal_support')
ON CONFLICT(version) DO NOTHING;
