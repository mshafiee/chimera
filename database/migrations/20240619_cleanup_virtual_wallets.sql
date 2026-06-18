-- Migration: Clean up virtual wallets from wallets table
-- Date: 2024-06-19
-- Description: Removes any TG_ prefixed virtual wallet addresses that may have been
--              created during testing or previous implementation attempts.
--              This keeps the wallets table clean for real on-chain wallet addresses only.

-- Remove any TG_ prefixed virtual wallets
-- This is safe to run even if no virtual wallets exist
DELETE FROM wallets WHERE address LIKE 'TG_%';

-- Verify cleanup
-- The following query should return 0 after running this migration:
-- SELECT COUNT(*) FROM wallets WHERE address LIKE 'TG_%';
