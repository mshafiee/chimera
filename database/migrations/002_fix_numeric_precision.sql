-- Migration: Fix integer-precision NUMERIC columns truncating fractional values to 0
--
-- ROOT CAUSE: The original schema declared financial columns as NUMERIC(30),
-- which PostgreSQL interprets as NUMERIC(30,0) -- i.e. ZERO decimal places.
-- Every fractional value was silently rounded to 0:
--   amount_sol 0.02      -> 0
--   entry_price 0.0000028 -> 0
--   jito_tip_sol 0.002   -> 0
-- This made all trade sizing, pricing, costs, and PnL invisible in the DB,
-- blocking position closes (token_amount looked NULL/0) and any profitability
-- reporting. Only integer-valued fields (e.g. entry_sol_price_usd=76) survived.
--
-- FIX: widen every affected column to NUMERIC(30,18) (18 decimal places covers
-- lamport-level SOL, sub-cent token prices, and micro-percentages). This is a
-- widening change -- safe, no data loss beyond values already truncated to 0.

-- trades: sizing, pricing, costs, PnL
ALTER TABLE trades ALTER COLUMN amount_sol TYPE NUMERIC(30,18) USING amount_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN price_at_signal TYPE NUMERIC(30,18) USING price_at_signal::numeric(30,18);
ALTER TABLE trades ALTER COLUMN pnl_sol TYPE NUMERIC(30,18) USING pnl_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN pnl_usd TYPE NUMERIC(30,18) USING pnl_usd::numeric(30,18);
ALTER TABLE trades ALTER COLUMN jito_tip_sol TYPE NUMERIC(30,18) USING jito_tip_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN dex_fee_sol TYPE NUMERIC(30,18) USING dex_fee_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN slippage_cost_sol TYPE NUMERIC(30,18) USING slippage_cost_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN total_cost_sol TYPE NUMERIC(30,18) USING total_cost_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN network_fee_sol TYPE NUMERIC(30,18) USING network_fee_sol::numeric(30,18);
ALTER TABLE trades ALTER COLUMN net_pnl_sol TYPE NUMERIC(30,18) USING net_pnl_sol::numeric(30,18);

-- positions: entry/exit pricing, PnL, token amount
ALTER TABLE positions ALTER COLUMN entry_amount_sol TYPE NUMERIC(30,18) USING entry_amount_sol::numeric(30,18);
ALTER TABLE positions ALTER COLUMN entry_price TYPE NUMERIC(30,18) USING entry_price::numeric(30,18);
ALTER TABLE positions ALTER COLUMN current_price TYPE NUMERIC(30,18) USING current_price::numeric(30,18);
ALTER TABLE positions ALTER COLUMN unrealized_pnl_sol TYPE NUMERIC(30,18) USING unrealized_pnl_sol::numeric(30,18);
ALTER TABLE positions ALTER COLUMN unrealized_pnl_percent TYPE NUMERIC(20,10) USING unrealized_pnl_percent::numeric(20,10);
ALTER TABLE positions ALTER COLUMN exit_price TYPE NUMERIC(30,18) USING exit_price::numeric(30,18);
ALTER TABLE positions ALTER COLUMN realized_pnl_sol TYPE NUMERIC(30,18) USING realized_pnl_sol::numeric(30,18);
ALTER TABLE positions ALTER COLUMN realized_pnl_usd TYPE NUMERIC(30,18) USING realized_pnl_usd::numeric(30,18);
ALTER TABLE positions ALTER COLUMN realized_net_pnl_sol TYPE NUMERIC(30,18) USING realized_net_pnl_sol::numeric(30,18);
ALTER TABLE positions ALTER COLUMN entry_sol_price_usd TYPE NUMERIC(30,18) USING entry_sol_price_usd::numeric(30,18);
ALTER TABLE positions ALTER COLUMN token_amount TYPE NUMERIC(30,18) USING token_amount::numeric(30,18);

-- exit_targets: entry/stop/peak pricing, fractions
ALTER TABLE exit_targets ALTER COLUMN entry_price TYPE NUMERIC(30,18) USING entry_price::numeric(30,18);
ALTER TABLE exit_targets ALTER COLUMN entry_amount_sol TYPE NUMERIC(30,18) USING entry_amount_sol::numeric(30,18);
ALTER TABLE exit_targets ALTER COLUMN trailing_stop_price TYPE NUMERIC(30,18) USING trailing_stop_price::numeric(30,18);
ALTER TABLE exit_targets ALTER COLUMN peak_price TYPE NUMERIC(30,18) USING peak_price::numeric(30,18);
ALTER TABLE exit_targets ALTER COLUMN peak_profit_percent TYPE NUMERIC(20,10) USING peak_profit_percent::numeric(20,10);
ALTER TABLE exit_targets ALTER COLUMN stop_loss_price TYPE NUMERIC(30,18) USING stop_loss_price::numeric(30,18);
ALTER TABLE exit_targets ALTER COLUMN remaining_fraction TYPE NUMERIC(10,8) USING remaining_fraction::numeric(10,8);

-- wallets: ROI, PnL, sizing stats
ALTER TABLE wallets ALTER COLUMN roi_7d TYPE NUMERIC(20,10) USING roi_7d::numeric(20,10);
ALTER TABLE wallets ALTER COLUMN roi_30d TYPE NUMERIC(20,10) USING roi_30d::numeric(20,10);
ALTER TABLE wallets ALTER COLUMN max_drawdown_30d TYPE NUMERIC(20,10) USING max_drawdown_30d::numeric(20,10);
ALTER TABLE wallets ALTER COLUMN avg_trade_size_sol TYPE NUMERIC(30,18) USING avg_trade_size_sol::numeric(30,18);
ALTER TABLE wallets ALTER COLUMN avg_win_sol TYPE NUMERIC(30,18) USING avg_win_sol::numeric(30,18);
ALTER TABLE wallets ALTER COLUMN avg_loss_sol TYPE NUMERIC(30,18) USING avg_loss_sol::numeric(30,18);
ALTER TABLE wallets ALTER COLUMN profit_factor TYPE NUMERIC(20,10) USING profit_factor::numeric(20,10);
ALTER TABLE wallets ALTER COLUMN realized_pnl_30d_sol TYPE NUMERIC(30,18) USING realized_pnl_30d_sol::numeric(30,18);

-- wallet_copy_performance
ALTER TABLE wallet_copy_performance ALTER COLUMN copy_pnl_7d TYPE NUMERIC(30,18) USING copy_pnl_7d::numeric(30,18);
ALTER TABLE wallet_copy_performance ALTER COLUMN copy_pnl_30d TYPE NUMERIC(30,18) USING copy_pnl_30d::numeric(30,18);
ALTER TABLE wallet_copy_performance ALTER COLUMN signal_success_rate TYPE NUMERIC(10,6) USING signal_success_rate::numeric(10,6);
ALTER TABLE wallet_copy_performance ALTER COLUMN avg_return_per_trade TYPE NUMERIC(20,10) USING avg_return_per_trade::numeric(20,10);

-- signal_aggregation
ALTER TABLE signal_aggregation ALTER COLUMN amount_sol TYPE NUMERIC(30,18) USING amount_sol::numeric(30,18);

-- jito_tip_history
ALTER TABLE jito_tip_history ALTER COLUMN tip_amount_sol TYPE NUMERIC(30,18) USING tip_amount_sol::numeric(30,18);

-- wqs_pnl_correlation
ALTER TABLE wqs_pnl_correlation ALTER COLUMN wqs_score_at_promotion TYPE NUMERIC(20,10) USING wqs_score_at_promotion::numeric(20,10);
ALTER TABLE wqs_pnl_correlation ALTER COLUMN actual_copy_pnl_7d_sol TYPE NUMERIC(30,18) USING actual_copy_pnl_7d_sol::numeric(30,18);
ALTER TABLE wqs_pnl_correlation ALTER COLUMN actual_copy_pnl_30d_sol TYPE NUMERIC(30,18) USING actual_copy_pnl_30d_sol::numeric(30,18);
ALTER TABLE wqs_pnl_correlation ALTER COLUMN actual_copy_pnl_all_sol TYPE NUMERIC(30,18) USING actual_copy_pnl_all_sol::numeric(30,18);

-- historical_liquidity
ALTER TABLE historical_liquidity ALTER COLUMN liquidity_usd TYPE NUMERIC(30,18) USING liquidity_usd::numeric(30,18);
ALTER TABLE historical_liquidity ALTER COLUMN price_usd TYPE NUMERIC(30,18) USING price_usd::numeric(30,18);
ALTER TABLE historical_liquidity ALTER COLUMN volume_24h_usd TYPE NUMERIC(30,18) USING volume_24h_usd::numeric(30,18);
