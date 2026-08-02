-- Mark-to-market NAV time series for the dashboard equity curve.
--
-- Written every ~60s by the operator's NAV snapshot task. NAV is computed
-- consistently with the circuit-breaker / portfolio-risk accounting:
--     nav_sol = total_capital_sol + realized_pnl_sol + unrealized_pnl_sol
-- where realized/unrealized come from the positions table (CLOSED / ACTIVE).
--
-- Retention: the snapshot task purges rows older than 90 days; the DESC index
-- keeps the dashboard history scan cheap.

CREATE TABLE IF NOT EXISTS portfolio_snapshots (
    id                 BIGSERIAL    PRIMARY KEY,
    recorded_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    nav_sol            NUMERIC(30,18) NOT NULL,
    capital_sol        NUMERIC(30,18) NOT NULL,
    realized_pnl_sol   NUMERIC(30,18) NOT NULL,
    unrealized_pnl_sol NUMERIC(30,18) NOT NULL,
    open_positions     INTEGER      NOT NULL DEFAULT 0,
    sol_price_usd      NUMERIC(30,18),
    trade_mode         TEXT
);

CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_recorded_at
    ON portfolio_snapshots (recorded_at DESC);
