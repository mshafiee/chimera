-- Wallet schema definition (shared source of truth)
-- This file is used by both Rust (sqlx) and Python (RosterWriter)
-- to ensure schema consistency across languages.
-- Financial values stored as TEXT (Decimal strings), scores/stats as REAL.

-- Wallets table: Tracked wallets with WQS scores (managed by Scout)
CREATE TABLE IF NOT EXISTS wallets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'CANDIDATE'
        CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')),
    wqs_score REAL,
    wqs_confidence REAL,  -- Sample confidence 0-1, unbundled from wqs_score
    roi_7d TEXT,
    roi_30d TEXT,
    trade_count_30d INTEGER,
    win_rate REAL,
    max_drawdown_30d TEXT,
    avg_trade_size_sol TEXT,
    avg_win_sol TEXT,
    avg_loss_sol TEXT,
    profit_factor TEXT,
    realized_pnl_30d_sol TEXT,
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
