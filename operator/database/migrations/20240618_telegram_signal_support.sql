-- Migration: Telegram Signal Support
-- Date: 2024-06-18
-- Description: Add support for Telegram channels as virtual wallet signal sources

-- Add wallet type to distinguish on-chain from virtual
ALTER TABLE wallets ADD COLUMN wallet_type TEXT DEFAULT 'ON_CHAIN';

-- Channel-specific tracking for telegram wallets
ALTER TABLE wallets ADD COLUMN channel_username TEXT;
ALTER TABLE wallets ADD COLUMN signal_frequency REAL DEFAULT 0.0;
ALTER TABLE wallets ADD COLUMN parse_success_rate REAL DEFAULT 1.0;

-- Indexes for virtual wallet queries
CREATE INDEX IF NOT EXISTS idx_wallets_type ON wallets(wallet_type);
CREATE INDEX IF NOT EXISTS idx_wallets_channel ON wallets(channel_username) WHERE wallet_type = 'TELEGRAM';

-- Track signal source in trades for attribution
ALTER TABLE trades ADD COLUMN signal_source TEXT DEFAULT 'WALLET';
CREATE INDEX IF NOT EXISTS idx_trades_source ON trades(signal_source);
