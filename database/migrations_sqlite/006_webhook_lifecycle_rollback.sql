-- ===================================================
-- Webhook Lifecycle Management Schema Rollback
-- Version: 1.0
-- Date: 2025-06-20
-- Description: Rollback script for webhook lifecycle extensions
-- WARNING: This will delete webhook lifecycle tracking data
-- ===================================================

BEGIN IMMEDIATE;

-- Drop audit table indexes first
DROP INDEX IF EXISTS idx_webhook_lifecycle_audit_status;
DROP INDEX IF EXISTS idx_webhook_lifecycle_audit_action;
DROP INDEX IF EXISTS idx_webhook_lifecycle_audit_wallet;

-- Drop audit table
DROP TABLE IF EXISTS webhook_lifecycle_audit;

-- Drop configuration table
DROP TABLE IF EXISTS webhook_configuration;

-- Drop wallet_monitoring indexes
DROP INDEX IF EXISTS idx_wallet_monitoring_webhook_status;
DROP INDEX IF EXISTS idx_wallet_monitoring_health_check;
DROP INDEX IF EXISTS idx_wallet_monitoring_helius_webhook_id;

-- Rebuild wallet_monitoring without the webhook lifecycle columns so the
-- schema is fully restored to its pre-005 state and migration 005 can be
-- re-applied (SQLite ALTER TABLE ADD COLUMN has no IF NOT EXISTS guard).
CREATE TABLE wallet_monitoring_new (
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

INSERT INTO wallet_monitoring_new (wallet_address, helius_webhook_id, rpc_polling_active, last_transaction_signature, last_monitored_at, monitoring_enabled, created_at, updated_at)
SELECT wallet_address, helius_webhook_id, rpc_polling_active, last_transaction_signature, last_monitored_at, monitoring_enabled, created_at, updated_at
FROM wallet_monitoring;

DROP TABLE wallet_monitoring;
ALTER TABLE wallet_monitoring_new RENAME TO wallet_monitoring;

CREATE INDEX idx_wallet_monitoring_enabled
    ON wallet_monitoring(monitoring_enabled) WHERE monitoring_enabled = 1;

COMMIT;

-- ===================================================
-- Rollback complete
-- ===================================================