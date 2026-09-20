#!/bin/bash
# Hydra Phase D: deploy the dust-live lane + program feed (Hydra pivot).
#
# Runs ON THE PRODUCTION SERVER (root@chimera-01.moez.tech, /opt/chimera).
# Fails fast at each gate — do not skip ahead past a failure.
#
# Sequence:
#   0. Helius quota probe (monthly-cap aware: no retry loop, probe don't schedule)
#   1. git pull (Hydra commits through b88137a or later)
#   2. CHIMERA_TRADE_MODE=dust_live in server .env (compose reads it since b6bf6cb)
#   3. Rebuild + recreate operator (embeds 0026/0027 migrations at compile time)
#   4. Assert: DUST_LIVE banner + 0026/0027 applied + vault OK
#   5. Verdict check (informational for dust; still binding for full Live)
#   6. Program feed (alongside wallet webhooks — NO deletion yet)
#   7. Day-14 + parity instructions
#
# Usage: bash scripts/hydra_deploy.sh
# Env:   HELIUS_API_KEY (or sourced from /opt/chimera/.env)
set -euo pipefail

cd /opt/chimera
LOG_FILE="${CHIMERA_DEPLOY_LOG:-/opt/chimera/data/logs/hydra-deploy.log}"
mkdir -p "$(dirname "$LOG_FILE")"
log() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$LOG_FILE"; }
fail() { log "ERROR: $*"; exit 1; }

if [ -z "${HELIUS_API_KEY:-}" ] && [ -f /opt/chimera/.env ]; then
    set -a
    # shellcheck disable=SC1091
    source /opt/chimera/.env
    set +a
fi
[ -n "${HELIUS_API_KEY:-}" ] || fail "HELIUS_API_KEY not set and /opt/chimera/.env not readable"

# ── 0. Quota probe (fail fast — monthly cap does NOT clear at midnight) ──
log "Step 0: Helius quota probe"
code="$(curl -sS -o /dev/null -w '%{http_code}' "https://api.helius.xyz/v0/webhooks?api-key=${HELIUS_API_KEY}" || true)"
[ "$code" = "200" ] || fail "Helius probe HTTP $code — 'max usage reached' means monthly cap: billing-cycle reset or fresh key required. Nothing else will work; stopping."
log "Quota probe OK (HTTP 200)"

# ── 1. Pull ───────────────────────────────────────────────────────────────
log "Step 1: git pull"
git pull origin main || fail "git pull failed"
git log --oneline -3 | tee -a "$LOG_FILE"

# ── 2. Dust mode in server .env ───────────────────────────────────────────
log "Step 2: CHIMERA_TRADE_MODE=dust_live"
touch /opt/chimera/.env
if grep -q '^CHIMERA_TRADE_MODE=' /opt/chimera/.env 2>/dev/null; then
    sed -i.bak 's/^CHIMERA_TRADE_MODE=.*/CHIMERA_TRADE_MODE=dust_live/' /opt/chimera/.env
    log "Updated existing CHIMERA_TRADE_MODE line (backup .env.bak)"
else
    echo 'CHIMERA_TRADE_MODE=dust_live' >> /opt/chimera/.env
    log "Appended CHIMERA_TRADE_MODE=dust_live"
fi
grep '^CHIMERA_TRADE_MODE=' /opt/chimera/.env | tee -a "$LOG_FILE"

# ── 3. Rebuild + recreate operator ────────────────────────────────────────
log "Step 3: build + recreate operator (embeds 0026/0027)"
export COMPOSE_PROFILE=mainnet-prod
export HELIUS_API_KEY
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator \
    || fail "operator build failed"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator \
    || fail "operator recreate failed"

# ── 4. Assertions ─────────────────────────────────────────────────────────
log "Step 4: assertions (waiting up to 120s for boot)"
ok_banner=false
for _ in $(seq 1 24); do
    if docker logs chimera-operator 2>&1 | grep -q 'TRADE MODE: DUST_LIVE'; then
        ok_banner=true
        break
    fi
    if docker logs chimera-operator 2>&1 | grep -q 'TRADE MODE: PAPER'; then
        docker logs chimera-operator 2>&1 | grep 'TRADE MODE' | tee -a "$LOG_FILE"
        fail "Operator booted PAPER, not DUST_LIVE — CHIMERA_TRADE_MODE did not bind. Check .env + compose."
    fi
    sleep 5
done
$ok_banner || fail "No DUST_LIVE banner within 120s — inspect: docker logs chimera-operator"
log "Banner OK: TRADE MODE: DUST_LIVE"

docker logs chimera-operator 2>&1 | grep -i -e 'vault.*valid\|secrets.*valid' \
    | tail -2 | tee -a "$LOG_FILE" \
    || log "WARN: no vault-validation line found — verify keypair manually"

for ver in 26 27; do
    present="$(docker exec chimera-postgres psql -U chimera -d chimera -t -A \
        -c "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = $ver;" 2>/dev/null | tr -d '\r' || true)"
    [ "${present:-0}" = "1" ] || fail "Migration 00$ver not in _sqlx_migrations — boot-migrate did not apply it."
    log "Migration 00$ver applied"
done

# ── 5. Verdict (informational for dust; binding for Live) ─────────────────
log "Step 5: profitability verdict (informational — dust is carved out)"
curl -sS --max-time 15 'http://localhost:8080/api/v1/profitability/verdict' \
    | head -c 2000 | tee -a "$LOG_FILE" || log "WARN: verdict endpoint unreachable (auth?); check dashboard."
echo "" | tee -a "$LOG_FILE"

# ── 6. Program feed (alongside — never deletes wallet webhooks) ──────────
log "Step 6: Hydra program feed"
bash scripts/consolidate_program_webhooks.sh || fail "program feed setup failed"

# ── 7. Next steps ─────────────────────────────────────────────────────────
log "Done. Dust-live lane deployed."
log "RETAIN wallet webhooks until the program feed proves >=85% fill parity."
log "Day-14 decision: docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/dust_cohort_report.sql"
log "All five lines must read PROMOTE; any PARK line parks the strategy (no scale-up)."
log "Full-live sizing additionally requires the 8-gate GO (n>=60)."
