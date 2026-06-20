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

-- Note: SQLite doesn't support DROP COLUMN directly
-- The following columns will remain in the schema but won't be used:
-- - webhook_status
-- - webhook_registered_at
-- - webhook_last_health_check
-- - webhook_health_status
-- - registration_attempts
-- - last_registration_error
-- - last_updated_url
--
-- To completely remove these columns, you would need to:
-- 1. Create a new wallet_monitoring table without these columns
-- 2. Copy existing data (excluding the new columns) to the new table
-- 3. Drop the old table
-- 4. Rename the new table to wallet_monitoring
-- 5. Recreate indexes and foreign keys

-- ===================================================
-- Rollback complete
-- Note: Some columns may remain in schema but won't be used
-- ===================================================