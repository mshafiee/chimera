# Dune Bootstrap Runbook (2026-09-07)

## Prereqs
- SSH: root@chimera-01.moez.tech
- Dune API key: already present at `/opt/chimera/.env` (`DUNE_API_KEY=...` — from the 2026-08-07 run)
- Dune config: already present at `/opt/chimera/config/config.yaml` §`dune` — `enabled: true`, `bootstrap_query_id: 8256459` (created 2026-08-07, still valid), `bootstrap_wallets_max: 60`, `bootstrap_roster_enabled: true`
- No env/config changes were needed — the 2026-09-06 audit showed the pipeline stopped by disuse, not config decay

## Running the bootstrap

```bash
cd /opt/chimera
# Binary lives at /app/bootstrap_dune (bare name is NOT on PATH):
docker compose --profile mainnet-prod run --rm operator /app/bootstrap_dune            # dry-run
docker compose --profile mainnet-prod run --rm operator /app/bootstrap_dune --apply --roster
```

## ⛔ Current blocker (2026-09-06 22:23 UTC+2)

Dry-run fails fail-closed with:

```
Dune execute returned HTTP 402 Payment Required: {"error":"This api request
would exceed your configured datapoint limit per billing cycle."}
```

**The Dune API key's datapoint allowance for its billing cycle is exhausted.**
No writes occurred (fail-closed; DB unchanged at 3,000 stale `dune_%` rows
from 2026-08-07).

Resolution options (operator decision):
1. Upgrade/raise the datapoint limit at dune.com → subscription settings, or
2. Wait for the billing-cycle reset and re-run the dry-run → `--apply --roster`

The existing query (`8256459`) remains usable; no `--create-query` needed.

## Verify after a successful --apply

```bash
docker exec chimera-postgres psql -U chimera -d chimera -Atc \
  "SELECT COUNT(*), MAX(exited_at)::date
   FROM shadow_exits e JOIN shadow_positions s USING(shadow_id)
   WHERE s.shadow_id LIKE 'dune_%';"
# Expect: n>0 and MAX(exited_at) within the last 2 days.
# Then run the baseline snapshot per docs/runbooks/2026-09-07-shadow-validation-protocol.md
```

## Verification log

| Date | Action | Result |
|---|---|---|
| 2026-09-06 | dry-run `/app/bootstrap_dune` | ⛔ HTTP 402 datapoint limit exhausted; fail-closed, no writes |
