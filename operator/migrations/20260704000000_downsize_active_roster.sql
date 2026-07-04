-- Migration: Aggressive Wallet Roster Downsizing
-- Description: Demote ACTIVE wallets with WQS < 75 to CANDIDATE status
-- Date: 2026-07-04

-- Create backup table for safe rollback
DROP TABLE IF EXISTS wallets_backup_20260704;
CREATE TABLE wallets_backup_20260704 AS SELECT * FROM wallets;

-- Update ACTIVE wallets with WQS < 75 to CANDIDATE
UPDATE wallets
SET
    status = 'CANDIDATE',
    notes = CASE
        WHEN notes IS NULL THEN 'Auto-demoted: WQS < 75 (roster downsizing 2026-07-04)'
        ELSE notes || '; Auto-demoted: WQS < 75 (roster downsizing 2026-07-04)'
    END,
    updated_at = datetime('now')
WHERE
    status = 'ACTIVE'
    AND wqs_score < 75.0;
