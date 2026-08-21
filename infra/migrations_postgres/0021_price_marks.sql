-- Per-position price-mark series (Phase 2E) — instrument the operator monitor
-- to record a price-history for each open position.
--
-- Problem: the DB previously recorded only entry/exit snapshots (no price
-- history), so the realize-vs-price gap (62.4% predicted vs 18% realized win)
-- could not be tuned — `fill_skew_report` ran on n=4 real sells, and a deferral
-- grid-search had no marks to defer against. This table fixes the *forward*
-- gap: the position monitor appends the price-cache USD mark for every open
-- position each evaluation tick (~1s young / 5s steady-state), so realized
-- gap and smart-exit (deferral) params become tunable on recorded marks.

CREATE TABLE IF NOT EXISTS position_price_marks (
    id            BIGSERIAL PRIMARY KEY,
    trade_uuid    TEXT NOT NULL,
    token_address TEXT NOT NULL,
    ts_unix       BIGINT NOT NULL,
    price_usd     NUMERIC(30,18) NOT NULL,
    source        TEXT NOT NULL DEFAULT 'monitor',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_position_price_marks_trade_ts
    ON position_price_marks (trade_uuid, ts_unix);

CREATE INDEX IF NOT EXISTS idx_position_price_marks_token_ts
    ON position_price_marks (token_address, ts_unix);
