-- Chimera v7.1 Database Schema - PostgreSQL
-- High-frequency copy-trading system for Solana
--
-- This is the PostgreSQL equivalent of schema.sql
-- Key differences from SQLite:
-- - INTEGER PRIMARY KEY AUTOINCREMENT → SERIAL/BIGSERIAL
-- - TIMESTAMP → TIMESTAMPTZ (timezone-aware)
-- - REAL → DOUBLE PRECISION (for financial calculations)
-- - REAL → NUMERIC(30,6) for monetary values (higher precision)
-- - Triggers use PostgreSQL syntax
-- - INSERT OR IGNORE → ON CONFLICT DO NOTHING

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";  -- For text similarity searches

-- Schema migration tracking (idempotent guard for migration files)
CREATE TABLE IF NOT EXISTS schema_migrations (
    version        TEXT PRIMARY KEY,
    applied_at    TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- CORE TRADING TABLES
-- =============================================================================

-- Trades table: Primary record of all trading signals received
CREATE TABLE IF NOT EXISTS trades (
    id                      BIGSERIAL PRIMARY KEY,
    trade_uuid             TEXT NOT NULL UNIQUE,
    wallet_address          TEXT NOT NULL,
    token_address           TEXT NOT NULL,
    token_symbol            TEXT,
    strategy                TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR', 'EXIT')),
    side                    TEXT NOT NULL CHECK(side IN ('BUY', 'SELL')),
    amount_sol              NUMERIC(30,6) NOT NULL,
    price_at_signal         NUMERIC(30,6),
    tx_signature            TEXT,
    status                  TEXT NOT NULL DEFAULT 'PENDING'
        CHECK(status IN ('PENDING', 'QUEUED', 'EXECUTING', 'ACTIVE', 'EXITING', 'CLOSED', 'FAILED', 'RETRY', 'DEAD_LETTER')),
    retry_count             INTEGER DEFAULT 0,
    error_message           TEXT,
    pnl_sol                 NUMERIC(30,6),
    pnl_usd                 NUMERIC(30,6),
    jito_tip_sol            NUMERIC(30,6) DEFAULT 0,
    dex_fee_sol             NUMERIC(30,6) DEFAULT 0,
    slippage_cost_sol       NUMERIC(30,6) DEFAULT 0,
    total_cost_sol          NUMERIC(30,6) DEFAULT 0,
    net_pnl_sol             NUMERIC(30,6),
    created_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for trades table
CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
CREATE INDEX IF NOT EXISTS idx_trades_status_queued ON trades(status) WHERE status = 'QUEUED';
CREATE INDEX IF NOT EXISTS idx_trades_wallet ON trades(wallet_address);
CREATE INDEX IF NOT EXISTS idx_trades_token ON trades(token_address);
CREATE INDEX IF NOT EXISTS idx_trades_created ON trades(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_trades_costs ON trades(total_cost_sol) WHERE total_cost_sol > 0;
CREATE INDEX IF NOT EXISTS idx_trades_net_pnl ON trades(net_pnl_sol) WHERE net_pnl_sol IS NOT NULL;

-- BRIN index for time-series data (trades table) - efficient for large datasets
CREATE INDEX IF NOT EXISTS idx_trades_created_brin ON trades USING BRIN(created_at);

-- Positions table: Active positions being tracked
CREATE TABLE IF NOT EXISTS positions (
    id                      BIGSERIAL PRIMARY KEY,
    trade_uuid             TEXT NOT NULL UNIQUE,
    wallet_address          TEXT NOT NULL,
    token_address           TEXT NOT NULL,
    token_symbol            TEXT,
    strategy                TEXT NOT NULL CHECK(strategy IN ('SHIELD', 'SPEAR')),
    entry_amount_sol        NUMERIC(30,6) NOT NULL,
    entry_price             NUMERIC(30,6) NOT NULL,
    entry_tx_signature      TEXT NOT NULL,
    current_price           NUMERIC(30,6),
    unrealized_pnl_sol      NUMERIC(30,6),
    unrealized_pnl_percent  NUMERIC(10,4),
    state                   TEXT NOT NULL DEFAULT 'ACTIVE'
        CHECK(state IN ('ACTIVE', 'EXITING', 'CLOSED')),
    exit_price              NUMERIC(30,6),
    exit_tx_signature       TEXT,
    realized_pnl_sol         NUMERIC(30,6),
    realized_pnl_usd       NUMERIC(30,6),
    entry_sol_price_usd     NUMERIC(30,6),
    opened_at               TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    last_updated            TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    closed_at               TIMESTAMPTZ,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

-- Indexes for positions table
CREATE INDEX IF NOT EXISTS idx_positions_state ON positions(state);
CREATE INDEX IF NOT EXISTS idx_positions_state_updated ON positions(state, last_updated);
CREATE INDEX IF NOT EXISTS idx_positions_wallet ON positions(wallet_address);
CREATE INDEX IF NOT EXISTS idx_positions_wallet_token ON positions(wallet_address, token_address);

-- =============================================================================
-- WALLET MANAGEMENT TABLES
-- =============================================================================

-- Wallets table: Tracked wallets with WQS scores (managed by Scout)
-- Schema source of truth: database/schema/wallets.sql
CREATE TABLE IF NOT EXISTS wallets (
    id                      BIGSERIAL PRIMARY KEY,
    address                 TEXT NOT NULL UNIQUE,
    status                  TEXT NOT NULL DEFAULT 'CANDIDATE'
        CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')),
    wqs_score               NUMERIC(10,2),
    wqs_confidence          NUMERIC(3,2),  -- Sample confidence 0-1, unbundled from wqs_score
    roi_7d                  NUMERIC(10,4),
    roi_30d                 NUMERIC(10,4),
    trade_count_30d         INTEGER,
    win_rate                NUMERIC(5,4),
    max_drawdown_30d        NUMERIC(10,4),
    avg_trade_size_sol      NUMERIC(30,6),
    avg_win_sol             NUMERIC(30,6),
    avg_loss_sol            NUMERIC(30,6),
    profit_factor           NUMERIC(10,4),
    realized_pnl_30d_sol    NUMERIC(30,6),
    last_trade_at           TIMESTAMPTZ,
    promoted_at             TIMESTAMPTZ,
    ttl_expires_at          TIMESTAMPTZ,  -- For temporary promotions
    notes                   TEXT,
    archetype               TEXT,  -- TraderArchetype as string (SNIPER, SWING, SCALPER, INSIDER, WHALE)
    avg_entry_delay_seconds NUMERIC(10,2),
    created_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status);
CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC);

-- Wallet address GIN index for fast text searches (if using advanced wallet filters)
CREATE INDEX IF NOT EXISTS idx_wallets_address_gin ON wallets USING GIN(address gin_trgm_ops);

-- =============================================================================
-- SYSTEM TABLES
-- =============================================================================

-- Dead Letter Queue: Failed operations for analysis/retry
CREATE TABLE IF NOT EXISTS dead_letter_queue (
    id                      BIGSERIAL PRIMARY KEY,
    trade_uuid              TEXT,
    payload                 TEXT NOT NULL,
    reason                  TEXT NOT NULL,  -- 'QUEUE_FULL', 'PARSE_ERROR', 'VALIDATION_FAILED', 'MAX_RETRIES'
    error_details           TEXT,
    source_ip               TEXT,
    retry_count             INTEGER DEFAULT 0,
    can_retry               BOOLEAN DEFAULT TRUE,
    received_at             TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    processed_at            TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_dlq_reason ON dead_letter_queue(reason);
CREATE INDEX IF NOT EXISTS idx_dlq_received ON dead_letter_queue(received_at DESC);

-- Config Audit: Track all configuration changes
CREATE TABLE IF NOT EXISTS config_audit (
    id                      BIGSERIAL PRIMARY KEY,
    key                     TEXT NOT NULL,
    old_value               TEXT,
    new_value               TEXT,
    changed_by              TEXT NOT NULL,  -- 'ADMIN', 'SYSTEM_CIRCUIT_BREAKER', 'API', etc.
    change_reason           TEXT,
    changed_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_config_audit_key ON config_audit(key);
CREATE INDEX IF NOT EXISTS idx_config_audit_changed ON config_audit(changed_at DESC);

-- Kill-switch state: single-row table written synchronously before returning from the
-- kill-switch API handler. On startup, main.rs reads this before checking config_audit
-- so crashes between the write and the in-memory circuit-breaker trip are safe.
CREATE TABLE IF NOT EXISTS kill_switch_state (
    id          INTEGER PRIMARY KEY CHECK (id = 1),  -- enforces single-row constraint
    state       TEXT NOT NULL DEFAULT 'INACTIVE' CHECK (state IN ('ACTIVE', 'INACTIVE')),
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    changed_by  TEXT NOT NULL DEFAULT 'SYSTEM',
    reason      TEXT
);

-- Circuit breaker state persistence: single-row table read on startup to restore
-- the last known circuit breaker state across process restarts.
CREATE TABLE IF NOT EXISTS circuit_breaker_state (
    id          INTEGER PRIMARY KEY,
    state       TEXT NOT NULL DEFAULT 'Active',
    tripped_at  TIMESTAMPTZ,
    trip_reason TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO circuit_breaker_state (id, state) VALUES (1, 'Active')
ON CONFLICT (id) DO NOTHING;

-- Admin Wallets: Authorization for API access
CREATE TABLE IF NOT EXISTS admin_wallets (
    wallet_address  TEXT PRIMARY KEY,
    role            TEXT NOT NULL DEFAULT 'readonly'
        CHECK(role IN ('admin', 'operator', 'readonly')),
    added_by        TEXT NOT NULL,
    notes           TEXT,
    added_at        TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- JITO & EXECUTION TABLES
-- =============================================================================

-- Jito Tip History: For dynamic tip calculation (cold start persistence)
CREATE TABLE IF NOT EXISTS jito_tip_history (
    id              BIGSERIAL PRIMARY KEY,
    tip_amount_sol  NUMERIC(30,6) NOT NULL,
    bundle_signature TEXT,
    strategy        TEXT CHECK(strategy IN ('SHIELD', 'SPEAR')),
    success         BOOLEAN DEFAULT TRUE,
    created_at      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_jito_tip_created ON jito_tip_history(created_at DESC);

-- BRIN index for time-series data (jito tips)
CREATE INDEX IF NOT EXISTS idx_jito_tip_created_brin ON jito_tip_history USING BRIN(created_at);

-- =============================================================================
-- RECONCILIATION & COMPLIANCE
-- =============================================================================

-- Reconciliation Log: Compare DB state vs on-chain state
CREATE TABLE IF NOT EXISTS reconciliation_log (
    id                      BIGSERIAL PRIMARY KEY,
    trade_uuid              TEXT NOT NULL,
    expected_state          TEXT NOT NULL,
    actual_on_chain         TEXT,  -- 'FOUND', 'MISSING', 'AMOUNT_MISMATCH'
    discrepancy             TEXT,  -- 'NONE', 'MISSING_TX', 'AMOUNT_MISMATCH', 'STATE_MISMATCH'
    on_chain_tx_signature   TEXT,
    on_chain_amount_sol     NUMERIC(30,6),
    expected_amount_sol     NUMERIC(30,6),
    resolved_at             TIMESTAMPTZ,
    resolved_by             TEXT,  -- 'AUTO', 'ADMIN', 'SYSTEM'
    notes                   TEXT,
    created_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_reconciliation_unresolved
    ON reconciliation_log(resolved_at) WHERE resolved_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_reconciliation_trade ON reconciliation_log(trade_uuid);

-- Backups Tracking
CREATE TABLE IF NOT EXISTS backups (
    id              BIGSERIAL PRIMARY KEY,
    path            TEXT NOT NULL,
    size_bytes      BIGINT,
    checksum        TEXT,
    backup_type     TEXT DEFAULT 'SCHEDULED',  -- 'SCHEDULED', 'MANUAL', 'PRE_MIGRATION'
    created_at      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Historical liquidity data for backtesting and validation
CREATE TABLE IF NOT EXISTS historical_liquidity (
    id              BIGSERIAL PRIMARY KEY,
    token_address   TEXT NOT NULL,
    liquidity_usd   NUMERIC(30,6) NOT NULL,
    price_usd       NUMERIC(30,6),
    volume_24h_usd  NUMERIC(30,6),
    timestamp       TIMESTAMPTZ NOT NULL,
    source          TEXT, -- 'birdeye', 'calculated', 'jupiter', etc.
    created_at      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(token_address, timestamp)
);

-- Index for efficient historical queries
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_token_time
    ON historical_liquidity(token_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_timestamp
    ON historical_liquidity(timestamp DESC);

-- BRIN index for time-series data (historical liquidity)
CREATE INDEX IF NOT EXISTS idx_historical_liquidity_brin ON historical_liquidity USING BRIN(timestamp);

-- =============================================================================
-- MONITORING TABLES
-- =============================================================================

-- Wallet monitoring: Track webhook subscriptions and polling state
CREATE TABLE IF NOT EXISTS wallet_monitoring (
    wallet_address               TEXT PRIMARY KEY,
    helius_webhook_id           TEXT,
    rpc_polling_active          BOOLEAN DEFAULT FALSE,
    last_transaction_signature   TEXT,
    last_monitored_at           TIMESTAMPTZ,
    monitoring_enabled          BOOLEAN DEFAULT TRUE,
    -- Webhook lifecycle columns (added in migration 005)
    webhook_status              TEXT DEFAULT 'active' CHECK(webhook_status IN ('active', 'paused', 'failed', 'orphaned')),
    webhook_registered_at       TIMESTAMPTZ,
    webhook_last_health_check   TIMESTAMPTZ,
    webhook_health_status       TEXT DEFAULT 'unknown' CHECK(webhook_health_status IN ('healthy', 'unhealthy', 'unknown', 'timeout', 'error')),
    registration_attempts       INTEGER DEFAULT 0,
    last_registration_error     TEXT,
    last_updated_url            TEXT,
    created_at                  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at                  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address)
);

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_enabled
    ON wallet_monitoring(monitoring_enabled) WHERE monitoring_enabled = TRUE;

-- Webhook lifecycle indexes (added in migration 005)
CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_webhook_status
    ON wallet_monitoring(webhook_status) WHERE webhook_status = 'active';

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_health_check
    ON wallet_monitoring(webhook_last_health_check);

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_helius_webhook_id
    ON wallet_monitoring(helius_webhook_id) WHERE helius_webhook_id IS NOT NULL;

-- Exit targets: Position-level profit targets and stops
CREATE TABLE IF NOT EXISTS exit_targets (
    id                      BIGSERIAL PRIMARY KEY,
    trade_uuid              TEXT NOT NULL UNIQUE,
    entry_price             NUMERIC(30,6) NOT NULL,
    entry_amount_sol        NUMERIC(30,6) NOT NULL,
    profit_targets          JSONB,  -- JSON array of target percentages
    targets_hit             JSONB, -- JSON array of hit targets
    trailing_stop_active    BOOLEAN DEFAULT FALSE,
    trailing_stop_price     NUMERIC(30,6),
    peak_price              NUMERIC(30,6),
    peak_profit_percent     NUMERIC(10,4),
    stop_loss_price         NUMERIC(30,6),
    remaining_fraction      NUMERIC(5,4) NOT NULL DEFAULT 1.0,
    entry_time              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    last_updated            TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid)
);

CREATE INDEX IF NOT EXISTS idx_exit_targets_trade ON exit_targets(trade_uuid);

-- JSONB GIN index for fast JSON queries on profit_targets and targets_hit
CREATE INDEX IF NOT EXISTS idx_exit_targets_targets_gin ON exit_targets USING GIN(profit_targets);
CREATE INDEX IF NOT EXISTS idx_exit_targets_hits_gin ON exit_targets USING GIN(targets_hit);

-- Signal aggregation: Multi-wallet signal tracking
CREATE TABLE IF NOT EXISTS signal_aggregation (
    id                      BIGSERIAL PRIMARY KEY,
    token_address           TEXT NOT NULL,
    wallet_address          TEXT NOT NULL,
    direction               TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
    amount_sol              NUMERIC(30,6) NOT NULL,
    signature               TEXT,
    is_consensus            BOOLEAN DEFAULT FALSE,
    consensus_wallet_count  INTEGER,
    created_at              TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

-- Unique dedup for signals that carry an on-chain signature
-- PostgreSQL partial unique index
CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_with_sig
    ON signal_aggregation(token_address, wallet_address, signature)
    WHERE signature IS NOT NULL;

-- Unique dedup for signals without a signature
CREATE UNIQUE INDEX IF NOT EXISTS idx_signal_aggregation_unique_no_sig
    ON signal_aggregation(token_address, wallet_address, direction, created_at)
    WHERE signature IS NULL;

CREATE INDEX IF NOT EXISTS idx_signal_aggregation_token_time
    ON signal_aggregation(token_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_signal_aggregation_consensus
    ON signal_aggregation(is_consensus) WHERE is_consensus = TRUE;

-- Wallet copy performance: Per-wallet copy trading metrics
CREATE TABLE IF NOT EXISTS wallet_copy_performance (
    wallet_address      TEXT PRIMARY KEY,
    copy_pnl_7d         NUMERIC(30,6) DEFAULT 0.0,
    copy_pnl_30d        NUMERIC(30,6) DEFAULT 0.0,
    signal_success_rate NUMERIC(5,4) DEFAULT 0.0,
    avg_return_per_trade NUMERIC(10,4) DEFAULT 0.0,
    total_trades        INTEGER DEFAULT 0,
    winning_trades      INTEGER DEFAULT 0,
    last_updated        TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address)
);

CREATE INDEX IF NOT EXISTS idx_wallet_copy_performance_pnl
    ON wallet_copy_performance(copy_pnl_7d DESC);

-- Rate limit metrics: Credit usage and rate tracking
CREATE TABLE IF NOT EXISTS rate_limit_metrics (
    id                  BIGSERIAL PRIMARY KEY,
    metric_type         TEXT NOT NULL,  -- 'webhook', 'rpc', 'total'
    requests_per_second NUMERIC(10,2),
    total_credits_used  BIGINT,
    rate_limit_hits     BIGINT DEFAULT 0,
    timestamp           TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_metrics_time
    ON rate_limit_metrics(timestamp DESC);

-- =============================================================================
-- WEBHOOK LIFECYCLE TABLES
-- =============================================================================

-- Webhook lifecycle audit: Track webhook registration and health
CREATE TABLE IF NOT EXISTS webhook_lifecycle_audit (
    id              BIGSERIAL PRIMARY KEY,
    wallet_address  TEXT NOT NULL,
    action          TEXT NOT NULL CHECK(action IN ('register', 'update', 'delete', 'toggle', 'health_check', 'reconcile')),
    status          TEXT NOT NULL CHECK(status IN ('success', 'failed', 'pending', 'retry')),
    webhook_id      TEXT,
    details         TEXT,
    error_message   TEXT,
    duration_ms     INTEGER,
    created_at      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

-- Webhook lifecycle audit indexes
CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_wallet
    ON webhook_lifecycle_audit(wallet_address, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_action
    ON webhook_lifecycle_audit(action, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_status
    ON webhook_lifecycle_audit(status, created_at DESC);

-- Webhook configuration: Track configuration changes for URL change detection
CREATE TABLE IF NOT EXISTS webhook_configuration (
    id              BIGSERIAL PRIMARY KEY,
    config_key      TEXT UNIQUE NOT NULL,
    config_value     TEXT NOT NULL,
    last_updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_by      TEXT DEFAULT 'system'
);

-- =============================================================================
-- WQS-to-PnL CORRELATION TABLE (Phase 3a)
-- =============================================================================
CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
    wallet_address          TEXT PRIMARY KEY,
    wqs_score_at_promotion NUMERIC(10,2) NOT NULL,
    actual_copy_pnl_7d_sol NUMERIC(30,6),
    actual_copy_pnl_30d_sol NUMERIC(30,6),
    actual_copy_pnl_all_sol NUMERIC(30,6),
    copy_trade_count_7d    INTEGER DEFAULT 0,
    copy_trade_count_30d   INTEGER DEFAULT 0,
    copy_trade_count_all   INTEGER DEFAULT 0,
    strategy               TEXT NOT NULL DEFAULT 'SHIELD'
        CHECK(strategy IN ('SHIELD', 'SPEAR')),
    wqs_components_json    JSONB,
    promoted_at            TIMESTAMPTZ NOT NULL,
    last_updated_at        TIMESTAMPTZ NOT NULL
);

-- JSONB GIN index for wqs_components_json queries
CREATE INDEX IF NOT EXISTS idx_wqs_pnl_components_gin ON wqs_pnl_correlation USING GIN(wqs_components_json);

-- =============================================================================
-- FUNCTIONS (PostgreSQL triggers require functions)
-- =============================================================================

-- Generic updated_at trigger function
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Generic last_updated trigger function
CREATE OR REPLACE FUNCTION update_last_updated_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.last_updated = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- TRIGGERS
-- =============================================================================

-- Auto-update updated_at on trades
CREATE TRIGGER trades_updated_at
    BEFORE UPDATE ON trades
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Auto-update updated_at on wallets
CREATE TRIGGER wallets_updated_at
    BEFORE UPDATE ON wallets
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Auto-update last_updated on positions
CREATE TRIGGER positions_updated_at
    BEFORE UPDATE ON positions
    FOR EACH ROW
    EXECUTE FUNCTION update_last_updated_column();

-- Auto-update updated_at on wallet_monitoring
CREATE TRIGGER wallet_monitoring_updated_at
    BEFORE UPDATE ON wallet_monitoring
    FOR EACH ROW
    EXECUTE FUNCTION update_last_updated_column();

-- Auto-update last_updated on exit_targets
CREATE TRIGGER exit_targets_updated_at
    BEFORE UPDATE ON exit_targets
    FOR EACH ROW
    EXECUTE FUNCTION update_last_updated_column();

-- =============================================================================
-- VIEWS FOR COMMON QUERIES (Optional performance optimization)
-- =============================================================================

-- Active positions with wallet info
CREATE OR REPLACE VIEW v_active_positions AS
SELECT
    p.id,
    p.trade_uuid,
    p.wallet_address,
    p.token_address,
    p.token_symbol,
    p.strategy,
    p.entry_amount_sol,
    p.entry_price,
    p.current_price,
    p.unrealized_pnl_sol,
    p.unrealized_pnl_percent,
    p.state,
    p.opened_at,
    p.last_updated,
    w.wqs_score,
    w.status as wallet_status,
    w.archetype
FROM positions p
JOIN wallets w ON p.wallet_address = w.address
WHERE p.state = 'ACTIVE';

-- Recent trades with PnL
CREATE OR REPLACE VIEW v_recent_trades_pnl AS
SELECT
    t.id,
    t.trade_uuid,
    t.wallet_address,
    t.token_address,
    t.token_symbol,
    t.strategy,
    t.side,
    t.amount_sol,
    t.price_at_signal,
    t.status,
    t.pnl_sol,
    t.pnl_usd,
    t.total_cost_sol,
    t.net_pnl_sol,
    t.created_at,
    t.updated_at
FROM trades t
WHERE t.status IN ('CLOSED', 'ACTIVE')
ORDER BY t.created_at DESC;

-- =============================================================================
-- COMMENTS FOR DOCUMENTATION
-- =============================================================================

COMMENT ON TABLE trades IS 'Primary record of all trading signals received';
COMMENT ON TABLE positions IS 'Active positions being tracked with unrealized PnL';
COMMENT ON TABLE wallets IS 'Tracked wallets with WQS scores (managed by Scout)';
COMMENT ON TABLE dead_letter_queue IS 'Failed operations for analysis and retry';
COMMENT ON TABLE config_audit IS 'Immutable audit trail of configuration changes';
COMMENT ON TABLE circuit_breaker_state IS 'Single-row table for circuit breaker state persistence';
COMMENT ON TABLE jito_tip_history IS 'Dynamic tip calculation cold start data';
COMMENT ON TABLE reconciliation_log IS 'On-chain vs off-chain state comparison';
COMMENT ON TABLE signal_aggregation IS 'Multi-wallet consensus tracking';
COMMENT ON TABLE wqs_pnl_correlation IS 'WQS-to-PnL correlation for predictive power analysis';
COMMENT ON TABLE webhook_lifecycle_audit IS 'Webhook registration and health tracking audit log';
COMMENT ON TABLE webhook_configuration IS 'Active webhook configurations for wallet monitoring';

COMMENT ON COLUMN trades.net_pnl_sol IS 'Net PnL after all costs (jito_tip + dex_fee + slippage)';
COMMENT ON COLUMN wallets.wqs_confidence IS 'Statistical confidence 0-1, separate from WQS score';
COMMENT ON COLUMN positions.unrealized_pnl_percent IS 'Percentage gain/loss since entry';
COMMENT ON COLUMN exit_targets.remaining_fraction IS 'Fraction of position still open (0.0 = fully exited)';
COMMENT ON COLUMN webhook_configuration.health_status IS 'Current health of webhook: unknown, healthy, degraded, failed';
