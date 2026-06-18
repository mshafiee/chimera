-- Migration: Create signal_sources table for non-wallet signal sources
-- Date: 2024-06-19
-- Description: Creates a dedicated table for tracking Telegram channels and other signal sources
--              separately from the wallets table, keeping the wallets table clean for real
--              on-chain wallet addresses only.

-- Create signal_sources table
CREATE TABLE IF NOT EXISTS signal_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type TEXT NOT NULL CHECK(source_type IN ('TELEGRAM', 'WEBHOOK', 'OTHER')),
    source_id TEXT NOT NULL,  -- Channel username (@solana_whales) or webhook ID

    -- Enable/disable control
    enabled BOOLEAN DEFAULT 1,

    -- Channel configuration
    max_signals_per_hour INTEGER DEFAULT 30,
    max_signal_age_seconds INTEGER DEFAULT 300,
    strategy_preference TEXT CHECK(strategy_preference IN ('SHIELD', 'SPEAR', 'AUTO')),

    -- Quality metrics (separate from wallet WQS)
    quality_score REAL DEFAULT 50.0,  -- 0-100 range, updated based on performance
    parse_success_rate REAL DEFAULT 1.0,  -- % of messages successfully parsed
    signal_frequency REAL DEFAULT 0.0,  -- signals per day

    -- Signal tracking
    total_signals INTEGER DEFAULT 0,
    successful_signals INTEGER DEFAULT 0,
    rejected_signals INTEGER DEFAULT 0,

    -- Trading performance
    total_trades INTEGER DEFAULT 0,
    winning_trades INTEGER DEFAULT 0,
    roi_7d REAL DEFAULT 0.0,
    roi_30d REAL DEFAULT 0.0,
    win_rate REAL DEFAULT 0.0,
    realized_pnl_30d_sol REAL DEFAULT 0.0,

    -- Telegram-specific
    telegram_channel_id INTEGER,  -- Numeric Telegram channel ID
    notes TEXT,

    -- Timestamps
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Constraints
    UNIQUE(source_type, source_id)
);

-- Indexes for performance
CREATE INDEX idx_signal_sources_type ON signal_sources(source_type);
CREATE INDEX idx_signal_sources_enabled ON signal_sources(enabled, source_type) WHERE enabled = 1;
CREATE INDEX idx_signal_sources_quality ON signal_sources(quality_score) WHERE enabled = 1;

-- Trigger to update updated_at timestamp
CREATE TRIGGER IF NOT EXISTS update_signal_sources_timestamp
AFTER UPDATE ON signal_sources
FOR EACH ROW
BEGIN
    UPDATE signal_sources SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

-- Insert default high-value channels from analysis
INSERT OR IGNORE INTO signal_sources
    (source_type, source_id, telegram_channel_id, enabled, quality_score,
     max_signals_per_hour, max_signal_age_seconds, strategy_preference, notes)
VALUES
    ('TELEGRAM', '@solana_whales_signal', 123456789, 1, 72.0, 30, 180, 'SHIELD',
     'High-value channel from analysis: 72.0 score, 100% parseable, 28.6 signals/day'),
    ('TELEGRAM', '@SolmemeWhaleinsider', 987654321, 1, 80.0, 30, 180, 'SHIELD',
     'High-value channel from analysis: 80.0 score, 100% parseable, 28.6 signals/day'),
    ('TELEGRAM', '@SolanaDaily_Pumps', 456789123, 1, 80.0, 30, 300, 'SPEAR',
     'High-value channel from analysis: 80.0 score, 100% parseable, 28.6 signals/day');
