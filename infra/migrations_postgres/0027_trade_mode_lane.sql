-- Hydra Phase B: trade-mode lane discriminator for the Day-14 dust cohort.
--
-- Stamps the execution lane (PAPER | LIVE | DEVNET | DUST_LIVE, Display
-- spelling) on every decision record and trade so the dust verdict can
-- scope its sample without run_id bookkeeping. Pre-0027 rows read back
-- as 'PAPER' (the only mode that existed in production before Hydra dust).
-- Forward-only; guarded for idempotent re-apply.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'decision_records' AND column_name = 'trade_mode'
    ) THEN
        ALTER TABLE decision_records
            ADD COLUMN trade_mode TEXT NOT NULL DEFAULT 'PAPER';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'trades' AND column_name = 'trade_mode'
    ) THEN
        ALTER TABLE trades
            ADD COLUMN trade_mode TEXT NOT NULL DEFAULT 'PAPER';
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_decision_records_trade_mode
    ON decision_records (trade_mode, decided_at DESC);
CREATE INDEX IF NOT EXISTS idx_trades_trade_mode
    ON trades (trade_mode, closed_at DESC);
