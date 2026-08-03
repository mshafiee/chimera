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
    wqs_confidence REAL CHECK (wqs_confidence IS NULL OR (wqs_confidence >= 0 AND wqs_confidence <= 1)),  -- Sample confidence 0-1, unbundled from wqs_score
    roi_7d TEXT CHECK (roi_7d IS NULL OR roi_7d = CAST(roi_7d AS NUMERIC)),
    roi_30d TEXT CHECK (roi_30d IS NULL OR roi_30d = CAST(roi_30d AS NUMERIC)),
    trade_count_30d INTEGER CHECK (trade_count_30d IS NULL OR trade_count_30d >= 0),
    win_rate REAL CHECK (win_rate IS NULL OR (win_rate >= 0 AND win_rate <= 1)),
    max_drawdown_30d TEXT CHECK (max_drawdown_30d IS NULL OR max_drawdown_30d = CAST(max_drawdown_30d AS NUMERIC)),
    avg_trade_size_sol TEXT CHECK (avg_trade_size_sol IS NULL OR avg_trade_size_sol = CAST(avg_trade_size_sol AS NUMERIC)),
    avg_win_sol TEXT CHECK (avg_win_sol IS NULL OR avg_win_sol = CAST(avg_win_sol AS NUMERIC)),
    avg_loss_sol TEXT CHECK (avg_loss_sol IS NULL OR avg_loss_sol = CAST(avg_loss_sol AS NUMERIC)),
    profit_factor TEXT CHECK (profit_factor IS NULL OR profit_factor = CAST(profit_factor AS NUMERIC)),
    realized_pnl_30d_sol TEXT CHECK (realized_pnl_30d_sol IS NULL OR realized_pnl_30d_sol = CAST(realized_pnl_30d_sol AS NUMERIC)),
    last_trade_at TIMESTAMP,
    promoted_at TIMESTAMP,
    ttl_expires_at TIMESTAMP,  -- For temporary promotions
    notes TEXT,
    archetype TEXT CHECK (archetype IS NULL OR archetype IN ('SNIPER', 'SWING', 'SCALPER', 'INSIDER', 'WHALE')),  -- TraderArchetype as string
    avg_entry_delay_seconds REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for wallets table
CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);
CREATE INDEX IF NOT EXISTS idx_wallets_ttl_expires ON wallets(ttl_expires_at);

-- Keep updated_at fresh on rows updated outside the app writers (e.g. shell
-- scripts). Fires only when the writer did not set updated_at itself (WHEN
-- guard also prevents recursive trigger firing).
CREATE TRIGGER IF NOT EXISTS trg_wallets_updated_at
AFTER UPDATE ON wallets
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
 AND NEW.updated_at IS NOT strftime('%Y-%m-%dT%H:%M:%f', 'now')
BEGIN
    UPDATE wallets SET updated_at = strftime('%Y-%m-%dT%H:%M:%f', 'now') WHERE id = NEW.id;
END;
