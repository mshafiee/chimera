-- Add Scout-derived wallet stats to wallets table
-- These columns are optional and may be NULL for older rows.

ALTER TABLE wallets ADD COLUMN IF NOT EXISTS avg_win_sol NUMERIC(30,6);
ALTER TABLE wallets ADD COLUMN IF NOT EXISTS avg_loss_sol NUMERIC(30,6);
ALTER TABLE wallets ADD COLUMN IF NOT EXISTS profit_factor NUMERIC(10,4);
ALTER TABLE wallets ADD COLUMN IF NOT EXISTS realized_pnl_30d_sol NUMERIC(30,6);

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('002', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;
