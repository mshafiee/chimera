-- Migration: Add cost tracking columns to trades table
-- Date: 2024-12-06
-- Description: Adds columns to track Jito tips, DEX fees, slippage, and net PnL

-- Add cost tracking columns
ALTER TABLE trades ADD COLUMN IF NOT EXISTS jito_tip_sol NUMERIC(30,6) DEFAULT 0;
ALTER TABLE trades ADD COLUMN IF NOT EXISTS dex_fee_sol NUMERIC(30,6) DEFAULT 0;
ALTER TABLE trades ADD COLUMN IF NOT EXISTS slippage_cost_sol NUMERIC(30,6) DEFAULT 0;
ALTER TABLE trades ADD COLUMN IF NOT EXISTS total_cost_sol NUMERIC(30,6) DEFAULT 0;
ALTER TABLE trades ADD COLUMN IF NOT EXISTS net_pnl_sol NUMERIC(30,6);

-- Add index for cost analysis queries
CREATE INDEX IF NOT EXISTS idx_trades_costs ON trades(total_cost_sol) WHERE total_cost_sol > 0;

-- Add index for net PnL analysis
CREATE INDEX IF NOT EXISTS idx_trades_net_pnl ON trades(net_pnl_sol) WHERE net_pnl_sol IS NOT NULL;

-- Track this migration as applied
INSERT INTO schema_migrations (version, applied_at) VALUES ('001', CURRENT_TIMESTAMP)
ON CONFLICT (version) DO NOTHING;
