-- Widen smart_money_signals numeric columns (2026-09-06).
--
-- 0023's NUMERIC(38,18) allows only 20 integer digits for amount_tokens, but
-- the parser's raw-unit paths (token_amount strings, not uiAmount) can carry
-- raw token quantities of 10^21+ for some mints — production hit
-- "numeric field overflow" within minutes of deploy. Plain NUMERIC is
-- unbounded; token_decimals records the unit scale so consumers normalize
-- explicitly.

ALTER TABLE smart_money_signals ALTER COLUMN amount_sol TYPE NUMERIC;
ALTER TABLE smart_money_signals ALTER COLUMN amount_tokens TYPE NUMERIC;
ALTER TABLE smart_money_signals ALTER COLUMN price_usd TYPE NUMERIC;
ALTER TABLE smart_money_signals ALTER COLUMN price_sol TYPE NUMERIC;
