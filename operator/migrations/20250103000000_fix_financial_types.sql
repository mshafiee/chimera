-- Chimera v7.2: Convert all financial REAL columns to TEXT (Decimal strings)
-- This fixes precision loss inherent in IEEE 754 floating-point for monetary values.
--
-- Migration strategy per table:
--   1. DROP TABLE IF EXISTS _new (idempotency guard)
--   2. CREATE TABLE _new with TEXT columns for financial values
--   3. INSERT INTO _new SELECT ..., CAST(old AS TEXT), ... FROM old
--   4. DROP TABLE IF EXISTS old (idempotency guard)
--   5. ALTER TABLE _new RENAME TO old
--   6. Recreate all indexes and triggers
--
-- Columns that stay REAL (non-financial stats/scores):
--   wqs_score, wqs_confidence, win_rate, avg_entry_delay_seconds,
--   signal_success_rate, requests_per_second
--
-- Triggers removed: application-level timestamp management replaced
-- with explicit .set("updated_at", ...) in Rust query code.

-- =============================================================================
-- TRADES
-- =============================================================================
DROP TABLE IF EXISTS trades_new;
CREATE TABLE trades_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    strategy TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR', 'EXIT')),
    side TEXT NOT NULL CHECK(side IN ('BUY', 'SELL')),
    amount_sol TEXT NOT NULL,
    price_at_signal TEXT,
    tx_signature TEXT,
    status TEXT NOT NULL DEFAULT 'PENDING'
        CHECK(status IN ('PENDING', 'QUEUED', 'EXECUTING', 'ACTIVE', 'EXITING', 'CLOSED', 'FAILED', 'RETRY', 'DEAD_LETTER')),
    retry_count INTEGER DEFAULT 0,
    error_message TEXT,
    pnl_sol TEXT,
    pnl_usd TEXT,
    jito_tip_sol TEXT DEFAULT '0',
    dex_fee_sol TEXT DEFAULT '0',
    slippage_cost_sol TEXT DEFAULT '0',
    total_cost_sol TEXT DEFAULT '0',
    net_pnl_sol TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO trades_new
SELECT
    id, trade_uuid, wallet_address, token_address, token_symbol,
    strategy, side,
    CAST(amount_sol AS TEXT),
    CAST(price_at_signal AS TEXT),
    tx_signature, status, retry_count, error_message,
    CAST(pnl_sol AS TEXT),
    CAST(pnl_usd AS TEXT),
    CAST(jito_tip_sol AS TEXT),
    CAST(dex_fee_sol AS TEXT),
    CAST(slippage_cost_sol AS TEXT),
    CAST(total_cost_sol AS TEXT),
    CAST(net_pnl_sol AS TEXT),
    created_at, updated_at
FROM trades
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='trades');

DROP TABLE IF EXISTS trades;
ALTER TABLE trades_new RENAME TO trades;

CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
CREATE INDEX IF NOT EXISTS idx_trades_status_queued ON trades(status) WHERE status = 'QUEUED';
CREATE INDEX IF NOT EXISTS idx_trades_wallet ON trades(wallet_address);
CREATE INDEX IF NOT EXISTS idx_trades_token ON trades(token_address);
CREATE INDEX IF NOT EXISTS idx_trades_created ON trades(created_at DESC);

-- =============================================================================
-- POSITIONS
-- =============================================================================
DROP TABLE IF EXISTS positions_new;
CREATE TABLE positions_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    strategy TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR')),
    entry_amount_sol TEXT NOT NULL,
    entry_price TEXT NOT NULL,
    entry_tx_signature TEXT NOT NULL,
    current_price TEXT,
    unrealized_pnl_sol TEXT,
    unrealized_pnl_percent TEXT,
    state TEXT NOT NULL DEFAULT 'ACTIVE'
        CHECK(state IN ('ACTIVE', 'EXITING', 'CLOSED')),
    exit_price TEXT,
    exit_tx_signature TEXT,
    realized_pnl_sol TEXT,
    realized_pnl_usd TEXT,
    entry_sol_price_usd TEXT,
    opened_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    closed_at TIMESTAMP,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

INSERT INTO positions_new
SELECT
    id, trade_uuid, wallet_address, token_address, token_symbol,
    strategy,
    CAST(entry_amount_sol AS TEXT),
    CAST(entry_price AS TEXT),
    entry_tx_signature,
    CAST(current_price AS TEXT),
    CAST(unrealized_pnl_sol AS TEXT),
    CAST(unrealized_pnl_percent AS TEXT),
    state,
    CAST(exit_price AS TEXT),
    exit_tx_signature,
    CAST(realized_pnl_sol AS TEXT),
    CAST(realized_pnl_usd AS TEXT),
    CAST(entry_sol_price_usd AS TEXT),
    opened_at, last_updated, closed_at
FROM positions
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='positions');

DROP TABLE IF EXISTS positions;
ALTER TABLE positions_new RENAME TO positions;

CREATE INDEX IF NOT EXISTS idx_positions_state ON positions(state);
CREATE INDEX IF NOT EXISTS idx_positions_state_updated ON positions(state, last_updated);
CREATE INDEX IF NOT EXISTS idx_positions_wallet ON positions(wallet_address);
CREATE INDEX IF NOT EXISTS idx_positions_wallet_token ON positions(wallet_address, token_address);

-- =============================================================================
-- WALLETS
-- =============================================================================
-- Keeps wqs_score, wqs_confidence, win_rate, avg_entry_delay_seconds as REAL
DROP TABLE IF EXISTS wallets_new;
CREATE TABLE wallets_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'CANDIDATE'
        CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')),
    wqs_score REAL,
    wqs_confidence REAL,
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
    ttl_expires_at TIMESTAMP,
    notes TEXT,
    archetype TEXT,
    avg_entry_delay_seconds REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO wallets_new
SELECT
    id, address, status, wqs_score, wqs_confidence,
    CAST(roi_7d AS TEXT),
    CAST(roi_30d AS TEXT),
    trade_count_30d, win_rate,
    CAST(max_drawdown_30d AS TEXT),
    CAST(avg_trade_size_sol AS TEXT),
    CAST(avg_win_sol AS TEXT),
    CAST(avg_loss_sol AS TEXT),
    CAST(profit_factor AS TEXT),
    CAST(realized_pnl_30d_sol AS TEXT),
    last_trade_at, promoted_at, ttl_expires_at, notes, archetype,
    avg_entry_delay_seconds, created_at, updated_at
FROM wallets
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='wallets');

DROP TABLE IF EXISTS wallets;
ALTER TABLE wallets_new RENAME TO wallets;

CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);

-- =============================================================================
-- EXIT TARGETS
-- =============================================================================
DROP TABLE IF EXISTS exit_targets_new;
CREATE TABLE exit_targets_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL UNIQUE,
    entry_price TEXT NOT NULL,
    entry_amount_sol TEXT NOT NULL,
    profit_targets TEXT,
    targets_hit TEXT,
    trailing_stop_active INTEGER DEFAULT 0,
    trailing_stop_price TEXT,
    peak_price TEXT,
    peak_profit_percent TEXT,
    stop_loss_price TEXT,
    remaining_fraction TEXT NOT NULL DEFAULT '1.0',
    entry_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

INSERT INTO exit_targets_new
SELECT
    id, trade_uuid,
    CAST(entry_price AS TEXT),
    CAST(entry_amount_sol AS TEXT),
    profit_targets, targets_hit, trailing_stop_active,
    CAST(trailing_stop_price AS TEXT),
    CAST(peak_price AS TEXT),
    CAST(peak_profit_percent AS TEXT),
    CAST(stop_loss_price AS TEXT),
    CAST(remaining_fraction AS TEXT),
    entry_time, last_updated
FROM exit_targets
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='exit_targets');

DROP TABLE IF EXISTS exit_targets;
ALTER TABLE exit_targets_new RENAME TO exit_targets;

CREATE INDEX IF NOT EXISTS idx_exit_targets_trade ON exit_targets(trade_uuid);

-- =============================================================================
-- JITO TIP HISTORY
-- =============================================================================
DROP TABLE IF EXISTS jito_tip_history_new;
CREATE TABLE jito_tip_history_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tip_amount_sol TEXT NOT NULL,
    bundle_signature TEXT,
    strategy TEXT CHECK(strategy IN ('SHIELD', 'SPEAR')),
    success INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO jito_tip_history_new
SELECT
    id,
    CAST(tip_amount_sol AS TEXT),
    bundle_signature, strategy, success, created_at
FROM jito_tip_history
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='jito_tip_history');

DROP TABLE IF EXISTS jito_tip_history;
ALTER TABLE jito_tip_history_new RENAME TO jito_tip_history;

CREATE INDEX IF NOT EXISTS idx_jito_tip_created ON jito_tip_history(created_at DESC);

-- =============================================================================
-- RECONCILIATION LOG
-- =============================================================================
DROP TABLE IF EXISTS reconciliation_log_new;
CREATE TABLE reconciliation_log_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL,
    expected_state TEXT NOT NULL,
    actual_on_chain TEXT,
    discrepancy TEXT,
    on_chain_tx_signature TEXT,
    on_chain_amount_sol TEXT,
    expected_amount_sol TEXT,
    resolved_at TIMESTAMP,
    resolved_by TEXT,
    notes TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO reconciliation_log_new
SELECT
    id, trade_uuid, expected_state, actual_on_chain, discrepancy,
    on_chain_tx_signature,
    CAST(on_chain_amount_sol AS TEXT),
    CAST(expected_amount_sol AS TEXT),
    resolved_at, resolved_by, notes, created_at
FROM reconciliation_log
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='reconciliation_log');

DROP TABLE IF EXISTS reconciliation_log;
ALTER TABLE reconciliation_log_new RENAME TO reconciliation_log;

CREATE INDEX IF NOT EXISTS idx_reconciliation_unresolved
    ON reconciliation_log(resolved_at) WHERE resolved_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_reconciliation_trade ON reconciliation_log(trade_uuid);

-- =============================================================================
-- HISTORICAL LIQUIDITY
-- =============================================================================
DROP TABLE IF EXISTS historical_liquidity_new;
CREATE TABLE historical_liquidity_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    liquidity_usd TEXT NOT NULL,
    price_usd TEXT,
    volume_24h_usd TEXT,
    timestamp TIMESTAMP NOT NULL,
    source TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(token_address, timestamp)
);

INSERT INTO historical_liquidity_new
SELECT
    id, token_address,
    CAST(liquidity_usd AS TEXT),
    CAST(price_usd AS TEXT),
    CAST(volume_24h_usd AS TEXT),
    timestamp, source, created_at
FROM historical_liquidity
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='historical_liquidity');

DROP TABLE IF EXISTS historical_liquidity;
ALTER TABLE historical_liquidity_new RENAME TO historical_liquidity;

CREATE INDEX IF NOT EXISTS idx_historical_liquidity_token_time
    ON historical_liquidity(token_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_timestamp
    ON historical_liquidity(timestamp DESC);

-- =============================================================================
-- SIGNAL AGGREGATION
-- =============================================================================
DROP TABLE IF EXISTS signal_aggregation_new;
CREATE TABLE signal_aggregation_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_address TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
    amount_sol TEXT NOT NULL,
    signature TEXT,
    is_consensus INTEGER DEFAULT 0,
    consensus_wallet_count INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

INSERT INTO signal_aggregation_new
SELECT
    id, token_address, wallet_address, direction,
    CAST(amount_sol AS TEXT),
    signature, is_consensus, consensus_wallet_count, created_at
FROM signal_aggregation
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='signal_aggregation');

DROP TABLE IF EXISTS signal_aggregation;
ALTER TABLE signal_aggregation_new RENAME TO signal_aggregation;

CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_with_sig
    ON signal_aggregation(token_address, wallet_address, signature)
    WHERE signature IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_no_sig
    ON signal_aggregation(token_address, wallet_address, direction, created_at)
    WHERE signature IS NULL;
CREATE INDEX IF NOT EXISTS idx_signal_aggregation_token_time
    ON signal_aggregation(token_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_signal_aggregation_consensus
    ON signal_aggregation(is_consensus) WHERE is_consensus = 1;

-- =============================================================================
-- WALLET COPY PERFORMANCE
-- =============================================================================
DROP TABLE IF EXISTS wallet_copy_performance_new;
CREATE TABLE wallet_copy_performance_new (
    wallet_address TEXT PRIMARY KEY,
    copy_pnl_7d TEXT DEFAULT '0',
    copy_pnl_30d TEXT DEFAULT '0',
    signal_success_rate REAL DEFAULT 0.0,
    avg_return_per_trade TEXT DEFAULT '0',
    total_trades INTEGER DEFAULT 0,
    winning_trades INTEGER DEFAULT 0,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address)
);

INSERT INTO wallet_copy_performance_new
SELECT
    wallet_address,
    CAST(copy_pnl_7d AS TEXT),
    CAST(copy_pnl_30d AS TEXT),
    signal_success_rate,
    CAST(avg_return_per_trade AS TEXT),
    total_trades, winning_trades, last_updated
FROM wallet_copy_performance
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='wallet_copy_performance');

DROP TABLE IF EXISTS wallet_copy_performance;
ALTER TABLE wallet_copy_performance_new RENAME TO wallet_copy_performance;

-- =============================================================================
-- WQS PNL CORRELATION
-- =============================================================================
DROP TABLE IF EXISTS wqs_pnl_correlation_new;
CREATE TABLE wqs_pnl_correlation_new (
    wallet_address TEXT PRIMARY KEY,
    wqs_score_at_promotion REAL NOT NULL,
    actual_copy_pnl_7d_sol TEXT,
    actual_copy_pnl_30d_sol TEXT,
    actual_copy_pnl_all_sol TEXT,
    copy_trade_count_7d INTEGER DEFAULT 0,
    copy_trade_count_30d INTEGER DEFAULT 0,
    copy_trade_count_all INTEGER DEFAULT 0,
    strategy TEXT NOT NULL DEFAULT 'SHIELD'
        CHECK(strategy IN ('SHIELD', 'SPEAR')),
    wqs_components_json TEXT,
    promoted_at TEXT NOT NULL,
    last_updated_at TEXT NOT NULL
);

INSERT INTO wqs_pnl_correlation_new
SELECT
    wallet_address, wqs_score_at_promotion,
    CAST(actual_copy_pnl_7d_sol AS TEXT),
    CAST(actual_copy_pnl_30d_sol AS TEXT),
    CAST(actual_copy_pnl_all_sol AS TEXT),
    copy_trade_count_7d, copy_trade_count_30d, copy_trade_count_all,
    strategy, wqs_components_json, promoted_at, last_updated_at
FROM wqs_pnl_correlation
WHERE EXISTS (SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='wqs_pnl_correlation');

DROP TABLE IF EXISTS wqs_pnl_correlation;
ALTER TABLE wqs_pnl_correlation_new RENAME TO wqs_pnl_correlation;

-- =============================================================================
-- DROP TRIGGERS (replaced by application-level timestamp management)
-- =============================================================================
DROP TRIGGER IF EXISTS trades_updated_at;
DROP TRIGGER IF EXISTS wallets_updated_at;
DROP TRIGGER IF EXISTS positions_updated_at;
DROP TRIGGER IF EXISTS wallet_monitoring_updated_at;
DROP TRIGGER IF EXISTS exit_targets_updated_at;
