-- Chimera 10-Day Evaluation Database Schema
-- Extended schema for comprehensive evaluation metrics and system analysis
-- This extends the base schema with evaluation-specific tables for systematic data collection

-- ===================================================================
-- EVALUATION SNAPSHOTS - Hourly system state captures
-- ===================================================================
CREATE TABLE IF NOT EXISTS evaluation_snapshots (
    id SERIAL PRIMARY KEY,
    snapshot_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    day_number INTEGER NOT NULL,
    hour_number INTEGER NOT NULL,

    -- System Health Metrics
    cpu_usage_percent REAL,
    memory_usage_percent REAL,
    disk_usage_percent REAL,
    network_io_bytes BIGINT,

    -- Trading Activity
    active_positions_count INTEGER DEFAULT 0,
    queue_depth INTEGER DEFAULT 0,
    total_trades_today INTEGER DEFAULT 0,
    successful_trades_today INTEGER DEFAULT 0,
    failed_trades_today INTEGER DEFAULT 0,

    -- Performance Metrics
    avg_trade_latency_ms REAL,
    p95_trade_latency_ms REAL,
    p99_trade_latency_ms REAL,
    rpc_latency_avg_ms REAL,
    rpc_latency_p95_ms REAL,

    -- Financial Metrics
    total_pnl_sol REAL DEFAULT 0.0,
    total_pnl_usd REAL DEFAULT 0.0,
    unrealized_pnl_sol REAL DEFAULT 0.0,
    total_costs_sol REAL DEFAULT 0.0,

    -- Risk Metrics
    circuit_breaker_state INTEGER DEFAULT 0,
    max_drawdown_percent REAL DEFAULT 0.0,
    portfolio_exposure_percent REAL DEFAULT 0.0,

    -- Database Performance
    db_query_latency_avg_ms REAL,
    db_connection_pool_usage REAL,
    db_lock_contention INTEGER DEFAULT 0,

    -- System Events
    error_count INTEGER DEFAULT 0,
    warning_count INTEGER DEFAULT 0,
    webhook_count INTEGER DEFAULT 0,

    -- Detailed JSON Data
    snapshot_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for efficient querying
CREATE INDEX idx_eval_snapshots_time ON evaluation_snapshots(snapshot_time);
CREATE INDEX idx_eval_snapshots_day_hour ON evaluation_snapshots(day_number, hour_number);
CREATE INDEX idx_eval_snapshots_performance ON evaluation_snapshots(avg_trade_latency_ms, p99_trade_latency_ms);
CREATE INDEX idx_eval_snapshots_financial ON evaluation_snapshots(total_pnl_sol, total_costs_sol);

-- ===================================================================
-- TRADE EXECUTION DETAILS - Detailed performance tracking
-- ===================================================================
CREATE TABLE IF NOT EXISTS trade_execution_details (
    id SERIAL PRIMARY KEY,
    trade_uuid TEXT NOT NULL,

    -- Timeline Tracking
    signal_received_time TIMESTAMP NOT NULL,
    trade_queued_time TIMESTAMP,
    trade_executing_time TIMESTAMP,
    trade_active_time TIMESTAMP,
    trade_closed_time TIMESTAMP,

    -- Latency Breakdown (microseconds)
    signal_to_queue_latency_us INTEGER,
    queue_to_execute_latency_us INTEGER,
    execute_to_active_latency_us INTEGER,
    total_execution_latency_us INTEGER,

    -- Execution Costs
    jito_tip_sol REAL DEFAULT 0.0,
    dex_fee_sol REAL DEFAULT 0.0,
    slippage_cost_sol REAL DEFAULT 0.0,
    network_fee_sol REAL DEFAULT 0.0,
    total_cost_sol REAL DEFAULT 0.0,

    -- RPC Performance
    rpc_calls_count INTEGER DEFAULT 0,
    rpc_total_latency_ms INTEGER DEFAULT 0,
    rpc_errors_count INTEGER DEFAULT 0,

    -- Database Performance
    db_queries_count INTEGER DEFAULT 0,
    db_total_latency_ms INTEGER DEFAULT 0,
    db_errors_count INTEGER DEFAULT 0,

    -- Trade Details
    wallet_address TEXT,
    token_address TEXT,
    strategy TEXT,
    action TEXT,
    amount_sol REAL,

    -- Result
    status TEXT DEFAULT 'PENDING',
    error_message TEXT,

    -- Detailed Data
    execution_details JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Foreign Key References
    CONSTRAINT fk_trade_uuid FOREIGN KEY (trade_uuid) REFERENCES trades(trade_uuid) ON DELETE CASCADE
);

CREATE INDEX idx_trade_details_uuid ON trade_execution_details(trade_uuid);
CREATE INDEX idx_trade_details_timeline ON trade_execution_details(signal_received_time, trade_active_time);
CREATE INDEX idx_trade_details_performance ON trade_execution_details(total_execution_latency_us);
CREATE INDEX idx_trade_details_costs ON trade_execution_details(total_cost_sol);

-- ===================================================================
-- SYSTEM RESOURCES - Container resource monitoring
-- ===================================================================
CREATE TABLE IF NOT EXISTS system_resources (
    id SERIAL PRIMARY KEY,
    snapshot_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    container_name TEXT NOT NULL,

    -- CPU Metrics
    cpu_percent REAL DEFAULT 0.0,
    cpu_cores REAL DEFAULT 0.0,

    -- Memory Metrics
    memory_mb INTEGER DEFAULT 0,
    memory_percent REAL DEFAULT 0.0,
    memory_limit_mb INTEGER DEFAULT 0,

    -- Disk I/O
    disk_read_mb REAL DEFAULT 0.0,
    disk_write_mb REAL DEFAULT 0.0,
    disk_io_total_mb REAL DEFAULT 0.0,

    -- Network I/O
    network_rx_mb REAL DEFAULT 0.0,
    network_tx_mb REAL DEFAULT 0.0,
    network_io_total_mb REAL DEFAULT 0.0,

    -- System Metrics
    processes_count INTEGER DEFAULT 0,
    threads_count INTEGER DEFAULT 0,
    file_descriptors_count INTEGER DEFAULT 0,

    -- Detailed Data
    resource_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_system_resources_time ON system_resources(snapshot_time);
CREATE INDEX idx_system_resources_container ON system_resources(container_name, snapshot_time);

-- ===================================================================
-- EVALUATION ANOMALIES - Real-time anomaly tracking
-- ===================================================================
CREATE TABLE IF NOT EXISTS evaluation_anomalies (
    id SERIAL PRIMARY KEY,
    anomaly_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    day_number INTEGER NOT NULL,
    hour_number INTEGER NOT NULL,

    -- Anomaly Details
    anomaly_type TEXT NOT NULL,
    severity TEXT DEFAULT 'WARNING', -- WARNING, CRITICAL
    metric_name TEXT NOT NULL,
    metric_value REAL,
    threshold_value REAL,
    deviation_percent REAL,

    -- Context
    description TEXT,
    affected_component TEXT,
    related_snapshot_id INTEGER,

    -- Alert Status
    alert_sent BOOLEAN DEFAULT FALSE,
    alert_time TIMESTAMP,
    acknowledged BOOLEAN DEFAULT FALSE,
    resolved BOOLEAN DEFAULT FALSE,
    resolution_time TIMESTAMP,
    resolution_notes TEXT,

    -- Detailed Data
    anomaly_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT fk_anomaly_snapshot FOREIGN KEY (related_snapshot_id) REFERENCES evaluation_snapshots(id) ON DELETE SET NULL
);

CREATE INDEX idx_anomalies_time ON evaluation_anomalies(anomaly_time);
CREATE INDEX idx_anomalies_type_severity ON evaluation_anomalies(anomaly_type, severity);
CREATE INDEX idx_anomalies_resolved ON evaluation_anomalies(resolved, anomaly_time);

-- ===================================================================
-- SIGNAL REPLAY LOG - Historical signal replay tracking
-- ===================================================================
CREATE TABLE IF NOT EXISTS signal_replay_log (
    id SERIAL PRIMARY KEY,
    replay_session_id TEXT NOT NULL,

    -- Signal Details
    original_signal_time TIMESTAMP NOT NULL,
    replay_time TIMESTAMP NOT NULL,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    action TEXT NOT NULL,
    amount_sol REAL,
    strategy TEXT DEFAULT 'shield',

    -- Replay Status
    replay_status TEXT DEFAULT 'PENDING',
    trade_uuid TEXT,
    response_status_code INTEGER,
    response_time_ms INTEGER,

    -- Performance
    original_to_replay_delay_ms INTEGER,
    replay_execution_latency_ms INTEGER,

    -- Result
    success BOOLEAN DEFAULT FALSE,
    error_message TEXT,

    -- Detailed Data
    signal_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_signal_replay_session ON signal_replay_log(replay_session_id);
CREATE INDEX idx_signal_replay_time ON signal_replay_log(replay_time);
CREATE INDEX idx_signal_replay_status ON signal_replay_log(replay_status, success);

-- ===================================================================
-- DAILY EVALUATION SUMMARIES - Pre-computed daily aggregates
-- ===================================================================
CREATE TABLE IF NOT EXISTS daily_evaluation_summaries (
    id SERIAL PRIMARY KEY,
    day_number INTEGER NOT NULL UNIQUE,
    summary_date DATE NOT NULL,

    -- Trading Performance
    total_trades INTEGER DEFAULT 0,
    successful_trades INTEGER DEFAULT 0,
    failed_trades INTEGER DEFAULT 0,
    success_rate_percent REAL,

    -- Financial Performance
    total_pnl_sol REAL DEFAULT 0.0,
    total_pnl_usd REAL DEFAULT 0.0,
    avg_pnl_per_trade_sol REAL,
    total_costs_sol REAL DEFAULT 0.0,
    avg_cost_per_trade_sol REAL,

    -- Performance Metrics
    avg_trade_latency_ms REAL,
    p95_trade_latency_ms REAL,
    p99_trade_latency_ms REAL,
    avg_rpc_latency_ms REAL,

    -- Risk Metrics
    max_drawdown_percent REAL,
    portfolio_volatility REAL,
    circuit_breaker_trips INTEGER DEFAULT 0,

    -- System Health
    avg_cpu_usage_percent REAL,
    avg_memory_usage_percent REAL,
    avg_disk_usage_percent REAL,
    total_errors INTEGER DEFAULT 0,
    total_warnings INTEGER DEFAULT 0,

    -- Database Performance
    avg_db_query_latency_ms REAL,
    db_lock_contentions INTEGER DEFAULT 0,

    -- Anomalies
    total_anomalies INTEGER DEFAULT 0,
    critical_anomalies INTEGER DEFAULT 0,
    warning_anomalies INTEGER DEFAULT 0,

    -- Signal Replay Metrics (if applicable)
    signals_replayed INTEGER DEFAULT 0,
    replay_success_rate_percent REAL,

    -- Detailed Data
    summary_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_daily_summaries_date ON daily_evaluation_summaries(summary_date);
CREATE INDEX idx_daily_summaries_performance ON daily_evaluation_summaries(total_pnl_sol, success_rate_percent);

-- ===================================================================
-- EVALUATION INCIDENTS - Significant event tracking
-- ===================================================================
CREATE TABLE IF NOT EXISTS evaluation_incidents (
    id SERIAL PRIMARY KEY,
    incident_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    day_number INTEGER NOT NULL,

    -- Incident Details
    incident_type TEXT NOT NULL,
    severity TEXT DEFAULT 'MEDIUM', -- LOW, MEDIUM, HIGH, CRITICAL
    title TEXT NOT NULL,
    description TEXT,

    -- Impact Assessment
    impact_duration_minutes INTEGER,
    trades_affected INTEGER DEFAULT 0,
    pnl_impact_sol REAL DEFAULT 0.0,
    system_downtime_minutes INTEGER DEFAULT 0,

    -- Root Cause Analysis
    root_cause_category TEXT,
    root_cause_description TEXT,
    contributing_factors TEXT,

    -- Resolution
    resolution_status TEXT DEFAULT 'OPEN', -- OPEN, INVESTIGATING, RESOLVED
    resolution_actions TEXT,
    resolution_time TIMESTAMP,
    resolved_by TEXT,

    -- Prevention
    prevention_actions TEXT,
    follow_up_required BOOLEAN DEFAULT FALSE,
    follow_up_notes TEXT,

    -- Related Data
    related_anomaly_ids INTEGER[],
    related_trade_uuids TEXT[],

    -- Detailed Data
    incident_data JSONB,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_incidents_time ON evaluation_incidents(incident_time);
CREATE INDEX idx_incidents_severity ON evaluation_incidents(severity, resolution_status);
CREATE INDEX idx_incidents_type ON evaluation_incidents(incident_type);

-- ===================================================================
-- HELPER FUNCTIONS - Performance trend analysis and utilities
-- ===================================================================

-- Function to calculate performance trends
CREATE OR REPLACE FUNCTION calculate_performance_trend(days_lookback INTEGER DEFAULT 7)
RETURNS TABLE (
    metric_name TEXT,
    current_value REAL,
    previous_value REAL,
    change_percent REAL,
    trend_direction TEXT
) AS $$
BEGIN
    RETURN QUERY
    WITH latest_snapshot AS (
        SELECT * FROM evaluation_snapshots
        ORDER BY snapshot_time DESC LIMIT 1
    ),
    previous_snapshots AS (
        SELECT * FROM evaluation_snapshots
        WHERE snapshot_time < (SELECT snapshot_time FROM latest_snapshot)
        ORDER BY snapshot_time DESC LIMIT days_lookback
    ),
    metrics AS (
        SELECT
            'avg_trade_latency_ms'::TEXT as metric_name,
            (SELECT avg_trade_latency_ms FROM latest_snapshot) as current_value,
            (SELECT AVG(avg_trade_latency_ms) FROM previous_snapshots) as previous_value

        UNION ALL

        SELECT
            'total_pnl_sol'::TEXT,
            (SELECT total_pnl_sol FROM latest_snapshot),
            (SELECT AVG(total_pnl_sol) FROM previous_snapshots)

        UNION ALL

        SELECT
            'memory_usage_percent'::TEXT,
            (SELECT memory_usage_percent FROM latest_snapshot),
            (SELECT AVG(memory_usage_percent) FROM previous_snapshots)

        UNION ALL

        SELECT
            'rpc_latency_avg_ms'::TEXT,
            (SELECT rpc_latency_avg_ms FROM latest_snapshot),
            (SELECT AVG(rpc_latency_avg_ms) FROM previous_snapshots)
    )
    SELECT
        m.metric_name,
        m.current_value,
        m.previous_value,
        CASE WHEN m.previous_value != 0
            THEN ((m.current_value - m.previous_value) / m.previous_value) * 100
            ELSE 0
        END as change_percent,
        CASE
            WHEN m.previous_value = 0 THEN 'STABLE'
            WHEN m.current_value > m.previous_value THEN 'IMPROVING'
            WHEN m.current_value < m.previous_value THEN 'DEGRADING'
            ELSE 'STABLE'
        END as trend_direction
    FROM metrics m
    WHERE m.current_value IS NOT NULL AND m.previous_value IS NOT NULL;
END;
$$ LANGUAGE plpgsql;

-- Function to calculate system degradation
CREATE OR REPLACE FUNCTION calculate_system_degradation(start_time TIMESTAMP, end_time TIMESTAMP)
RETURNS TABLE (
    component TEXT,
    degradation_type TEXT,
    baseline_value REAL,
    current_value REAL,
    degradation_percent REAL,
    severity TEXT
) AS $$
BEGIN
    RETURN QUERY
    WITH baseline AS (
        SELECT
            AVG(avg_trade_latency_ms) as baseline_latency,
            AVG(memory_usage_percent) as baseline_memory,
            AVG(cpu_usage_percent) as baseline_cpu,
            AVG(rpc_latency_avg_ms) as baseline_rpc_latency
        FROM evaluation_snapshots
        WHERE snapshot_time >= start_time AND snapshot_time < start_time + INTERVAL '1 hour'
    ),
    current AS (
        SELECT
            AVG(avg_trade_latency_ms) as current_latency,
            AVG(memory_usage_percent) as current_memory,
            AVG(cpu_usage_percent) as current_cpu,
            AVG(rpc_latency_avg_ms) as current_rpc_latency
        FROM evaluation_snapshots
        WHERE snapshot_time >= end_time AND snapshot_time < end_time + INTERVAL '1 hour'
    )
    SELECT
        'Trade Execution'::TEXT as component,
        'Latency Degradation'::TEXT as degradation_type,
        b.baseline_latency as baseline_value,
        c.current_latency as current_value,
        CASE WHEN b.baseline_latency > 0
            THEN ((c.current_latency - b.baseline_latency) / b.baseline_latency) * 100
            ELSE 0
        END as degradation_percent,
        CASE
            WHEN b.baseline_latency = 0 THEN 'UNKNOWN'
            WHEN ((c.current_latency - b.baseline_latency) / b.baseline_latency) > 0.5 THEN 'CRITICAL'
            WHEN ((c.current_latency - b.baseline_latency) / b.baseline_latency) > 0.2 THEN 'HIGH'
            WHEN ((c.current_latency - b.baseline_latency) / b.baseline_latency) > 0.1 THEN 'MEDIUM'
            ELSE 'LOW'
        END as severity
    FROM baseline b, current c
    WHERE b.baseline_latency IS NOT NULL AND c.current_latency IS NOT NULL

    UNION ALL

    SELECT
        'System Resources'::TEXT,
        'Memory Degradation'::TEXT,
        b.baseline_memory,
        c.current_memory,
        CASE WHEN b.baseline_memory > 0
            THEN ((c.current_memory - b.baseline_memory) / b.baseline_memory) * 100
            ELSE 0
        END,
        CASE
            WHEN b.baseline_memory = 0 THEN 'UNKNOWN'
            WHEN ((c.current_memory - b.baseline_memory) / b.baseline_memory) > 0.5 THEN 'CRITICAL'
            WHEN ((c.current_memory - b.baseline_memory) / b.baseline_memory) > 0.2 THEN 'HIGH'
            WHEN ((c.current_memory - b.baseline_memory) / b.baseline_memory) > 0.1 THEN 'MEDIUM'
            ELSE 'LOW'
        END
    FROM baseline b, current c
    WHERE b.baseline_memory IS NOT NULL AND c.current_memory IS NOT NULL

    UNION ALL

    SELECT
        'RPC Performance'::TEXT,
        'Latency Degradation'::TEXT,
        b.baseline_rpc_latency,
        c.current_rpc_latency,
        CASE WHEN b.baseline_rpc_latency > 0
            THEN ((c.current_rpc_latency - b.baseline_rpc_latency) / b.baseline_rpc_latency) * 100
            ELSE 0
        END,
        CASE
            WHEN b.baseline_rpc_latency = 0 THEN 'UNKNOWN'
            WHEN ((c.current_rpc_latency - b.baseline_rpc_latency) / b.baseline_rpc_latency) > 0.5 THEN 'CRITICAL'
            WHEN ((c.current_rpc_latency - b.baseline_rpc_latency) / b.baseline_rpc_latency) > 0.2 THEN 'HIGH'
            WHEN ((c.current_rpc_latency - b.baseline_rpc_latency) / b.baseline_rpc_latency) > 0.1 THEN 'MEDIUM'
            ELSE 'LOW'
        END
    FROM baseline b, current c
    WHERE b.baseline_rpc_latency IS NOT NULL AND c.current_rpc_latency IS NOT NULL;
END;
$$ LANGUAGE plpgsql;

-- Function to generate daily summary
CREATE OR REPLACE FUNCTION generate_daily_summary(target_day INTEGER)
RETURNS INTEGER AS $$
DECLARE
    summary_count INTEGER;
BEGIN
    -- Calculate daily aggregates from hourly snapshots
    INSERT INTO daily_evaluation_summaries (
        day_number,
        summary_date,
        total_trades,
        successful_trades,
        failed_trades,
        success_rate_percent,
        total_pnl_sol,
        avg_pnl_per_trade_sol,
        total_costs_sol,
        avg_cost_per_trade_sol,
        avg_trade_latency_ms,
        p95_trade_latency_ms,
        p99_trade_latency_ms,
        avg_rpc_latency_ms,
        max_drawdown_percent,
        avg_cpu_usage_percent,
        avg_memory_usage_percent,
        avg_disk_usage_percent,
        total_errors,
        total_warnings,
        avg_db_query_latency_ms,
        total_anomalies,
        critical_anomalies,
        warning_anomalies
    )
    SELECT
        target_day,
        (SELECT MIN(DATE(snapshot_time)) FROM evaluation_snapshots WHERE day_number = target_day),
        SUM(total_trades_today),
        SUM(successful_trades_today),
        SUM(failed_trades_today),
        CASE WHEN SUM(total_trades_today) > 0
            THEN (SUM(successful_trades_today)::FLOAT / SUM(total_trades_today)) * 100
            ELSE 0
        END,
        SUM(total_pnl_sol),
        CASE WHEN SUM(total_trades_today) > 0
            THEN SUM(total_pnl_sol) / SUM(total_trades_today)
            ELSE 0
        END,
        SUM(total_costs_sol),
        CASE WHEN SUM(total_trades_today) > 0
            THEN SUM(total_costs_sol) / SUM(total_trades_today)
            ELSE 0
        END,
        AVG(avg_trade_latency_ms),
        MAX(p95_trade_latency_ms),
        MAX(p99_trade_latency_ms),
        AVG(rpc_latency_avg_ms),
        MAX(max_drawdown_percent),
        AVG(cpu_usage_percent),
        AVG(memory_usage_percent),
        AVG(disk_usage_percent),
        SUM(error_count),
        SUM(warning_count),
        AVG(db_query_latency_avg_ms),
        (SELECT COUNT(*) FROM evaluation_anomalies WHERE day_number = target_day),
        (SELECT COUNT(*) FROM evaluation_anomalies WHERE day_number = target_day AND severity = 'CRITICAL'),
        (SELECT COUNT(*) FROM evaluation_anomalies WHERE day_number = target_day AND severity = 'WARNING')
    FROM evaluation_snapshots
    WHERE day_number = target_day
    ON CONFLICT (day_number) DO UPDATE SET
        summary_date = EXCLUDED.summary_date,
        total_trades = EXCLUDED.total_trades,
        successful_trades = EXCLUDED.successful_trades,
        failed_trades = EXCLUDED.failed_trades,
        success_rate_percent = EXCLUDED.success_rate_percent,
        total_pnl_sol = EXCLUDED.total_pnl_sol,
        avg_pnl_per_trade_sol = EXCLUDED.avg_pnl_per_trade_sol,
        total_costs_sol = EXCLUDED.total_costs_sol,
        avg_cost_per_trade_sol = EXCLUDED.avg_cost_per_trade_sol,
        avg_trade_latency_ms = EXCLUDED.avg_trade_latency_ms,
        p95_trade_latency_ms = EXCLUDED.p95_trade_latency_ms,
        p99_trade_latency_ms = EXCLUDED.p99_trade_latency_ms,
        avg_rpc_latency_ms = EXCLUDED.avg_rpc_latency_ms,
        max_drawdown_percent = EXCLUDED.max_drawdown_percent,
        avg_cpu_usage_percent = EXCLUDED.avg_cpu_usage_percent,
        avg_memory_usage_percent = EXCLUDED.avg_memory_usage_percent,
        avg_disk_usage_percent = EXCLUDED.avg_disk_usage_percent,
        total_errors = EXCLUDED.total_errors,
        total_warnings = EXCLUDED.total_warnings,
        avg_db_query_latency_ms = EXCLUDED.avg_db_query_latency_ms,
        total_anomalies = EXCLUDED.total_anomalies,
        critical_anomalies = EXCLUDED.critical_anomalies,
        warning_anomalies = EXCLUDED.warning_anomalies,
        updated_at = CURRENT_TIMESTAMP;

    GET DIAGNOSTICS summary_count = ROW_COUNT;
    RETURN summary_count;
END;
$$ LANGUAGE plpgsql;

-- ===================================================================
-- TRIGGERS - Automatic timestamp updates
-- ===================================================================

-- Update trigger function for updated_at timestamps
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply triggers to relevant tables
CREATE TRIGGER update_evaluation_snapshots_updated_at BEFORE UPDATE ON evaluation_snapshots
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_trade_execution_details_updated_at BEFORE UPDATE ON trade_execution_details
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_evaluation_anomalies_updated_at BEFORE UPDATE ON evaluation_anomalies
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_daily_evaluation_summaries_updated_at BEFORE UPDATE ON daily_evaluation_summaries
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_evaluation_incidents_updated_at BEFORE UPDATE ON evaluation_incidents
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- ===================================================================
-- VIEWS - Common query patterns for evaluation analysis
-- ===================================================================

-- View for current system status
CREATE OR REPLACE VIEW v_current_system_status AS
SELECT
    s.snapshot_time,
    s.day_number,
    s.hour_number,
    s.cpu_usage_percent,
    s.memory_usage_percent,
    s.active_positions_count,
    s.queue_depth,
    s.avg_trade_latency_ms,
    s.total_pnl_sol,
    s.circuit_breaker_state,
    (SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = FALSE) as active_anomalies,
    (SELECT COUNT(*) FROM evaluation_incidents WHERE resolution_status != 'RESOLVED') as active_incidents
FROM evaluation_snapshots s
ORDER BY s.snapshot_time DESC
LIMIT 1;

-- View for performance trends
CREATE OR REPLACE VIEW v_performance_trends AS
SELECT
    day_number,
    AVG(avg_trade_latency_ms) as avg_daily_latency,
    AVG(p95_trade_latency_ms) as avg_p95_latency,
    AVG(p99_trade_latency_ms) as avg_p99_latency,
    SUM(total_trades_today) as daily_trades,
    SUM(successful_trades_today) as daily_successful_trades,
    SUM(total_pnl_sol) as daily_pnl
FROM evaluation_snapshots
GROUP BY day_number
ORDER BY day_number;

-- View for anomaly summary
CREATE OR REPLACE VIEW v_anomaly_summary AS
SELECT
    day_number,
    anomaly_type,
    severity,
    COUNT(*) as anomaly_count,
    COUNT(DISTINCT metric_name) as affected_metrics_count,
    AVG(deviation_percent) as avg_deviation_percent
FROM evaluation_anomalies
GROUP BY day_number, anomaly_type, severity
ORDER BY day_number, anomaly_type, severity;

-- ===================================================================
-- INITIAL DATA SETUP
-- ===================================================================

-- Create evaluation periods (10 days)
INSERT INTO evaluation_periods (day_number, start_time, end_time, status)
SELECT
    generate_series(1, 10) as day_number,
    CURRENT_DATE + (generate_series(1, 10) - 1) * INTERVAL '1 day' as start_time,
    CURRENT_DATE + (generate_series(1, 10)) * INTERVAL '1 day' as end_time,
    'PENDING' as status
ON CONFLICT (day_number) DO NOTHING;

-- Grant permissions (adjust as needed for your setup)
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO chimera_user;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO chimera_user;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO chimera_user;

-- ===================================================================
-- INDEX OPTIMIZATION FOR EVALUATION QUERIES
-- ===================================================================

-- Composite indexes for common evaluation query patterns
CREATE INDEX idx_eval_snapshot_performance_trend ON evaluation_snapshots(day_number, avg_trade_latency_ms, total_pnl_sol);
CREATE INDEX idx_anomaly_investigation ON evaluation_anomalies(severity, resolved, anomaly_time);
CREATE INDEX idx_trade_details_analysis ON trade_execution_details(trade_uuid, total_execution_latency_us, total_cost_sol);
CREATE INDEX idx_incident_analysis ON evaluation_incidents(incident_time, severity, resolution_status);

-- Partial indexes for filtered queries
CREATE INDEX idx_active_anomalies ON evaluation_anomalies(anomaly_time) WHERE resolved = FALSE;
CREATE INDEX idx_critical_incidents ON evaluation_incidents(incident_time) WHERE severity = 'CRITICAL';
CREATE INDEX idx_failed_trades ON trade_execution_details(signal_received_time) WHERE status = 'FAILED';

-- ===================================================================
-- COMMENTS - Documentation for database objects
-- ===================================================================

COMMENT ON TABLE evaluation_snapshots IS 'Hourly system state snapshots capturing comprehensive metrics for 10-day evaluation';
COMMENT ON TABLE trade_execution_details IS 'Detailed performance tracking for individual trades with latency breakdown and cost analysis';
COMMENT ON TABLE system_resources IS 'Container resource monitoring for CPU, memory, disk, and network metrics';
COMMENT ON TABLE evaluation_anomalies IS 'Real-time anomaly detection and tracking with alert status';
COMMENT ON TABLE signal_replay_log IS 'Historical signal replay tracking for controlled testing scenarios';
COMMENT ON TABLE daily_evaluation_summaries IS 'Pre-computed daily aggregates for efficient evaluation reporting';
COMMENT ON TABLE evaluation_incidents IS 'Significant event tracking with root cause analysis and resolution tracking';

COMMENT ON FUNCTION calculate_performance_trend(INTEGER) IS 'Calculate performance trends comparing current metrics to N-day baseline';
COMMENT ON FUNCTION calculate_system_degradation(TIMESTAMP, TIMESTAMP) IS 'Calculate system component degradation between two time periods';
COMMENT ON FUNCTION generate_daily_summary(INTEGER) IS 'Generate daily summary aggregates from hourly snapshots';

COMMENT ON VIEW v_current_system_status IS 'Real-time view of current system status including active anomalies and incidents';
COMMENT ON VIEW v_performance_trends IS 'Daily performance trends for latency, trades, and PnL analysis';
COMMENT ON VIEW v_anomaly_summary IS 'Anomaly summary grouped by day, type, and severity';

-- ===================================================================
-- END OF EVALUATION SCHEMA
-- ===================================================================