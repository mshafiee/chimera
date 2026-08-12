#!/bin/bash
# Profitability evidence loop — diagnose over-rejection + map the achievable frontier.
#
# Runs entirely against the operator's read-only shadow data:
#   A. rejection funnel (which gate rejects the most, and was it right?)
#   B. Pareto frontier  (what win rate / monthly return is actually achievable?)
#
# Usage (on the production server, or anywhere with DB access):
#   bash scripts/profitability_loop.sh
#
# Requires DATABASE_URL / CHIMERA_DB_URL to point at the operator Postgres, and
# the scout Python env (psycopg) on PATH for part B.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -z "${DATABASE_URL:-}${CHIMERA_DB_URL:-}" ]]; then
    # Default to the in-container production DB when run on the server.
    export DATABASE_URL="postgres://chimera:chimera@localhost:5432/chimera"
fi

echo "=== A. Rejection funnel (per-gate counterfactual PnL) ==="
if docker exec chimera-postgres pg_isready -U chimera -d chimera >/dev/null 2>&1; then
    docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/rejection_funnel.sql
else
    psql "$DATABASE_URL" -f scripts/rejection_funnel.sql
fi

echo
echo "=== B. Pareto frontier (achievable region) ==="
if command -v python3 >/dev/null 2>&1; then
    (cd scout && python3 -m analysis.cli frontier)
else
    echo "python3 not found — run 'python -m analysis.cli frontier' from the scout/ dir" >&2
fi
