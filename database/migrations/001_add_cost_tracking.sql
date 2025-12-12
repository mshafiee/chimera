-- Migration: Add cost tracking columns to trades table
-- Date: 2024-12-06
-- Description: Adds columns to track Jito tips, DEX fees, slippage, and net PnL

-- Add cost tracking columns
ALTER TABLE trades ADD COLUMN jito_tip_sol REAL DEFAULT 0;
ALTER TABLE trades ADD COLUMN dex_fee_sol REAL DEFAULT 0;
ALTER TABLE trades ADD COLUMN slippage_cost_sol REAL DEFAULT 0;
ALTER TABLE trades ADD COLUMN total_cost_sol REAL DEFAULT 0;
ALTER TABLE trades ADD COLUMN net_pnl_sol REAL;  -- PnL after all costs

-- Add index for cost analysis queries
CREATE INDEX IF NOT EXISTS idx_trades_costs ON trades(total_cost_sol) WHERE total_cost_sol > 0;

-- Add index for net PnL analysis
CREATE INDEX IF NOT EXISTS idx_trades_net_pnl ON trades(net_pnl_sol) WHERE net_pnl_sol IS NOT NULL;




