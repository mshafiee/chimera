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
    roi_7d REAL,
    roi_30d REAL,
    trade_count_30d INTEGER,
    win_rate REAL,
    max_drawdown_30d REAL,
    avg_trade_size_sol REAL,
    avg_win_sol REAL,
    avg_loss_sol REAL,
    profit_factor REAL,
    realized_pnl_30d_sol REAL,
    last_trade_at TIMESTAMP,
    promoted_at TIMESTAMP,
    ttl_expires_at TIMESTAMP,  -- For temporary promotions
    notes TEXT,
    archetype TEXT,  -- TraderArchetype as string (SNIPER, SWING, SCALPER, INSIDER, WHALE)
    avg_entry_delay_seconds REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for wallets table
CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);


