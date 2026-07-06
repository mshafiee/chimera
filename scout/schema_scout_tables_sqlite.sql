-- Chimera Database Schema - SQLite
-- Generated from database/schema_yaml/*.yaml
-- Financial values stored as TEXT (Decimal strings) to avoid IEEE 754 precision loss

-- =============================================================================
-- ML predictions for wallet performance
-- =============================================================================

CREATE TABLE IF NOT EXISTS ml_predictions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    model_version TEXT NOT NULL,
    prediction_type TEXT NOT NULL,
    predicted_value TEXT NOT NULL,
    confidence_score REAL,
    features_json TEXT,
    prediction_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    actual_value TEXT,
    actual_timestamp TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_ml_predictions_wallet ON ml_predictions (wallet_address, prediction_timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_type ON ml_predictions (prediction_type, prediction_timestamp DESC);

-- =============================================================================
-- Exit recommendations for positions
-- =============================================================================

CREATE TABLE IF NOT EXISTS exit_recommendations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    recommendation_type TEXT NOT NULL CHECK(recommendation_type IN ('TAKE_PROFIT', 'STOP_LOSS', 'TRAILING_STOP', 'MANUAL')),
    target_price TEXT,
    target_percentage TEXT,
    reason TEXT,
    confidence_score REAL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    executed_at TIMESTAMP,
    executed INTEGER DEFAULT 0,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid) ON DELETE CASCADE,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_exit_recommendations_trade ON exit_recommendations (trade_uuid, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_exit_recommendations_wallet ON exit_recommendations (wallet_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_exit_recommendations_pending ON exit_recommendations (created_at DESC) WHERE executed = false;

-- =============================================================================
-- System alerts and notifications
-- =============================================================================

CREATE TABLE IF NOT EXISTS alerts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_type TEXT NOT NULL CHECK(alert_type IN ('WALLET_PERFORMANCE', 'POSITION_RISK', 'SYSTEM', 'WEBHOOK', 'RATE_LIMIT', 'LIQUIDITY')),
    severity TEXT NOT NULL CHECK(severity IN ('INFO', 'WARNING', 'ERROR', 'CRITICAL')),
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    wallet_address TEXT,
    trade_uuid TEXT,
    metadata_json TEXT,
    acknowledged INTEGER DEFAULT 0,
    acknowledged_at TIMESTAMP,
    acknowledged_by TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_alerts_type ON alerts (alert_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_severity ON alerts (severity, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_unacknowledged ON alerts (created_at DESC) WHERE acknowledged = false;
CREATE INDEX IF NOT EXISTS idx_alerts_wallet ON alerts (wallet_address, created_at DESC);

-- =============================================================================
-- System performance metrics
-- =============================================================================

CREATE TABLE IF NOT EXISTS metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    metric_name TEXT NOT NULL,
    metric_value TEXT NOT NULL,
    metric_unit TEXT,
    labels_json TEXT,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_metrics_name ON metrics (metric_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_timestamp ON metrics (timestamp DESC);

-- =============================================================================
-- System health check results
-- =============================================================================

CREATE TABLE IF NOT EXISTS health_checks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    check_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('HEALTHY', 'DEGRADED', 'UNHEALTHY', 'UNKNOWN')),
    response_time_ms INTEGER,
    details_json TEXT,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_health_checks_name ON health_checks (check_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_health_checks_status ON health_checks (status, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_health_checks_timestamp ON health_checks (timestamp DESC);

-- =============================================================================
-- Account growth tracking over time
-- =============================================================================

CREATE TABLE IF NOT EXISTS growth_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    balance_sol TEXT NOT NULL,
    balance_usd TEXT,
    total_pnl_sol TEXT,
    total_pnl_usd TEXT,
    trade_count INTEGER DEFAULT 0,
    win_count INTEGER DEFAULT 0,
    loss_count INTEGER DEFAULT 0,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_growth_history_wallet ON growth_history (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_growth_history_timestamp ON growth_history (timestamp DESC);

-- =============================================================================
-- Capital deposits and withdrawals
-- =============================================================================

CREATE TABLE IF NOT EXISTS capital_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('DEPOSIT', 'WITHDRAWAL', 'PROFIT_DISTRIBUTION', 'LOSS_REALIZATION')),
    amount_sol TEXT NOT NULL,
    amount_usd TEXT,
    balance_before_sol TEXT,
    balance_after_sol TEXT,
    tx_signature TEXT,
    notes TEXT,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_capital_events_wallet ON capital_events (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_capital_events_type ON capital_events (event_type, timestamp DESC);

-- =============================================================================
-- Growth-related alerts and notifications
-- =============================================================================

CREATE TABLE IF NOT EXISTS growth_alerts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    alert_type TEXT NOT NULL CHECK(alert_type IN ('NEW_HIGH', 'NEW_LOW', 'DRAWDOWN_WARNING', 'PROFIT_TARGET', 'LOSS_LIMIT')),
    current_balance_sol TEXT NOT NULL,
    previous_balance_sol TEXT,
    percentage_change TEXT,
    threshold_value TEXT,
    acknowledged INTEGER DEFAULT 0,
    acknowledged_at TIMESTAMP,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_growth_alerts_wallet ON growth_alerts (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_growth_alerts_type ON growth_alerts (alert_type, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_growth_alerts_unacknowledged ON growth_alerts (wallet_address) WHERE acknowledged = false;

-- =============================================================================
-- Credit usage history for rate limiting
-- =============================================================================

CREATE TABLE IF NOT EXISTS credit_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    credits_used INTEGER NOT NULL,
    credits_remaining INTEGER NOT NULL,
    operation_type TEXT NOT NULL,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_credit_history_wallet ON credit_history (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_credit_history_timestamp ON credit_history (timestamp DESC);

-- =============================================================================
-- Historical wallet performance metrics
-- =============================================================================

CREATE TABLE IF NOT EXISTS wallet_performance_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    total_trades INTEGER DEFAULT 0,
    winning_trades INTEGER DEFAULT 0,
    losing_trades INTEGER DEFAULT 0,
    win_rate TEXT,
    total_pnl_sol TEXT,
    total_pnl_usd TEXT,
    avg_win_sol TEXT,
    avg_loss_sol TEXT,
    profit_factor TEXT,
    max_drawdown_sol TEXT,
    max_drawdown_percent TEXT,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_wallet_performance_history_wallet ON wallet_performance_history (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_wallet_performance_history_timestamp ON wallet_performance_history (timestamp DESC);

-- =============================================================================
-- ROI calculations and metrics
-- =============================================================================

CREATE TABLE IF NOT EXISTS roi_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    timeframe TEXT NOT NULL CHECK(timeframe IN ('1h', '24h', '7d', '30d', '90d', 'all')),
    roi_percent TEXT,
    roi_sol TEXT,
    roi_usd TEXT,
    total_invested_sol TEXT,
    total_returned_sol TEXT,
    trade_count INTEGER DEFAULT 0,
    sharpe_ratio TEXT,
    sortino_ratio TEXT,
    max_drawdown_percent TEXT,
    calculated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_roi_metrics_wallet ON roi_metrics (wallet_address, timeframe);
CREATE INDEX IF NOT EXISTS idx_roi_metrics_timeframe ON roi_metrics (timeframe, calculated_at DESC);

-- =============================================================================
-- Multi-timeframe wallet discovery statistics
-- =============================================================================

CREATE TABLE IF NOT EXISTS multi_timeframe_discovery_stats (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    discovery_timeframe TEXT NOT NULL,
    wqs_score REAL,
    sample_size INTEGER,
    confidence_interval TEXT,
    signal_count INTEGER,
    avg_signal_strength TEXT,
    discovered_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_multi_timeframe_discovery_wallet ON multi_timeframe_discovery_stats (wallet_address, discovery_timeframe);
CREATE INDEX IF NOT EXISTS idx_multi_timeframe_discovery_wqs ON multi_timeframe_discovery_stats (wqs_score DESC);
