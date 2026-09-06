-- Durable pre-admission signal recording (Gate 0, 2026-09-06).
--
-- Root cause being fixed: the in-memory SignalAggregator only counted
-- admitted-signal consensus in a 5-minute window, and with a 5-wallet ACTIVE
-- roster, decision_records.consensus_wallet_count was NULL for 178,307 of
-- 180,738 decisions. Multi-wallet cluster behaviour was therefore
-- unobservable, and the held cluster plan's §1.2 statistics could never have
-- been produced by this pipeline.
--
-- This table records EVERY parsed swap of a tracked wallet (ACTIVE/PROVING)
-- the moment it is parsed, BEFORE circuit breaker / selection / admission.
-- Bilateral (BUY + SELL) so disinvestment and cluster exit logic can later
-- consume it. price_usd/price_sol are nullable: recording must never fail
-- because ancillary pricing data was unavailable.
--
-- token_decimals records the mint decimals at parse time so consumers can
-- normalize raw-token amounts to UI units (audit finding: parser mixes
-- uiAmount and raw token_amount paths — normalized via decimals, never
-- re-derived later).

CREATE TABLE IF NOT EXISTS smart_money_signals (
    id BIGSERIAL PRIMARY KEY,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    token_symbol TEXT,
    side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
    amount_sol NUMERIC(30, 18) NOT NULL,
    amount_tokens NUMERIC(38, 18) NOT NULL,
    token_decimals INT,
    price_usd NUMERIC(30, 18),
    price_sol NUMERIC(30, 18),
    tx_signature TEXT NOT NULL,
    slot BIGINT NOT NULL,
    block_time TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_smart_signal_tx_token_side UNIQUE (tx_signature, token_address, side)
);

-- Window scan pattern: token + side + time (cluster trigger + revalidation).
CREATE INDEX IF NOT EXISTS idx_smart_signals_token_window
    ON smart_money_signals (token_address, side, block_time DESC)
    INCLUDE (wallet_address, amount_sol, amount_tokens, price_sol);

-- Per-wallet history (future roster analytics).
CREATE INDEX IF NOT EXISTS idx_smart_signals_wallet_time
    ON smart_money_signals (wallet_address, block_time DESC);
