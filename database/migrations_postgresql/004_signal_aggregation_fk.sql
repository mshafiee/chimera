-- Migration: Add signal aggregation foreign key support
-- Date: 2024-12-15
-- Description: Adds foreign key constraint for signal aggregation

-- Drop and recreate with proper foreign key
ALTER TABLE signal_aggregation DROP CONSTRAINT IF EXISTS signal_aggregation_wallet_address_fkey;
ALTER TABLE signal_aggregation ADD CONSTRAINT signal_aggregation_wallet_address_fkey
    FOREIGN KEY (wallet_address) REFERENCES wallets(address) ON DELETE CASCADE;

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('004', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;
