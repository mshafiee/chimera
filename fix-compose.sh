#!/bin/bash
# Fix docker-compose-haproxy.yml service references

set -euo pipefail

COMPOSE_FILE="docker-compose-haproxy.yml"

[ -f "$COMPOSE_FILE" ] || { echo "ERROR: $COMPOSE_FILE not found" >&2; exit 1; }

# Context-aware replacement: only match the service name when followed by
# end-of-line, ':' or whitespace, so prefixed names like chimera-redis-eval
# are not corrupted. Keep a backup of the original.
sed -i.bak -E \
    -e 's/chimera-redis([: ]|$)/redis\1/g' \
    -e 's/chimera-prometheus([: ]|$)/prometheus\1/g' \
    -e 's/chimera-grafana([: ]|$)/grafana\1/g' \
    -e 's/chimera-alertmanager([: ]|$)/alertmanager\1/g' \
    "$COMPOSE_FILE"
rm -f "$COMPOSE_FILE.bak"

# Validate the result: the compose file must still parse, and no chimera-
# prefixed service references may remain.
if command -v docker > /dev/null 2>&1; then
    docker compose -f "$COMPOSE_FILE" config -q || { echo "ERROR: $COMPOSE_FILE is no longer valid" >&2; exit 1; }
fi
if grep -n 'chimera-\(redis\|prometheus\|grafana\|alertmanager\)' "$COMPOSE_FILE"; then
    echo "ERROR: remaining chimera- references found" >&2
    exit 1
fi

echo "Fixed service references:"
echo "  chimera-redis -> redis"
echo "  chimera-prometheus -> prometheus"
echo "  chimera-grafana -> grafana"
echo "  chimera-alertmanager -> alertmanager"
