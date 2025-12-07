-- Chimera v7.1 Database Schema
-- High-frequency copy-trading system for Solana

-- Enable WAL mode for concurrent reads during writes
PRAGMA journal_mode = WAL;
-- Set busy timeout to 5 seconds to handle momentary locks
PRAGMA busy_timeout = 5000;
-- Enable foreign key constraints
PRAGMA foreign_keys = ON;

-- =============================================================================
-- CORE TRADING TABLES
-- =============================================================================

-- Trades table: Primary record of all trading signals received
CREATE TABLE IF NOT EXISTS trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    strategy TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR', 'EXIT')),
    side TEXT NOT NULL CHECK(side IN ('BUY', 'SELL')),
    amount_sol REAL NOT NULL,
    price_at_signal REAL,
    tx_signature TEXT,
    status TEXT NOT NULL DEFAULT 'PENDING' 
        CHECK(status IN ('PENDING', 'QUEUED', 'EXECUTING', 'ACTIVE', 'EXITING', 'CLOSED', 'FAILED', 'RETRY', 'DEAD_LETTER')),
    retry_count INTEGER DEFAULT 0,
    error_message TEXT,
    pnl_sol REAL,
    pnl_usd REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for trades table
CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
CREATE INDEX IF NOT EXISTS idx_trades_status_queued ON trades(status) WHERE status = 'QUEUED';
CREATE INDEX IF NOT EXISTS idx_trades_wallet ON trades(wallet_address);
CREATE INDEX IF NOT EXISTS idx_trades_token ON trades(token_address);
CREATE INDEX IF NOT EXISTS idx_trades_created ON trades(created_at DESC);

-- Positions table: Active positions being tracked
CREATE TABLE IF NOT EXISTS positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    strategy TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR')),
    entry_amount_sol REAL NOT NULL,
    entry_price REAL NOT NULL,
    entry_tx_signature TEXT NOT NULL,
    current_price REAL,
    unrealized_pnl_sol REAL,
    unrealized_pnl_percent REAL,
    state TEXT NOT NULL DEFAULT 'ACTIVE'
        CHECK(state IN ('ACTIVE', 'EXITING', 'CLOSED')),
    exit_price REAL,
    exit_tx_signature TEXT,
    realized_pnl_sol REAL,
    realized_pnl_usd REAL,
    opened_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    closed_at TIMESTAMP,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

-- Indexes for positions table
CREATE INDEX IF NOT EXISTS idx_positions_state ON positions(state);
CREATE INDEX IF NOT EXISTS idx_positions_state_updated ON positions(state, last_updated);
CREATE INDEX IF NOT EXISTS idx_positions_wallet ON positions(wallet_address);

-- =============================================================================
-- WALLET MANAGEMENT TABLES
-- =============================================================================

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
    last_trade_at TIMESTAMP,
    promoted_at TIMESTAMP,
    ttl_expires_at TIMESTAMP,  -- For temporary promotions
    notes TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);

-- =============================================================================
-- SYSTEM TABLES
-- =============================================================================

-- Dead Letter Queue: Failed operations for analysis/retry
CREATE TABLE IF NOT EXISTS dead_letter_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT,
    payload TEXT NOT NULL,
    reason TEXT NOT NULL,  -- 'QUEUE_FULL', 'PARSE_ERROR', 'VALIDATION_FAILED', 'MAX_RETRIES'
    error_details TEXT,
    source_ip TEXT,
    retry_count INTEGER DEFAULT 0,
    can_retry INTEGER DEFAULT 1,  -- Boolean: 1 = can retry, 0 = permanent failure
    received_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_dlq_reason ON dead_letter_queue(reason);
CREATE INDEX IF NOT EXISTS idx_dlq_received ON dead_letter_queue(received_at DESC);

-- Config Audit: Track all configuration changes
CREATE TABLE IF NOT EXISTS config_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    changed_by TEXT NOT NULL,  -- 'ADMIN', 'SYSTEM_CIRCUIT_BREAKER', 'API', etc.
    change_reason TEXT,
    changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_config_audit_key ON config_audit(key);
CREATE INDEX IF NOT EXISTS idx_config_audit_changed ON config_audit(changed_at DESC);

-- Admin Wallets: Authorization for API access
CREATE TABLE IF NOT EXISTS admin_wallets (
    wallet_address TEXT PRIMARY KEY,
    role TEXT NOT NULL DEFAULT 'readonly'
        CHECK(role IN ('admin', 'operator', 'readonly')),
    added_by TEXT NOT NULL,
    notes TEXT,
    added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- JITO & EXECUTION TABLES
-- =============================================================================

-- Jito Tip History: For dynamic tip calculation (cold start persistence)
CREATE TABLE IF NOT EXISTS jito_tip_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tip_amount_sol REAL NOT NULL,
    bundle_signature TEXT,
    strategy TEXT CHECK(strategy IN ('SHIELD', 'SPEAR')),
    success INTEGER DEFAULT 1,  -- Boolean: 1 = bundle landed, 0 = failed
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_jito_tip_created ON jito_tip_history(created_at DESC);

-- =============================================================================
-- RECONCILIATION & COMPLIANCE
-- =============================================================================

-- Reconciliation Log: Compare DB state vs on-chain state
CREATE TABLE IF NOT EXISTS reconciliation_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL,
    expected_state TEXT NOT NULL,
    actual_on_chain TEXT,  -- 'FOUND', 'MISSING', 'AMOUNT_MISMATCH'
    discrepancy TEXT,  -- 'NONE', 'MISSING_TX', 'AMOUNT_MISMATCH', 'STATE_MISMATCH'
    on_chain_tx_signature TEXT,
    on_chain_amount_sol REAL,
    expected_amount_sol REAL,
    resolved_at TIMESTAMP,
    resolved_by TEXT,  -- 'AUTO', 'ADMIN', 'SYSTEM'
    notes TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_reconciliation_unresolved 
    ON reconciliation_log(resolved_at) WHERE resolved_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_reconciliation_trade ON reconciliation_log(trade_uuid);

-- Backups Tracking
CREATE TABLE IF NOT EXISTS backups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    size_bytes INTEGER,
    checksum TEXT,
    backup_type TEXT DEFAULT 'SCHEDULED',  -- 'SCHEDULED', 'MANUAL', 'PRE_MIGRATION'
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- TRIGGERS
-- =============================================================================

-- Auto-update updated_at on trades
CREATE TRIGGER IF NOT EXISTS trades_updated_at 
    AFTER UPDATE ON trades
BEGIN
    UPDATE trades SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

-- Auto-update updated_at on wallets
CREATE TRIGGER IF NOT EXISTS wallets_updated_at
    AFTER UPDATE ON wallets
BEGIN
    UPDATE wallets SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;

-- Auto-update last_updated on positions
CREATE TRIGGER IF NOT EXISTS positions_updated_at
    AFTER UPDATE ON positions
BEGIN
    UPDATE positions SET last_updated = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
