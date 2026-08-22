#!/bin/bash
# Demote the 7 measured-NEGATIVE ACTIVE wallets (profitability fix Phase 1).
#
# Why: prod measurement 2026-08-22 found 7 ACTIVE wallets with negative
# copy-PnL still admitting signals (HvFdDW, 2qG8ih, 5PQRrd, C86cuE, BaoKkn,
# 9EVzTz, BZYoDs — prefixes; full addresses resolved from the live roster).
# They are demoted through the EXISTING operator path — the authenticated
# PUT /api/v1/wallets/:address endpoint used by the dashboard and the same
# status transition dune_monitor's negative-EV demotion performs — so the
# change is audit-logged, triggers webhook cleanup, and never touches SQL
# by hand. Idempotent: already-CANDIDATE wallets simply do not match.
#
# Usage: bash scripts/demote_negative_wallets.sh
# Env:   CHIMERA_API_URL   (default https://chimera-01.moez.tech)
#        CHIMERA_API_KEY   (operator/admin API key; or sourced from /opt/chimera/.env)
set -euo pipefail

API_URL="${CHIMERA_API_URL:-https://chimera-01.moez.tech}"
REASON="Demoted 2026-08-22: measured negative copy-PnL (profitability fix Phase 1)"
# 6-char address prefixes from the prod profitability measurement.
PREFIXES=("HvFdDW" "2qG8ih" "5PQRrd" "C86cuE" "BaoKkn" "9EVzTz" "BZYoDs")

if [ -z "${CHIMERA_API_KEY:-}" ] && [ -f /opt/chimera/.env ]; then
    set -a
    # shellcheck disable=SC1091
    source /opt/chimera/.env
    set +a
fi
[ -n "${CHIMERA_API_KEY:-}" ] || { echo "ERROR: CHIMERA_API_KEY not set" >&2; exit 1; }

echo "Fetching ACTIVE roster from ${API_URL}..."
ROSTER=$(curl -fsS -H "Authorization: Bearer ${CHIMERA_API_KEY}" \
    "${API_URL}/api/v1/wallets?status=ACTIVE")

demoted=0
for prefix in "${PREFIXES[@]}"; do
    address=$(echo "$ROSTER" | python3 -c "
import json, sys
prefix = '$prefix'
roster = json.load(sys.stdin)
matches = [w['address'] for w in roster.get('wallets', [])
           if w.get('address', '').startswith(prefix)]
print(matches[0] if matches else '')
")
    if [ -z "$address" ]; then
        echo "  ${prefix}: not ACTIVE (already demoted or absent) — skipping"
        continue
    fi
    echo "  Demoting ${address} (${prefix}...): PUT status=CANDIDATE"
    curl -fsS -X PUT \
        -H "Authorization: Bearer ${CHIMERA_API_KEY}" \
        -H "Content-Type: application/json" \
        -d "{\"status\": \"CANDIDATE\", \"reason\": \"${REASON}\"}" \
        "${API_URL}/api/v1/wallets/${address}" > /dev/null
    demoted=$((demoted + 1))
done

echo "Done: ${demoted}/${#PREFIXES[@]} wallets demoted to CANDIDATE."
