#!/bin/bash
# Roster repair 2026-09-19: unfreeze admissions for the paper measurement lane.
#
# Why: BUY admission requires status == ACTIVE (selection.rs B2). After the Sep
# sweeps, the 3 remaining ACTIVE wallets have never traded and emit zero
# signals, while 100% of live flow (132Tkgf, 12PPVhB8, 2h7Ns9w2, HgPFzrUm)
# arrives from PROVING/CANDIDATE wallets and dies at WALLET_NOT_ACTIVE.
# Admissions are structurally zero until the roster matches reality.
#
# What: demote the 3 decorative ACTIVE wallets to CANDIDATE; promote 3
# measurement-lane wallets to ACTIVE (flow volume + best shadow win-rate,
# paper-only). 132Tkgf stays PROVING: highest volume but lottery-shaped
# (6.5% win, moonshot-carried t=2.03) — inconsistent with the robustness
# stance; its flow still feeds shadow measurement either way.
# Evidence: docs/superpowers/analysis/2026-09-19-realized-cohort-mining.md,
# interim peek 2026-09-19 (n=1971 corrected cohort).
#
# All changes go through the operator PUT /api/v1/wallets/:address path
# (audit-logged, webhook cleanup), never hand-SQL. Idempotent: wallets
# already at the target status are skipped.
#
# Usage: bash scripts/roster_repair_2026_09_20.sh
# Env:   CHIMERA_API_URL (default https://chimera-01.moez.tech)
#        CHIMERA_API_KEY (or sourced from /opt/chimera/.env)
set -euo pipefail

API_URL="${CHIMERA_API_URL:-https://chimera-01.moez.tech}"

if [ -z "${CHIMERA_API_KEY:-}" ] && [ -f /opt/chimera/.env ]; then
    set -a
    # shellcheck disable=SC1091
    source /opt/chimera/.env
    set +a
fi
[ -n "${CHIMERA_API_KEY:-}" ] || { echo "ERROR: CHIMERA_API_KEY not set" >&2; exit 1; }

DEMOTE_REASON="Roster hygiene 2026-09-19: ACTIVE but never traded, zero signal flow"
PROMOTE_REASON="Measurement lane 2026-09-19: live signal flow + best shadow win-rate; paper-only"

# prefix:target-status pairs
RULES=(
    "12kNFp:CANDIDATE"
    "129i4z:CANDIDATE"
    "Cr1n5Z:CANDIDATE"
    "12PPVh:ACTIVE"
    "HgPFzr:ACTIVE"
    "2h7Ns9:ACTIVE"
)

echo "Fetching roster from ${API_URL}..."
ROSTER_JSON="$(curl -fsS -H "Authorization: Bearer ${CHIMERA_API_KEY}" \
    "${API_URL}/api/v1/wallets?limit=20000")"
echo "$ROSTER_JSON" > /tmp/roster_repair_20260919.json

resolve() { # resolve <prefix> -> "address|status"
    python3 -c "
import json
prefix = '$1'
roster = json.load(open('/tmp/roster_repair_20260919.json'))
for w in roster.get('wallets', []):
    if w.get('address', '').startswith(prefix):
        print(w['address'] + '|' + w.get('status', '?'))
        break
"
}

set_status() { # set_status <address> <status> <reason>
    curl -fsS -X PUT \
        -H "Authorization: Bearer ${CHIMERA_API_KEY}" \
        -H "Content-Type: application/json" \
        -d "{\"status\": \"$2\", \"reason\": \"$3\"}" \
        "${API_URL}/api/v1/wallets/$1" > /dev/null
}

changed=0
skipped=0
for rule in "${RULES[@]}"; do
    prefix="${rule%%:*}"
    target="${rule##*:}"
    info="$(resolve "$prefix")"
    if [ -z "$info" ]; then
        echo "  ${prefix}: not found in roster — skipping"
        skipped=$((skipped + 1))
        continue
    fi
    address="${info%%|*}"
    current="${info##*|}"
    if [ "$current" = "$target" ]; then
        echo "  ${prefix}... already ${target} — skipping"
        skipped=$((skipped + 1))
        continue
    fi
    reason="$DEMOTE_REASON"
    [ "$target" = "ACTIVE" ] && reason="$PROMOTE_REASON"
    echo "  ${prefix}... ${current} -> ${target}"
    set_status "$address" "$target" "$reason"
    changed=$((changed + 1))
done

echo "Done: ${changed} changed, ${skipped} skipped."
echo "Verifying ACTIVE set:"
curl -fsS -H "Authorization: Bearer ${CHIMERA_API_KEY}" \
    "${API_URL}/api/v1/wallets?status=ACTIVE" \
    | python3 -c "
import json, sys
roster = json.load(sys.stdin)
for w in roster.get('wallets', []):
    print(' ', w.get('address', '')[:8], w.get('status'), 'wqs=', w.get('wqs_score'))
"
rm -f /tmp/roster_repair_20260919.json
