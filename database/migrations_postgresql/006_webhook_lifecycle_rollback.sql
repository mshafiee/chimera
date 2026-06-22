-- ===================================================
-- Webhook Lifecycle Management Schema Rollback
-- Version: 1.0
-- Date: 2025-06-20
-- Description: Rollback script for webhook lifecycle extensions
-- WARNING: This will delete webhook lifecycle tracking data
-- ===================================================

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

-- PostgreSQL supports dropping columns (unlike SQLite)
-- Remove the webhook lifecycle columns from wallet_monitoring table
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS webhook_status;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS webhook_registered_at;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS webhook_last_health_check;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS webhook_health_status;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS registration_attempts;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS last_registration_error;
ALTER TABLE wallet_monitoring DROP COLUMN IF EXISTS last_updated_url;

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('006', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;

-- ===================================================
-- Rollback complete
-- All webhook lifecycle tables and columns have been removed
-- ===================================================
