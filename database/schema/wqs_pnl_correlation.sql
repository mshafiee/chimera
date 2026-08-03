-- WQS-to-Actual-PnL correlation table (Phase 3a)
-- Written by the Rust Operator when it closes copy-trade positions,
-- read by the Python Scout to compute WQS predictive power.
-- Financial PnL values stored as TEXT (Decimal strings).
--
-- One row per wallet by design: this table is upserted (INSERT OR REPLACE on
-- wallet_address) and intentionally keeps only the latest promotion snapshot.
-- Full promotion history is preserved in promotion_episodes instead.

CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
    wallet_address TEXT PRIMARY KEY,
    wqs_score_at_promotion REAL NOT NULL
        CHECK (wqs_score_at_promotion BETWEEN 0 AND 100),
    actual_copy_pnl_7d_sol TEXT
        CHECK (actual_copy_pnl_7d_sol IS NULL OR actual_copy_pnl_7d_sol = CAST(actual_copy_pnl_7d_sol AS NUMERIC)),
    actual_copy_pnl_30d_sol TEXT
        CHECK (actual_copy_pnl_30d_sol IS NULL OR actual_copy_pnl_30d_sol = CAST(actual_copy_pnl_30d_sol AS NUMERIC)),
    actual_copy_pnl_all_sol TEXT
        CHECK (actual_copy_pnl_all_sol IS NULL OR actual_copy_pnl_all_sol = CAST(actual_copy_pnl_all_sol AS NUMERIC)),
    copy_trade_count_7d INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_7d >= 0),
    copy_trade_count_30d INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_30d >= 0),
    copy_trade_count_all INTEGER NOT NULL DEFAULT 0 CHECK (copy_trade_count_all >= 0),
    strategy TEXT NOT NULL DEFAULT 'SHIELD'
        CHECK(strategy IN ('SHIELD', 'SPEAR')),
    wqs_components_json TEXT CHECK (wqs_components_json IS NULL OR json_valid(wqs_components_json)),  -- JSON blob of component scores at promotion time
    promoted_at TEXT NOT NULL,
    last_updated_at TEXT NOT NULL
        CHECK (last_updated_at >= promoted_at)
);
