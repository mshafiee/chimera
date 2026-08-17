#!/bin/bash
# Consolidate Helius webhook coverage for all ACTIVE wallets.
#
# Why: only 15/49 ACTIVE wallets had webhooks; the 34 uncovered wallets
# (including the four proven copy-targets) could never deliver signals, and
# the polling fallback for them burned the daily Helius quota by ~07:00 UTC.
# The health task's per-wallet registration was quota-blocked and its 441
# failed calls / 2 days never produced more webhooks.
#
# This script deterministically creates webhook(s) covering every ACTIVE
# wallet, writes the webhook_id into wallet_monitoring, and only then deletes
# stale duplicate webhooks. Idempotent: re-running resumes from detected
# state. Intended to run right after the midnight-UTC quota reset.
#
# Usage: bash scripts/consolidate_webhooks.sh
# Env:   HELIUS_API_KEY (or sourced from /opt/chimera/.env)
# Logs:  stdout + /opt/chimera/data/logs/consolidation.log
set -euo pipefail

LOG_FILE="${CHIMERA_CONSOLIDATION_LOG:-/opt/chimera/data/logs/consolidation.log}"
BASE_URL="https://api.helius.xyz"
BATCH_SIZE=10          # matches MAX_WALLETS_PER_WEBHOOK in webhook_lifecycle.rs
QUOTA_RETRIES=12       # 12 x 600s = 2h retry window after midnight reset
QUOTA_SLEEP=600
mkdir -p "$(dirname "$LOG_FILE")"

log() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" | tee -a "$LOG_FILE"; }
fail() { log "ERROR: $*"; exit 1; }

# ── 0. Load the Helius API key ────────────────────────────────────────────────
if [ -z "${HELIUS_API_KEY:-}" ] && [ -f /opt/chimera/.env ]; then
    set -a
    # shellcheck disable=SC1091
    source /opt/chimera/.env
    set +a
fi
[ -n "${HELIUS_API_KEY:-}" ] || fail "HELIUS_API_KEY not set and /opt/chimera/.env not readable"

psql_q() { # psql_q <sql> — one -tA row per line
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "$1" 2>/dev/null
}

# ── 1. Read ACTIVE wallets + webhook URL from the DB ─────────────────────────
log "Reading ACTIVE wallets and webhook URL from DB"
readarray -t WALLETS < <(psql_q "SELECT address FROM wallets WHERE status='ACTIVE' ORDER BY address;")
[ "${#WALLETS[@]}" -gt 0 ] || fail "No ACTIVE wallets found"
WEBHOOK_URL="$(psql_q "SELECT config_value FROM webhook_configuration WHERE config_key='current_webhook_url' LIMIT 1;" | tr -d '\r')"
if [ -z "$WEBHOOK_URL" ]; then
    WEBHOOK_URL="https://chimera-01.moez.tech/api/v1/monitoring/helius-webhook"
    log "webhook URL not in DB — using default $WEBHOOK_URL"
fi
log "Found ${#WALLETS[@]} ACTIVE wallets; webhook URL: $WEBHOOK_URL"

# ── 2. List existing webhooks ─────────────────────────────────────────────────
api() { # api <method> <path> [json-body]
    local method="$1" path="$2" body="${3:-}"
    if [ -n "$body" ]; then
        curl -sS -X "$method" -H 'Content-Type: application/json' \
            -d "$body" "${BASE_URL}${path}?api-key=${HELIUS_API_KEY}"
    else
        curl -sS -X "$method" "${BASE_URL}${path}?api-key=${HELIUS_API_KEY}"
    fi
}

list_webhooks() {
    api GET /v0/webhooks
}

# ── 3. Quota gate: probe until the reset lands (up to 2h) ────────────────────
probe_quota() {
    local resp status
    resp="$(list_webhooks || true)"
    status="$(curl -sS -o /dev/null -w '%{http_code}' "${BASE_URL}/v0/webhooks?api-key=${HELIUS_API_KEY}" || true)"
    [ "$status" = "200" ]
}

tries=0
while ! probe_quota; do
    tries=$((tries + 1))
    log "Quota probe failed (HTTP != 200) — attempt $tries/$QUOTA_RETRIES, sleeping ${QUOTA_SLEEP}s"
    [ "$tries" -ge "$QUOTA_RETRIES" ] && fail "Helius quota never recovered within retry window"
    sleep "$QUOTA_SLEEP"
done
log "Quota probe OK — Helius API available"

# ── 4. Early exit if coverage already complete (idempotent re-run) ───────────
# The LIST endpoint omits accountAddresses, so coverage is determined from
# single GETs of the webhooks referenced by ACTIVE rows in the DB.
COVERED=()
resp="$(list_webhooks || true)"
if [ -n "$resp" ] && echo "$resp" | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
    sys.exit(0 if isinstance(d,list) else 1)
except Exception:
    sys.exit(1)' 2>/dev/null; then
    while IFS= read -r wid; do
        [ -n "$wid" ] || continue
        COVERED+=("$wid")
    done < <(echo "$resp" | python3 -c '
import json,sys
try:
    whs=json.load(sys.stdin)
    for w in whs:
        print(w.get("webhookID",""))
except Exception:
    pass
')
fi
log "Existing webhooks on Helius: ${COVERED[*]:-none}"

already_covered() {
    local union_file
    union_file="$(mktemp)"
    while IFS= read -r wid; do
        [ -n "$wid" ] || continue
        api GET "/v0/webhooks/$wid" 2>/dev/null | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin)
    for a in d.get("accountAddresses",[]):
        print(a)
except Exception:
    pass' >> "$union_file" || true
    done < <(psql_q "SELECT DISTINCT helius_webhook_id FROM wallet_monitoring wm JOIN wallets w ON w.address=wm.wallet_address WHERE w.status='ACTIVE' AND wm.helius_webhook_id IS NOT NULL AND wm.helius_webhook_id != '';" | tr -d '\r')
    local missing=0
    for w in "${WALLETS[@]}"; do
        grep -qxF "$w" "$union_file" || missing=$((missing + 1))
    done
    rm -f "$union_file"
    [ "$missing" -eq 0 ]
}
if already_covered; then
    log "All ${#WALLETS[@]} ACTIVE wallets already covered — nothing to do"
    exit 0
fi

# ── 5. Create webhook(s) covering all ACTIVE wallets ─────────────────────────
create_webhook() { # create_webhook <comma-joined-addresses-json-array>
    local addresses="$1"
    local body
    body="$(python3 -c "
import json,sys
print(json.dumps({
  'webhookURL': '''$WEBHOOK_URL''',
  'transactionTypes': ['SWAP'],
  'accountAddresses': json.loads('''$addresses'''),
  'webhookType': 'enhanced',
}))")"
    api POST /v0/webhooks "$body"
}

CREATED_IDS=()
ALL_JSON="$(python3 -c "
import json
wallets = '''$(printf '%s\n' "${WALLETS[@]}")'''.strip().split('\n')
print(json.dumps(wallets))")"

log "Attempting one webhook covering all ${#WALLETS[@]} wallets"
resp="$(create_webhook "$ALL_JSON")"
if echo "$resp" | python3 -c 'import json,sys
try:
    json.load(sys.stdin); sys.exit(0)
except Exception:
    sys.exit(1)'; then
    NEW_ID="$(echo "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("webhookID",""))')"
    [ -n "$NEW_ID" ] || fail "Webhook create returned no webhookID: $resp"
    CREATED_IDS+=("$NEW_ID")
    log "Created consolidated webhook $NEW_ID with all ${#WALLETS[@]} wallets"
else
    log "Full-size create failed ($resp) — chunking into batches of $BATCH_SIZE"
    idx=0
    while [ "$idx" -lt "${#WALLETS[@]}" ]; do
        chunk=("${WALLETS[@]:idx:BATCH_SIZE}")
        idx=$((idx + BATCH_SIZE))
        chunk_json="$(python3 -c "
import json
wallets = '''$(printf '%s\n' "${chunk[@]}")'''.strip().split('\n')
print(json.dumps(wallets))")"
        resp="$(create_webhook "$chunk_json")"
        NEW_ID="$(echo "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("webhookID",""))' 2>/dev/null || true)"
        [ -n "$NEW_ID" ] || fail "Chunk create failed: $resp"
        CREATED_IDS+=("$NEW_ID")
        log "Created chunk webhook $NEW_ID for ${#chunk[@]} wallets"
        sleep 2
    done
fi

# ── 6. Write webhook_id into wallet_monitoring ───────────────────────────────
log "Writing webhook IDs into wallet_monitoring"
# Fetch each created webhook and upsert every address it covers (the created
# set is authoritative; wallets added by the health task in between are left
# to their own rows).
for wid in "${CREATED_IDS[@]}"; do
    wdata="$(api GET "/v0/webhooks/$wid" || true)"
    echo "$wdata" | python3 -c "
import json,sys,subprocess
try:
    w=json.load(sys.stdin)
except Exception:
    sys.exit(0)
wid='''$wid'''
for addr in w.get('accountAddresses',[]):
    subprocess.run([
        'docker','exec','chimera-postgres','psql','-U','chimera','-d','chimera','-t','-A','-c',
        f\"INSERT INTO wallet_monitoring (wallet_address, helius_webhook_id, monitoring_enabled, webhook_status) VALUES ('{addr}', '{wid}', true, 'registered') ON CONFLICT (wallet_address) DO UPDATE SET helius_webhook_id = EXCLUDED.helius_webhook_id, webhook_status = 'registered', last_registration_error = NULL, updated_at = NOW();\"
    ], check=False)
    print(f'upsert {addr[:8]}... -> {wid[:8]}')
" 2>/dev/null || true
done
log "wallet_monitoring upserts complete"

# ── 7. Verify coverage before any deletion ───────────────────────────────────
# NOTE: the LIST endpoint does not include accountAddresses — coverage must be
# verified via single GETs of the created webhook(s).
log "Verifying coverage"
COVERED_COUNT=0
for w in "${WALLETS[@]}"; do
    found=false
    for wid in "${CREATED_IDS[@]}"; do
        if api GET "/v0/webhooks/$wid" | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    sys.exit(1)
w='''$w'''
sys.exit(0 if w in d.get('accountAddresses',[]) else 1)" 2>/dev/null; then
            found=true
            break
        fi
    done
    $found && COVERED_COUNT=$((COVERED_COUNT + 1))
done
log "Coverage: $COVERED_COUNT / ${#WALLETS[@]} ACTIVE wallets"
[ "$COVERED_COUNT" -eq "${#WALLETS[@]}" ] || fail "Coverage incomplete — aborting before cleanup"

# ── 8. Delete superseded webhooks ────────────────────────────────────────────
# After the upserts every ACTIVE row points at a created webhook; any other
# webhook on the account is either empty or superseded. Keep anything still
# referenced by an ACTIVE wallet's DB row; delete the rest (clearing refs
# first so nothing dangles).
log "Deleting superseded webhooks"
for wid in "${COVERED[@]}"; do
    [ -n "$wid" ] || continue
    skip=false
    for cid in "${CREATED_IDS[@]}"; do [ "$wid" = "$cid" ] && skip=true; done
    $skip && continue
    if refcount="$(psql_q "SELECT count(*) FROM wallet_monitoring wm JOIN wallets w ON w.address=wm.wallet_address WHERE w.status='ACTIVE' AND wm.helius_webhook_id='$wid';" | tr -d '\r')"; then
        [ "${refcount:-0}" -gt 0 ] && { log "Keeping $wid (referenced by ${refcount} ACTIVE wallets)"; continue; }
    fi
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -c \
        "UPDATE wallet_monitoring SET helius_webhook_id = NULL WHERE helius_webhook_id = '$wid';" >/dev/null 2>&1 || true
    code="$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE "${BASE_URL}/v0/webhooks/${wid}?api-key=${HELIUS_API_KEY}" || true)"
    log "DELETE webhook $wid -> HTTP $code"
done

log "Done. Coverage $COVERED_COUNT/${#WALLETS[@]} ACTIVE wallets. Created: ${CREATED_IDS[*]:-none}"
