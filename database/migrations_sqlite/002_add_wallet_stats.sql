-- Add Scout-derived wallet stats to wallets table
-- These columns are optional and may be NULL for older rows.

ALTER TABLE wallets ADD COLUMN avg_win_sol TEXT;
ALTER TABLE wallets ADD COLUMN avg_loss_sol TEXT;
ALTER TABLE wallets ADD COLUMN profit_factor TEXT;
ALTER TABLE wallets ADD COLUMN realized_pnl_30d_sol TEXT;


-- Track this migration as applied
INSERT OR IGNORE INTO schema_migrations (version) VALUES ('002');
