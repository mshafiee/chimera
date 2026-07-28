-- Add inactivity tracking columns to wallet_monitoring for Phase 1 wallet rotation
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS last_speculative_signal_at TIMESTAMPTZ;
ALTER TABLE wallet_monitoring ADD COLUMN IF NOT EXISTS inactivity_demotion_count INTEGER NOT NULL DEFAULT 0;