#!/usr/bin/env bash
# Chimera Version Consistency Check
# Verifies VERSION file matches all synced component manifests.
#
# Usage: scripts/check-version-consistency.sh
# Exit code: 0 = consistent, 1 = drift detected
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VERSION_FILE="$ROOT_DIR/VERSION"

if [ ! -f "$VERSION_FILE" ]; then
  echo "ERROR: VERSION file not found at $VERSION_FILE"
  exit 1
fi

EXPECTED=$(cat "$VERSION_FILE" | tr -d '[:space:]')
if [ -z "$EXPECTED" ]; then
  echo "ERROR: VERSION file is empty"
  exit 1
fi

echo "Checking version consistency (expected: $EXPECTED)"
ERRORS=0

check_field() {
  local label="$1"
  local actual="$2"
  if [ "$actual" = "$EXPECTED" ]; then
    echo "  ✓ $label: $actual"
  else
    echo "  ✗ $label: expected $EXPECTED, got $actual"
    ERRORS=$((ERRORS + 1))
  fi
}

# Cargo.toml
if [ -f "$ROOT_DIR/operator/Cargo.toml" ]; then
  actual=$(grep '^version' "$ROOT_DIR/operator/Cargo.toml" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
  check_field "operator/Cargo.toml" "$actual"
else
  echo "  ⚠ operator/Cargo.toml not found"
  ERRORS=$((ERRORS + 1))
fi

# package.json
if [ -f "$ROOT_DIR/web/package.json" ]; then
  actual=$(grep '"version"' "$ROOT_DIR/web/package.json" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
  check_field "web/package.json" "$actual"
else
  echo "  ⚠ web/package.json not found"
  ERRORS=$((ERRORS + 1))
fi

# pyproject.toml
if [ -f "$ROOT_DIR/scout/pyproject.toml" ]; then
  actual=$(grep '^version' "$ROOT_DIR/scout/pyproject.toml" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
  check_field "scout/pyproject.toml" "$actual"
else
  echo "  ⚠ scout/pyproject.toml not found"
  ERRORS=$((ERRORS + 1))
fi

# scout/_version.py
if [ -f "$ROOT_DIR/scout/_version.py" ]; then
  actual=$(grep '__version__' "$ROOT_DIR/scout/_version.py" | head -1 | sed 's/.*"\([^"]*\)".*/\1/')
  check_field "scout/_version.py" "$actual"
else
  echo "  ⚠ scout/_version.py not found"
  ERRORS=$((ERRORS + 1))
fi

# config.yaml header comment
if [ -f "$ROOT_DIR/config/config.yaml" ]; then
  actual=$(grep '^# v' "$ROOT_DIR/config/config.yaml" | head -1 | sed 's/^# v//')
  check_field "config/config.yaml" "$actual"
else
  echo "  ⚠ config/config.yaml not found"
  ERRORS=$((ERRORS + 1))
fi

echo ""
if [ "$ERRORS" -eq 0 ]; then
  echo "✓ All versions consistent at $EXPECTED"
  exit 0
else
  echo "✗ Version drift detected ($ERRORS mismatches)"
  exit 1
fi
