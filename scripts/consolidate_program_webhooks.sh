#!/bin/bash
# Hydra program-subscription feed: one Helius webhook watching the 5
# established program IDs (Raydium AMMv4/CPMM/CLMM + Meteora DLMM/DAMM).
#
# This runs ALONGSIDE the wallet webhooks (consolidate_webhooks.sh) during the
# transition. Wallet webhooks are deleted only after the program feed proves
# fill-rate parity. Idempotent: skips creation when a webhook already covers
# all 5 program IDs.
#
# Usage: bash scripts/consolidate_program_webhooks.sh
# Env:   HELIUS_API_KEY (or sourced from /opt/chimera/.env)
set -euo pipefail

BASE_URL="https://api.helius.xyz"
PROGRAMS_JSON='["675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8","CPMMoo8L3F4NbTegBCKVN6G57yCiCR9xtsRGPBXyzs9","CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK","LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo","EoCxW6Yoqw8ThpSV9fuib479C9R9SgfW1QKV7eYt"]'
WEBHOOK_URL="${CHIMERA_WEBHOOK_URL:-https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook}"

if [ -z "${HELIUS_API_KEY:-}" ] && [ -f /opt/chimera/.env ]; then
    set -a
    # shellcheck disable=SC1091
    source /opt/chimera/.env
    set +a
fi
[ -n "${HELIUS_API_KEY:-}" ] || { echo "ERROR: HELIUS_API_KEY not set"; exit 1; }

existing="$(curl -sS "${BASE_URL}/v0/webhooks?api-key=${HELIUS_API_KEY}" || true)"
echo "$existing" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if isinstance(d,list) else 1)' 2>/dev/null || { echo "ERROR: Helius list failed (quota?)"; exit 1; }

# Check each webhook for full 5-program coverage via single GETs.
covered_id=""
for wid in $(echo "$existing" | python3 -c 'import json,sys; [print(w.get("webhookID","")) for w in json.load(sys.stdin)]'); do
    [ -n "$wid" ] || continue
    if curl -sS "${BASE_URL}/v0/webhooks/${wid}?api-key=${HELIUS_API_KEY}" | python3 -c "
import json,sys
want=set(json.loads('''$PROGRAMS_JSON'''))
try:
    d=json.load(sys.stdin)
except Exception:
    sys.exit(1)
sys.exit(0 if want.issubset(set(d.get('accountAddresses',[]))) else 1)" 2>/dev/null; then
        covered_id="$wid"
        break
    fi
done

if [ -n "$covered_id" ]; then
    echo "Program feed already covered by webhook $covered_id — nothing to do"
    exit 0
fi

body="$(python3 -c "
import json
print(json.dumps({
  'webhookURL': '''$WEBHOOK_URL''',
  'transactionTypes': ['SWAP'],
  'accountAddresses': json.loads('''$PROGRAMS_JSON'''),
  'webhookType': 'enhanced',
}))")"
resp="$(curl -sS -X POST -H 'Content-Type: application/json' -d "$body" "${BASE_URL}/v0/webhooks?api-key=${HELIUS_API_KEY}")"
new_id="$(echo "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("webhookID",""))' 2>/dev/null || true)"
[ -n "$new_id" ] || { echo "ERROR: program webhook create failed: $resp"; exit 1; }
echo "Created Hydra program-feed webhook $new_id (5 programs). Wallet webhooks retained."
