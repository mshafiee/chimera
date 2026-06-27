-- Add network_fee_sol to trades ( Decimal, default '0')
ALTER TABLE trades ADD COLUMN network_fee_sol TEXT DEFAULT '0';

-- Add realized_net_pnl_sol to positions (accumulated net PnL after cost deduction)
ALTER TABLE positions ADD COLUMN realized_net_pnl_sol TEXT;

-- Add token_amount to positions (on-chain token balance at entry)
ALTER TABLE positions ADD COLUMN token_amount TEXT;
