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
    jito_tip_sol REAL DEFAULT 0,
    dex_fee_sol REAL DEFAULT 0,
    slippage_cost_sol REAL DEFAULT 0,
    total_cost_sol REAL DEFAULT 0,
    net_pnl_sol REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for trades table
CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
CREATE INDEX IF NOT EXISTS idx_trades_status_queued ON trades(status) WHERE status = 'QUEUED';
CREATE INDEX IF NOT EXISTS idx_trades_wallet ON trades(wallet_address);
CREATE INDEX IF NOT EXISTS idx_trades_token ON trades(token_address);
CREATE INDEX IF NOT EXISTS idx_trades_created ON trades(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_trades_costs ON trades(total_cost_sol) WHERE total_cost_sol > 0;
CREATE INDEX IF NOT EXISTS idx_trades_net_pnl ON trades(net_pnl_sol) WHERE net_pnl_sol IS NOT NULL;

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

-- Historical liquidity data for backtesting and validation
CREATE TABLE IF NOT EXISTS historical_liquidity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    liquidity_usd REAL NOT NULL,
    price_usd REAL,
    volume_24h_usd REAL,
    timestamp TIMESTAMP NOT NULL,
    source TEXT, -- 'birdeye', 'calculated', 'jupiter', etc.
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(token_address, timestamp)
);

-- Index for efficient historical queries
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_token_time 
    ON historical_liquidity(token_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_timestamp 
    ON historical_liquidity(timestamp DESC);

-- =============================================================================
-- MONITORING TABLES
-- =============================================================================

-- Wallet monitoring: Track webhook subscriptions and polling state
CREATE TABLE IF NOT EXISTS wallet_monitoring (
    wallet_address TEXT PRIMARY KEY,
    helius_webhook_id TEXT,
    rpc_polling_active INTEGER DEFAULT 0,
    last_transaction_signature TEXT,
    last_monitored_at TIMESTAMP,
    monitoring_enabled INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address)
);

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_enabled 
    ON wallet_monitoring(monitoring_enabled) WHERE monitoring_enabled = 1;

-- Exit targets: Position-level profit targets and stops
CREATE TABLE IF NOT EXISTS exit_targets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    entry_price REAL NOT NULL,
    entry_amount_sol REAL NOT NULL,
    profit_targets TEXT,  -- JSON array of target percentages
    targets_hit TEXT,     -- JSON array of hit targets
    trailing_stop_active INTEGER DEFAULT 0,
    trailing_stop_price REAL,
    peak_price REAL,
    peak_profit_percent REAL,
    stop_loss_price REAL,
    entry_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

CREATE INDEX IF NOT EXISTS idx_exit_targets_trade ON exit_targets(trade_uuid);

-- Signal aggregation: Multi-wallet signal tracking
CREATE TABLE IF NOT EXISTS signal_aggregation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
    amount_sol REAL NOT NULL,
    signature TEXT,
    is_consensus INTEGER DEFAULT 0,
    consensus_wallet_count INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(token_address, wallet_address, signature)
);

CREATE INDEX IF NOT EXISTS idx_signal_aggregation_token_time 
    ON signal_aggregation(token_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_signal_aggregation_consensus 
    ON signal_aggregation(is_consensus) WHERE is_consensus = 1;

-- Wallet copy performance: Per-wallet copy trading metrics
CREATE TABLE IF NOT EXISTS wallet_copy_performance (
    wallet_address TEXT PRIMARY KEY,
    copy_pnl_7d REAL DEFAULT 0.0,
    copy_pnl_30d REAL DEFAULT 0.0,
    signal_success_rate REAL DEFAULT 0.0,
    avg_return_per_trade REAL DEFAULT 0.0,
    total_trades INTEGER DEFAULT 0,
    winning_trades INTEGER DEFAULT 0,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address)
);

CREATE INDEX IF NOT EXISTS idx_wallet_copy_performance_pnl 
    ON wallet_copy_performance(copy_pnl_7d DESC);

-- Rate limit metrics: Credit usage and rate tracking
CREATE TABLE IF NOT EXISTS rate_limit_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    metric_type TEXT NOT NULL,  -- 'webhook', 'rpc', 'total'
    requests_per_second REAL,
    total_credits_used INTEGER,
    rate_limit_hits INTEGER DEFAULT 0,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_metrics_time 
    ON rate_limit_metrics(timestamp DESC);

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

-- Auto-update updated_at on wallet_monitoring
CREATE TRIGGER IF NOT EXISTS wallet_monitoring_updated_at
    AFTER UPDATE ON wallet_monitoring
BEGIN
    UPDATE wallet_monitoring SET updated_at = CURRENT_TIMESTAMP WHERE wallet_address = NEW.wallet_address;
END;

-- Auto-update last_updated on exit_targets
CREATE TRIGGER IF NOT EXISTS exit_targets_updated_at
    AFTER UPDATE ON exit_targets
BEGIN
    UPDATE exit_targets SET last_updated = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
