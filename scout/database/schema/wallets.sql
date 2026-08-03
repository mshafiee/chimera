-- Wallet schema definition (shared source of truth)
-- This file is used by both Rust (sqlx) and Python (RosterWriter)
-- to ensure schema consistency across languages.

-- Wallets table: Tracked wallets with WQS scores (managed by Scout)
CREATE TABLE IF NOT EXISTS wallets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'CANDIDATE'
        CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')),
    wqs_score REAL,
    wqs_confidence REAL CHECK(wqs_confidence IS NULL OR wqs_confidence BETWEEN 0 AND 1),
    roi_7d REAL,
    roi_30d REAL,
    trade_count_30d INTEGER CHECK(trade_count_30d IS NULL OR trade_count_30d >= 0),
    win_rate REAL CHECK(win_rate IS NULL OR win_rate BETWEEN 0 AND 1),
    max_drawdown_30d REAL CHECK(max_drawdown_30d IS NULL OR max_drawdown_30d <= 0),
    avg_trade_size_sol REAL,
    avg_win_sol REAL,
    avg_loss_sol REAL,
    profit_factor REAL,
    realized_pnl_30d_sol REAL,
    last_trade_at TIMESTAMP,
    promoted_at TIMESTAMP,
    ttl_expires_at TIMESTAMP,  -- For temporary promotions
    notes TEXT,
    archetype TEXT CHECK(archetype IS NULL OR archetype IN ('SNIPER', 'SWING', 'SCALPER', 'INSIDER', 'WHALE')),
    avg_entry_delay_seconds REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for wallets table
CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);
CREATE INDEX IF NOT EXISTS idx_wallets_ttl_expires_at ON wallets(ttl_expires_at);

-- Keep updated_at fresh on every UPDATE
CREATE TRIGGER IF NOT EXISTS trg_wallets_touch
    AFTER UPDATE ON wallets
    FOR EACH ROW
    BEGIN
        UPDATE wallets SET updated_at = CURRENT_TIMESTAMP WHERE id = OLD.id;
    END;
