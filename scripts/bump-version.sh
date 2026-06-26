#!/usr/bin/env bash
# Chimera Version Bump & Release Script
#
# Bumps VERSION, syncs all component manifests, updates doc headers,
# generates CHANGELOG section, commits, and tags.
#
# Usage:
#   scripts/bump-version.sh patch              # 1.0.0 → 1.0.1
#   scripts/bump-version.sh minor              # 1.0.0 → 1.1.0
#   scripts/bump-version.sh major              # 1.0.0 → 2.0.0
#   scripts/bump-version.sh minor --pre=beta  # 1.0.0 → 1.1.0-beta.1
#   scripts/bump-version.sh minor --final    # 1.1.0-beta.1 → 1.1.0
#
# Exit code: 0 = success, 1 = error
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VERSION_FILE="$ROOT_DIR/VERSION"
CHANGELOG_FILE="$ROOT_DIR/CHANGELOG.md"

PRE_RELEASE=""
FINAL=false
BUMP_TYPE="${1:?Usage: bump-version.sh <patch|minor|major> [--pre=alpha|beta|rc] [--final]}"

shift

for arg in "$@"; do
  case "$arg" in
    --pre=*) PRE_RELEASE="${arg#--pre=}" ;;
    --final)  FINAL=true ;;
    *)
      echo "ERROR: Unknown argument: $arg"
      echo "Usage: bump-version.sh <patch|minor|major> [--pre=alpha|beta|rc] [--final]"
      exit 1
      ;;
  esac
done

if [ "$FINAL" = true ] && [ -n "$PRE_RELEASE" ]; then
  echo "ERROR: --final and --pre are mutually exclusive"
  exit 1
fi

if ! echo "$BUMP_TYPE" | grep -qE '^(patch|minor|major)$'; then
  echo "ERROR: Bump type must be patch, minor, or major"
  exit 1
fi

if [ ! -f "$VERSION_FILE" ]; then
  echo "ERROR: VERSION file not found at $VERSION_FILE"
  exit 1
fi

CURRENT=$(cat "$VERSION_FILE" | tr -d '[:space:]')
if [ -z "$CURRENT" ]; then
  echo "ERROR: VERSION file is empty"
  exit 1
fi

bump_version() {
  local ver="$1"
  local type="$2"
  local pre="$3"
  local final="$4"

  local major minor patch pre_id pre_num

  if [[ "$ver" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)(-([a-z]+)\.([0-9]+))?$ ]]; then
    major="${BASH_REMATCH[1]}"
    minor="${BASH_REMATCH[2]}"
    patch="${BASH_REMATCH[3]}"
    pre_id="${BASH_REMATCH[5]:-}"
    pre_num="${BASH_REMATCH[6]:-}"
  else
    echo "ERROR: Cannot parse version: $ver"
    exit 1
  fi

  if [ "$final" = true ]; then
    echo "${major}.${minor}.${patch}"
    return
  fi

  case "$type" in
    major)
      major=$((major + 1)); minor=0; patch=0
      ;;
    minor)
      minor=$((minor + 1)); patch=0
      ;;
    patch)
      patch=$((patch + 1))
      ;;
  esac

  if [ -n "$pre" ]; then
    if [ "$type" = "patch" ] && [ "$pre_id" = "$pre" ]; then
      pre_num=$((pre_num + 1))
    else
      pre_num=1
    fi
    echo "${major}.${minor}.${patch}-${pre}.${pre_num}"
  else
    echo "${major}.${minor}.${patch}"
  fi
}

NEXT=$(bump_version "$CURRENT" "$BUMP_TYPE" "$PRE_RELEASE" "$FINAL")
echo "Current version: $CURRENT"
echo "Next version:    $NEXT"
echo ""

if [ -t 1 ]; then
  read -rp "Proceed with bump to $NEXT? [y/N] " confirm
  if [[ "$confirm" != [yY] ]]; then
    echo "Aborted"
    exit 0
  fi
fi

set_version() {
  local file="$1"
  local pattern="$2"
  local replacement="$3"
  sed -i.bak "s${pattern}${replacement}${pattern:0:1}" "$file" && rm -f "${file}.bak"
}

echo "Updating VERSION file..."
echo "$NEXT" > "$VERSION_FILE"

echo "Syncing operator/Cargo.toml..."
set_version "$ROOT_DIR/operator/Cargo.toml" \
  "|^version = \".*\"|version = \"${NEXT}\"|" \
  ""

echo "Syncing web/package.json..."
set_version "$ROOT_DIR/web/package.json" \
  '|  "version": ".*"|  "version": "'"$NEXT"'"|' \
  ""

echo "Syncing scout/pyproject.toml..."
set_version "$ROOT_DIR/scout/pyproject.toml" \
  "|^version = \".*\"|version = \"${NEXT}\"|" \
  ""

echo "Syncing scout/_version.py..."
set_version "$ROOT_DIR/scout/_version.py" \
  '|__version__ = ".*"|__version__ = "'"$NEXT"'"|' \
  ""

echo "Syncing config/config.yaml header..."
set_version "$ROOT_DIR/config/config.yaml" \
  "|^# v.*|# v${NEXT}|" \
  ""

echo "Updating doc headers..."
for doc_file in \
  "$ROOT_DIR/docs/core/pdd.md" \
  "$ROOT_DIR/docs/monitoring/gateway-implementation-summary.md" \
  "$ROOT_DIR/docs/monitoring/access-guide.md" \
  "$ROOT_DIR/docs/operations/security-audit.md" \
  "$ROOT_DIR/docs/guides/scout-user-guide.md" \
  "$ROOT_DIR/docs/guides/scout-deployment-guide.md"
do
  if [ -f "$doc_file" ]; then
    set_version "$doc_file" \
      '|**Version:** [0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*|**Version:** '"$NEXT"'|' \
      ""
    echo "  Updated $(basename "$doc_file")"
  else
    echo "  Skipped $(basename "$doc_file") (not found)"
  fi
done

echo "Updating README.md badge..."
set_version "$ROOT_DIR/README.md" \
  "|version-v.*-blue|version-v${NEXT}-blue|" \
  ""

echo ""
echo "Generating CHANGELOG entry..."

LAST_TAG=$(git -C "$ROOT_DIR" describe --tags --abbrev=0 2>/dev/null || echo "")
if [ -n "$LAST_TAG" ]; then
  LOG_RANGE="${LAST_TAG}..HEAD"
else
  LOG_RANGE="HEAD"
fi

CHANGES=$(git -C "$ROOT_DIR" log "$LOG_RANGE" --pretty=format:"%s" 2>/dev/null || true)

if [ -n "$CHANGES" ]; then
  FEATS=$(echo "$CHANGES" | grep -E '^feat' | sed 's/^/  - /' || true)
  FIXES=$(echo "$CHANGES" | grep -E '^fix' | sed 's/^/  - /' || true)
  CHANGED=$(echo "$CHANGES" | grep -E '^(perf|refactor|build|ci)' | sed 's/^/  - /' || true)

  ENTRY="## [${NEXT}] - $(date +%Y-%m-%d)

"
  if [ -n "$FEATS" ]; then
    ENTRY="${ENTRY}### Added
${FEATS}

"
  fi
  if [ -n "$FIXES" ]; then
    ENTRY="${ENTRY}### Fixed
${FIXES}

"
  fi
  if [ -n "$CHANGED" ]; then
    ENTRY="${ENTRY}### Changed
${CHANGED}

"
  fi

  if [ -n "$PRE_RELEASE" ] || [ "$FINAL" = true ]; then
    ENTRY="${ENTRY}_Pre-release ${NEXT}_
"
  fi

  if [ -f "$CHANGELOG_FILE" ]; then
    awk -v entry="$ENTRY" '
      {print}
      /^## \[Unreleased\]/ {print entry}
    ' "$CHANGELOG_FILE" > "$CHANGELOG_FILE.tmp" && mv "$CHANGELOG_FILE.tmp" "$CHANGELOG_FILE"
  else
    printf "# Changelog\n\n%s" "$ENTRY" > "$CHANGELOG_FILE"
  fi
  echo "CHANGELOG entry added for $NEXT"
else
  echo "No conventional commits found since last tag; skipping CHANGELOG generation"
fi

echo ""
echo "Committing and tagging..."
git -C "$ROOT_DIR" add -A
git -C "$ROOT_DIR" commit -m "chore(release): v${NEXT}"
git -C "$ROOT_DIR" tag -a "v${NEXT}" -m "Release v${NEXT}"

echo ""
echo "✓ Release v${NEXT} prepared successfully"
echo ""
echo "Next steps:"
echo "  git push --follow-tags"
