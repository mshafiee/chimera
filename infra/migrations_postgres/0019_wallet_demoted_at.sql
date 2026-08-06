-- Demotion timestamp: set whenever a wallet is demoted to CANDIDATE.
-- The Dune promotion gate ignores wallets demoted within the cooldown
-- window, preventing the churn loop where shadow-quality demotes a wallet
-- (recent 48h signals under our exits) and Dune re-promotes it minutes
-- later on historical 7d PnL (observed live 2026-08-05: demote 15:32:44,
-- re-promote 15:33:37).
ALTER TABLE wallets ADD COLUMN IF NOT EXISTS demoted_at TIMESTAMPTZ;
