#!/bin/bash
# Chimera Rollback Script — RETIRED (2026-07)
#
# This script targeted the SQLite/systemd production stack: it restored
# data/chimera.db from a file backup and restarted a systemd service. That
# stack no longer exists. Production is Docker Compose + PostgreSQL
# (see AGENTS.md "Deployment Workflow"), where rollback is:
#
#   git revert <commit> && git push origin main
#   cd /opt/chimera && git pull origin main
#   COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml \
#       -f docker-compose-haproxy.yml build operator
#   COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml \
#       -f docker-compose-haproxy.yml up -d --force-recreate operator
#
# A Compose-native rollback script (with PostgreSQL PITR / volume snapshots)
# is tracked as part of the separate deploy-safety plan. Do not run this
# script — it would attempt to restore a database file that no longer exists
# and restart a service that is not managed by systemd.

echo "ERROR: ops/rollback.sh is RETIRED. It targets the decommissioned SQLite/systemd stack." >&2
echo "Use the git-revert + Compose rebuild rollback described in AGENTS.md." >&2
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    exit 1
else
    return 1
fi
