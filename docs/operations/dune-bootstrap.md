# Dune Bootstrap — Reference Query & Runbook

One-shot bootstrap of the trading system with historical wallet PnL evidence
from Dune Analytics. Seeded rows use `exit_strategy='dune_wallet'`,
`shadow_id` prefix `dune_`, `exit_reason='dune_bootstrap'` and feed the
wallet t-stat / proven-wallet / smart-money-cluster gates via
`get_wallet_pnl_statistics` (which unions `mirror_main` + `dune_wallet`).
The token mirror gate reads ONLY `mirror_main` — bootstrap rows can never
count as mirror-gate evidence.

## 1. The query (created via API — no manual action)

The bootstrap query is created automatically by the binary:

```bash
# Creates the query privately via POST /api/v1/query, copying the trades
# table (dex_solana.trades) from dune.promote_query_id, then executes it
# and prints the dry-run report.
DUNE_API_KEY=... bootstrap_dune --create-query
```

The live query is **8256459** ("Chimera Bootstrap — Wallet Round-Trip PnL
(30d)"), pinned in `config/config.yaml` (`dune.bootstrap_query_id`).
Superseded private iterations 8256427 / 8256431 remain in the workspace
(v1 API has no delete); ignore them. Use `--from-query <id>` to copy the
table from a different reference query.

Reference SQL (what `--create-query` generates; adapt only if the workspace
dataset differs):

```sql
WITH buys AS (
    SELECT
        trader_id AS wallet_address,
        token_bought_mint_address AS token_address,
        block_time AS entry_ts,
        amount_usd AS buy_usd,
        ROW_NUMBER() OVER (
            PARTITION BY trader_id, token_bought_mint_address
            ORDER BY block_time
        ) AS rn
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '30' day
      AND token_sold_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT')
      AND trader_id IS NOT NULL
      AND token_bought_mint_address IS NOT NULL
      AND amount_usd > 50
),
sells AS (
    SELECT
        trader_id AS wallet_address,
        token_sold_mint_address AS token_address,
        block_time AS exit_ts,
        amount_usd AS sell_usd,
        ROW_NUMBER() OVER (
            PARTITION BY trader_id, token_sold_mint_address
            ORDER BY block_time
        ) AS rn
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '30' day
      AND token_bought_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT')
      AND trader_id IS NOT NULL
      AND token_sold_mint_address IS NOT NULL
      AND amount_usd > 50
),
round_trips AS (
    SELECT
        b.wallet_address, b.token_address,
        (s.sell_usd - b.buy_usd) / b.buy_usd * 100.0 AS pnl_pct,
        s.exit_ts,
        date_diff('second', b.entry_ts, s.exit_ts) AS hold_duration_secs,
        ROW_NUMBER() OVER (PARTITION BY b.wallet_address ORDER BY s.exit_ts DESC) AS rn
    FROM buys b
    JOIN sells s
      ON b.wallet_address = s.wallet_address
     AND b.token_address  = s.token_address
     AND b.rn             = s.rn
    WHERE s.exit_ts > b.entry_ts AND s.sell_usd > 0 AND b.buy_usd > 0
),
wallet_counts AS (
    SELECT wallet_address, COUNT(*) AS n FROM round_trips GROUP BY wallet_address
)
SELECT rt.wallet_address, rt.token_address, ROUND(rt.pnl_pct, 4) AS pnl_pct,
       rt.exit_ts, rt.hold_duration_secs
FROM round_trips rt
JOIN wallet_counts wc ON wc.wallet_address = rt.wallet_address
WHERE wc.n >= 10 AND rt.rn <= 50
ORDER BY rt.wallet_address, rt.exit_ts
LIMIT 20000
```

Notes:

- **Rows are bounded** (≥10 round trips per wallet filter + 50 per wallet +
  LIMIT 20000), so credit cost is bounded: one execution per run.
- `wallet_address` is the trader wallet (44-char base58); `pnl_pct` is the
  round-trip return in percent; `exit_ts` is a Dune timestamp
  (`YYYY-MM-DD HH:MM:SS.fff UTC` — the binary parses this and other common
  formats); `hold_duration_secs` may be NULL.
- Status on 2026-08-07: 649 wallets matched (≥10 round trips, 30d);
  dry-run processed the top 60 — 30 PROFITABLE (t > 1.645).

## 2. Run on the server

Config is mounted read-only at `/app/config`; the binary reads
`DUNE_API_KEY` + `DATABASE_URL` from the environment (both already present
for the operator service).

```bash
# Build + pull
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build bootstrap_dune

# 1. Dry-run (default; no writes) — review the per-wallet report
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml run --rm bootstrap_dune bootstrap_dune --dry-run

# 2. Real run — replaces dune_% rows per wallet, idempotent
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml run --rm bootstrap_dune bootstrap_dune --apply

# 3. Roster seed (CANDIDATE only; ACTIVE/REJECTED never touched)
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml run --rm bootstrap_dune bootstrap_dune --apply --roster
```

Flags: `--dry-run` (default), `--apply`, `--roster`, `--no-roster`,
`--wallet <addr>` (process only one wallet), `--create-query` (create the
query via the Dune API when `bootstrap_query_id` is 0), `--from-query <id>`
(reference query for the table name).

## 3. Validation

```sql
-- dune_wallet rows present
SELECT exit_strategy, COUNT(*) FROM shadow_exits GROUP BY 1;

-- a seeded wallet's union statistics (mirror_main + dune_wallet)
SELECT sp.wallet_address, COUNT(*), AVG(se.pnl_pct), STDDEV(se.pnl_pct)
FROM shadow_exits se
JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
WHERE sp.wallet_address = '<wallet>'
  AND se.exit_strategy IN ('mirror_main', 'dune_wallet')
  AND sp.opened_at > NOW() - INTERVAL '30 days'
GROUP BY sp.wallet_address;

-- roster: new CANDIDATE wallets, existing statuses unchanged
SELECT status, COUNT(*) FROM wallets GROUP BY 1;
```

Ad-hoc analysis SQL over shadow data MUST filter
`shadow_id NOT LIKE 'dune\_%'` (or `exit_strategy = 'mirror_main'`) so
bootstrap rows never count as mirror-gate or gate-performance evidence
(Phase 6 A/B hygiene).
