# Versioning Policy

Project Chimera uses **unified Semantic Versioning** (`MAJOR.MINOR.PATCH[-prerelease]`) across all components (operator, scout, web). There is one version for the entire platform.

## Source of Truth

The `VERSION` file at the repository root contains the canonical version string (e.g. `1.0.0`). All other version declarations are derived from this file:

| File | Synced by `bump-version.sh` |
|------|-----------------------------|
| `VERSION` | Primary (read) |
| `operator/Cargo.toml` (`package.version`) | Auto-synced |
| `web/package.json` (`version`) | Auto-synced |
| `scout/pyproject.toml` (`project.version`) | Auto-synced |
| `scout/_version.py` (`__version__`) | Auto-synced |
| `config/config.yaml` (header comment) | Auto-synced |
| `web/src/lib/version.ts` (via package.json) | Auto-synced |
| Doc headers (`**Version:** X.Y.Z`) | Auto-synced |

**Never edit these files individually.** Use `make release` to bump versions atomically.

## Semver Rules (Chimera-Specific)

| Segment | Bump when... |
|---------|-------------|
| **MAJOR** (`X.0.0`) | Breaking webhook payload schema, non-backward-compatible DB migration, config key removal/rename, deployment topology change, or any change requiring re-validation of the execution/risk path |
| **MINOR** (`0.X.0`) | Backward-compatible features: new endpoints, additive config options, new WQS/strategy features, additive DB migrations |
| **PATCH** (`0.0.X`) | Bug fixes, performance improvements, refactors, docs, non-breaking dependency bumps |
| **Pre-release** (`-alpha.N`, `-beta.N`, `-rc.N`) | `-alpha`: internal testing. `-beta`: paper-trade validation. `-rc`: preflight-passed, pre-production. **Never trade live on alpha/beta.** |

### Safety-Critical Marker

Any change to the following modules is flagged in the CHANGELOG with `🛡️ safety:` regardless of bump level:

- `operator/src/circuit_breaker.rs`
- Risk limit configuration (`config.yaml` risk sections)
- Execution engine (`operator/src/engine/`)
- Token safety parser (`operator/src/token/`)

This ensures safety-critical changes are never silently shipped.

## Release Workflow

### 1. Bump and Release

```bash
# On main branch, after merging all PRs for the release
git checkout main && git pull

# Patch release (bug fix)
make release TYPE=patch

# Minor release (new feature)
make release TYPE=minor

# Major release (breaking change)
make release TYPE=major

# Pre-release
make release TYPE=minor --pre=beta

# Promote pre-release to final
make release TYPE=minor --final
```

### 2. Push and CI Release

```bash
git push --follow-tags
```

The `release.yml` workflow triggers on the tag push, runs tests, builds artifacts, and publishes a GitHub Release.

### What `make release` Does

1. Reads `VERSION`, computes next version
2. Updates `VERSION` file
3. Syncs all component manifests (Cargo.toml, package.json, pyproject.toml, _version.py, config.yaml)
4. Updates standardized doc headers (`**Version:** X.Y.Z`)
5. Generates CHANGELOG section from conventional commits since last tag
6. Commits as `chore(release): vX.Y.Z`
7. Creates annotated git tag `vX.Y.Z`

## Version Consistency Check

```bash
# Run locally
make version-check

# Also runs automatically in CI (required gate)
```

This verifies `VERSION` matches all synced files. CI will fail if versions drift.

## Commit Message Convention

All commits must follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

feat(operator): add priority queue load shedding
fix(scout): correct WQS temporal penalty calculation
docs(guide): update deployment instructions
chore(release): v1.2.0
```

Types used by the bump script:

| Type | CHANGELOG Section | Bump |
|------|-------------------|------|
| `feat` | Added | MINOR |
| `fix` | Fixed | PATCH |
| `perf` | Changed | PATCH |
| `refactor` | Changed | PATCH |
| `docs` | Changed | — (no bump) |
| `chore` | — | — (no bump) |
| `ci` | — | — (no bump) |
| `test` | — | — (no bump) |
| `build` | — | — (no bump) |

### Force a Bump Level

To override conventional-commit-based bumping:

```bash
make release TYPE=major   # Always bumps major
make release TYPE=minor   # Always bumps minor
make release TYPE=patch   # Always bumps patch
```

## Pre-Release Strategy

```
1.0.0-alpha.1  → internal dev / testing
1.0.0-alpha.2  → fixes from alpha.1
1.0.0-beta.1   → paper-trading validation
1.0.0-beta.2   → fixes from beta.1
1.0.0-rc.1     → preflight passed, pre-production
1.0.0-rc.2     → fixes from rc.1
1.0.0          → production release
```

Rules:
- Pre-releases never go on `main` without a final tag following
- Pre-release builds are suffixed with the pre-release identifier in artifact names
- CHANGELOG entries are accumulated; only the final release shows the full list

## Branching

- `main` — release branch. Tags are cut only from `main`.
- `develop` — integration branch for ongoing work.
- Feature branches merge to `develop` via PR.
- `develop` merges to `main` for release.

## Historical References

Documentation archived in `docs/archive/` and dated changelog/runbook entries reference prior version numbers (e.g., `v7.1`, `v7.1.1`). These are historical records and are intentionally **not updated** by the bump script.

The unified version numbering was introduced at `v1.0.0`. Prior versions (`v7.1` and earlier) used independent per-component versioning and are preserved as-is in historical documentation.
