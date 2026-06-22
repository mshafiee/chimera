-- ===================================================
-- Webhook Lifecycle Management Schema Extension
-- Version: 1.0
-- Date: 2025-06-20
-- Description: Adds webhook lifecycle tracking, health monitoring,
--              and audit logging capabilities to the Chimera database
-- ===================================================

-- Add webhook lifecycle tracking columns to existing wallet_monitoring table
-- These columns track webhook registration status, health, and retry attempts

ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS webhook_status TEXT DEFAULT 'active' CHECK(webhook_status IN ('active', 'paused', 'failed', 'orphaned'));
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS webhook_registered_at TIMESTAMPTZ;
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS webhook_last_health_check TIMESTAMPTZ;
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS webhook_health_status TEXT DEFAULT 'unknown' CHECK(webhook_health_status IN ('healthy', 'unhealthy', 'unknown', 'timeout', 'error'));
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS registration_attempts INTEGER DEFAULT 0;
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS last_registration_error TEXT;
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS last_updated_url TEXT;

-- Create indexes for lifecycle queries to optimize performance
-- Partial indexes are used for better performance on common queries

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_webhook_status
    ON wallet_monitoring(webhook_status) WHERE webhook_status = 'active';

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_health_check
    ON wallet_monitoring(webhook_last_health_check);

CREATE INDEX IF NOT EXISTS idx_wallet_monitoring_helius_webhook_id
    ON wallet_monitoring(helius_webhook_id) WHERE helius_webhook_id IS NOT NULL;

-- Create webhook lifecycle audit table for comprehensive tracking
-- This table logs all webhook operations with timestamps and details

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

-- Create indexes for audit table queries

CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_wallet
    ON webhook_lifecycle_audit(wallet_address, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_action
    ON webhook_lifecycle_audit(action, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_lifecycle_audit_status
    ON webhook_lifecycle_audit(status, created_at DESC);

-- Create webhook configuration tracking table for URL change detection
-- This table tracks configuration changes to detect when webhook URLs change

CREATE TABLE IF NOT EXISTS webhook_configuration (
    id              BIGSERIAL PRIMARY KEY,
    config_key      TEXT UNIQUE NOT NULL,
    config_value     TEXT NOT NULL,
    last_updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_by      TEXT DEFAULT 'system'
);

-- Insert current webhook URL configuration if it exists
INSERT INTO webhook_configuration (config_key, config_value, updated_by)
SELECT 'current_webhook_url',
       (SELECT value FROM config WHERE key = 'helius_webhook_url'),
       'migration'
WHERE EXISTS (SELECT 1 FROM config WHERE key = 'helius_webhook_url')
ON CONFLICT (config_key) DO NOTHING;

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('005', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;

-- ===================================================
-- Migration complete
-- Total new columns: 7
-- Total new tables: 2
-- Total new indexes: 6
-- ===================================================
