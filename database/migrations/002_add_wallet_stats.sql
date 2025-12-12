-- Add Scout-derived wallet stats to wallets table
-- These columns are optional and may be NULL for older rows.

ALTER TABLE wallets ADD COLUMN avg_win_sol REAL;
ALTER TABLE wallets ADD COLUMN avg_loss_sol REAL;
ALTER TABLE wallets ADD COLUMN profit_factor REAL;
ALTER TABLE wallets ADD COLUMN realized_pnl_30d_sol REAL;

