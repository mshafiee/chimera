-- Chimera Database Schema - PostgreSQL
-- Generated from database/schema_yaml/*.yaml

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

-- =============================================================================
-- ML predictions for wallet performance
-- =============================================================================

-- ML predictions for wallet performance
COMMENT ON TABLE ml_predictions IS 'ML predictions for wallet performance';
CREATE TABLE IF NOT EXISTS ml_predictions (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    model_version TEXT NOT NULL,
    prediction_type TEXT NOT NULL,
    predicted_value NUMERIC(30) NOT NULL,
    confidence_score DOUBLE PRECISION,
    features_json JSONB,
    prediction_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    actual_value NUMERIC(30),
    actual_timestamp TIMESTAMPTZ,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_ml_predictions_wallet ON ml_predictions (wallet_address, prediction_timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_ml_predictions_type ON ml_predictions (prediction_type, prediction_timestamp DESC);

-- =============================================================================
-- Exit recommendations for positions
-- =============================================================================

-- Exit recommendations for positions
COMMENT ON TABLE exit_recommendations IS 'Exit recommendations for positions';
CREATE TABLE IF NOT EXISTS exit_recommendations (
    id BIGSERIAL PRIMARY KEY,
    trade_uuid TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    recommendation_type TEXT NOT NULL CHECK(recommendation_type IN ('TAKE_PROFIT', 'STOP_LOSS', 'TRAILING_STOP', 'MANUAL')),
    target_price NUMERIC(30),
    target_percentage NUMERIC(10),
    reason TEXT,
    confidence_score DOUBLE PRECISION,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    executed_at TIMESTAMPTZ,
    executed BOOLEAN DEFAULT FALSE,
    FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid) ON DELETE CASCADE,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_exit_recommendations_trade ON exit_recommendations (trade_uuid, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_exit_recommendations_wallet ON exit_recommendations (wallet_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_exit_recommendations_pending ON exit_recommendations (created_at DESC) WHERE executed = false;

-- =============================================================================
-- System alerts and notifications
-- =============================================================================

-- System alerts and notifications
COMMENT ON TABLE alerts IS 'System alerts and notifications';
CREATE TABLE IF NOT EXISTS alerts (
    id BIGSERIAL PRIMARY KEY,
    alert_type TEXT NOT NULL CHECK(alert_type IN ('WALLET_PERFORMANCE', 'POSITION_RISK', 'SYSTEM', 'WEBHOOK', 'RATE_LIMIT', 'LIQUIDITY')),
    severity TEXT NOT NULL CHECK(severity IN ('INFO', 'WARNING', 'ERROR', 'CRITICAL')),
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    wallet_address TEXT,
    trade_uuid TEXT,
    metadata_json JSONB,
    acknowledged BOOLEAN DEFAULT FALSE,
    acknowledged_at TIMESTAMPTZ,
    acknowledged_by TEXT,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_alerts_type ON alerts (alert_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_severity ON alerts (severity, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_unacknowledged ON alerts (created_at DESC) WHERE acknowledged = false;
CREATE INDEX IF NOT EXISTS idx_alerts_wallet ON alerts (wallet_address, created_at DESC);

-- =============================================================================
-- System performance metrics
-- =============================================================================

-- System performance metrics
COMMENT ON TABLE metrics IS 'System performance metrics';
CREATE TABLE IF NOT EXISTS metrics (
    id BIGSERIAL PRIMARY KEY,
    metric_name TEXT NOT NULL,
    metric_value NUMERIC(30) NOT NULL,
    metric_unit TEXT,
    labels_json JSONB,
    timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_metrics_name ON metrics (metric_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_timestamp ON metrics (timestamp DESC);

-- =============================================================================
-- System health check results
-- =============================================================================

-- System health check results
COMMENT ON TABLE health_checks IS 'System health check results';
CREATE TABLE IF NOT EXISTS health_checks (
    id BIGSERIAL PRIMARY KEY,
    check_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('HEALTHY', 'DEGRADED', 'UNHEALTHY', 'UNKNOWN')),
    response_time_ms INTEGER,
    details_json JSONB,
    timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_health_checks_name ON health_checks (check_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_health_checks_status ON health_checks (status, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_health_checks_timestamp ON health_checks (timestamp DESC);

-- =============================================================================
-- Account growth tracking over time
-- =============================================================================

-- Account growth tracking over time
COMMENT ON TABLE growth_history IS 'Account growth tracking over time';
CREATE TABLE IF NOT EXISTS growth_history (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    balance_sol NUMERIC(30) NOT NULL,
    balance_usd NUMERIC(30),
    total_pnl_sol NUMERIC(30),
    total_pnl_usd NUMERIC(30),
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

-- Capital deposits and withdrawals
COMMENT ON TABLE capital_events IS 'Capital deposits and withdrawals';
CREATE TABLE IF NOT EXISTS capital_events (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('DEPOSIT', 'WITHDRAWAL', 'PROFIT_DISTRIBUTION', 'LOSS_REALIZATION')),
    amount_sol NUMERIC(30) NOT NULL,
    amount_usd NUMERIC(30),
    balance_before_sol NUMERIC(30),
    balance_after_sol NUMERIC(30),
    tx_signature TEXT,
    notes TEXT,
    timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_capital_events_wallet ON capital_events (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_capital_events_type ON capital_events (event_type, timestamp DESC);

-- =============================================================================
-- Growth-related alerts and notifications
-- =============================================================================

-- Growth-related alerts and notifications
COMMENT ON TABLE growth_alerts IS 'Growth-related alerts and notifications';
CREATE TABLE IF NOT EXISTS growth_alerts (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    alert_type TEXT NOT NULL CHECK(alert_type IN ('NEW_HIGH', 'NEW_LOW', 'DRAWDOWN_WARNING', 'PROFIT_TARGET', 'LOSS_LIMIT')),
    current_balance_sol NUMERIC(30) NOT NULL,
    previous_balance_sol NUMERIC(30),
    percentage_change NUMERIC(10),
    threshold_value NUMERIC(10),
    acknowledged BOOLEAN DEFAULT FALSE,
    acknowledged_at TIMESTAMPTZ,
    timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_growth_alerts_wallet ON growth_alerts (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_growth_alerts_type ON growth_alerts (alert_type, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_growth_alerts_unacknowledged ON growth_alerts (wallet_address) WHERE acknowledged = false;

-- =============================================================================
-- Credit usage history for rate limiting
-- =============================================================================

-- Credit usage history for rate limiting
COMMENT ON TABLE credit_history IS 'Credit usage history for rate limiting';
CREATE TABLE IF NOT EXISTS credit_history (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    credits_used INTEGER NOT NULL,
    credits_remaining INTEGER NOT NULL,
    operation_type TEXT NOT NULL,
    timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_credit_history_wallet ON credit_history (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_credit_history_timestamp ON credit_history (timestamp DESC);

-- =============================================================================
-- Historical wallet performance metrics
-- =============================================================================

-- Historical wallet performance metrics
COMMENT ON TABLE wallet_performance_history IS 'Historical wallet performance metrics';
CREATE TABLE IF NOT EXISTS wallet_performance_history (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    total_trades INTEGER DEFAULT 0,
    winning_trades INTEGER DEFAULT 0,
    losing_trades INTEGER DEFAULT 0,
    win_rate NUMERIC(5),
    total_pnl_sol NUMERIC(30),
    total_pnl_usd NUMERIC(30),
    avg_win_sol NUMERIC(30),
    avg_loss_sol NUMERIC(30),
    profit_factor NUMERIC(10),
    max_drawdown_sol NUMERIC(30),
    max_drawdown_percent NUMERIC(10),
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_wallet_performance_history_wallet ON wallet_performance_history (wallet_address, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_wallet_performance_history_timestamp ON wallet_performance_history (timestamp DESC);

-- =============================================================================
-- ROI calculations and metrics
-- =============================================================================

-- ROI calculations and metrics
COMMENT ON TABLE roi_metrics IS 'ROI calculations and metrics';
CREATE TABLE IF NOT EXISTS roi_metrics (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    timeframe TEXT NOT NULL CHECK(timeframe IN ('1h', '24h', '7d', '30d', '90d', 'all')),
    roi_percent NUMERIC(10),
    roi_sol NUMERIC(30),
    roi_usd NUMERIC(30),
    total_invested_sol NUMERIC(30),
    total_returned_sol NUMERIC(30),
    trade_count INTEGER DEFAULT 0,
    sharpe_ratio NUMERIC(10),
    sortino_ratio NUMERIC(10),
    max_drawdown_percent NUMERIC(10),
    calculated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_roi_metrics_wallet ON roi_metrics (wallet_address, timeframe);
CREATE INDEX IF NOT EXISTS idx_roi_metrics_timeframe ON roi_metrics (timeframe, calculated_at DESC);

-- =============================================================================
-- Multi-timeframe wallet discovery statistics
-- =============================================================================

-- Multi-timeframe wallet discovery statistics
COMMENT ON TABLE multi_timeframe_discovery_stats IS 'Multi-timeframe wallet discovery statistics';
CREATE TABLE IF NOT EXISTS multi_timeframe_discovery_stats (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    discovery_timeframe TEXT NOT NULL,
    wqs_score DOUBLE PRECISION,
    sample_size INTEGER,
    confidence_interval NUMERIC(10),
    signal_count INTEGER,
    avg_signal_strength NUMERIC(10),
    discovered_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_multi_timeframe_discovery_wallet ON multi_timeframe_discovery_stats (wallet_address, discovery_timeframe);
CREATE INDEX IF NOT EXISTS idx_multi_timeframe_discovery_wqs ON multi_timeframe_discovery_stats (wqs_score DESC);
