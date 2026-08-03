#!/bin/bash
# Reclaim Docker disk: free STALE build cache + dangling images.
#
# Build cache is the dominant disk consumer — every operator/scout rebuild
# layers gigabytes that BuildKit never expires on its own. Without this, disk
# climbs past the 80% warning threshold within days of active deploys.
#
# Safe: never removes running containers/images or volumes. Keeps build cache
# touched within KEEP_HOURS (fast incremental rebuilds) and frees the rest.
#
# Cron: daily (see ops/ cron setup / server crontab).
set -e
set -o pipefail

KEEP_HOURS="${DOCKER_PRUNE_KEEP_HOURS:-24}"

echo "=== Docker prune $(date -u +%FT%TZ) ==="
BEFORE=$(df -h / | awk 'NR==2 {print $5 " used"}')

# Free build-cache layers not used in the last KEEP_HOURS (the big consumer).
docker builder prune -af --filter "until=${KEEP_HOURS}h" 2>/dev/null | tail -1 || true

# Dangling (untagged) images left over from rebuilds. -f, no --all: never
# removes tagged images in use by running containers.
docker image prune -f 2>/dev/null | tail -1 || true

AFTER=$(df -h / | awk 'NR==2 {print $5 " used"}')
echo "disk: ${BEFORE} -> ${AFTER}"
