-- 0006_trade_statuses.sql
-- Extend trades.status CHECK with statuses used by the accounting/lifecycle
-- remediation:
--   REJECTED              — orphan SELL with no position (deployed earlier),
--                           duplicate BUY rejected at pre-execution admission
--   PENDING_CONFIRMATION  — submitted BUY not yet confirmed on-chain; recovery
--                           reconciliation finalizes (opens) or fails it
--
-- Without this, update_trade_status(REJECTED | PENDING_CONFIRMATION) violates
-- trades_status_check and the rejection is silently lost (trade stays in its
-- prior status). Verified live on production 2026-07-24.

ALTER TABLE trades DROP CONSTRAINT IF EXISTS trades_status_check;

ALTER TABLE trades ADD CONSTRAINT trades_status_check
    CHECK (status IN (
        'PENDING',
        'QUEUED',
        'EXECUTING',
        'ACTIVE',
        'EXITING',
        'CLOSED',
        'FAILED',
        'RETRY',
        'DEAD_LETTER',
        'REJECTED',
        'PENDING_CONFIRMATION'
    ));

-- =============================================================================
-- Table comments (moved from 0001 — they must run AFTER their tables exist)
-- =============================================================================

COMMENT ON TABLE schema_migrations IS 'Schema migration tracking (idempotent guard for migration files)';
COMMENT ON TABLE trades IS 'Primary record of all trading signals received';
COMMENT ON TABLE positions IS 'Active positions being tracked';
COMMENT ON TABLE wallets IS 'Tracked wallets with WQS scores (managed by Scout)';
COMMENT ON TABLE dead_letter_queue IS 'Failed operations for analysis/retry';
COMMENT ON TABLE config_audit IS 'Track all configuration changes';
COMMENT ON TABLE kill_switch_state IS 'Single-row table written synchronously before returning from kill-switch API handler';
COMMENT ON TABLE circuit_breaker_state IS 'Single-row table read on startup to restore circuit breaker state';
COMMENT ON TABLE admin_wallets IS 'Authorization for API access';
COMMENT ON TABLE jito_tip_history IS 'For dynamic tip calculation (cold start persistence)';
COMMENT ON TABLE reconciliation_log IS 'Compare DB state vs on-chain state';
COMMENT ON TABLE backups IS 'Backups tracking';
COMMENT ON TABLE historical_liquidity IS 'Historical liquidity data for backtesting and validation';
COMMENT ON TABLE wallet_monitoring IS 'Track webhook subscriptions and polling state';
COMMENT ON TABLE exit_targets IS 'Position-level profit targets and stops';
COMMENT ON TABLE signal_aggregation IS 'Multi-wallet signal tracking';
COMMENT ON TABLE wallet_copy_performance IS 'Per-wallet copy trading metrics';
COMMENT ON TABLE rate_limit_metrics IS 'Credit usage and rate tracking';
COMMENT ON TABLE webhook_lifecycle_audit IS 'Track webhook registration and health';
COMMENT ON TABLE webhook_configuration IS 'Track configuration changes for URL change detection';
COMMENT ON TABLE wqs_pnl_correlation IS 'WQS-to-PnL correlation for predictive power analysis';
