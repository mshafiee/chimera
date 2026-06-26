# Changelog

All notable changes to Project Chimera will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0] - 2026-06-26

### Changed
- Unified versioning: introduced single `VERSION` file as source of truth across operator, scout, and web components
- Standardized all component versions to `1.0.0` (previously: operator 7.1.0, web 1.0.0, scout 0.1.0)
- Web UI version display now reads dynamically from `package.json` via `web/src/lib/version.ts`

### Added
- `VERSION` file (root) as canonical version source
- `CHANGELOG.md` for release history tracking
- `docs/core/versioning.md` versioning policy and mechanism documentation
- `scripts/bump-version.sh` automated version bump and release tool
- `scripts/check-version-consistency.sh` version drift detection (CI-enforced)
- `.github/workflows/release.yml` automated release workflow on tag push
- `scout/_version.py` for programmatic version access in Python
- Version consistency check job in CI pipeline

### Security
- Any changes to circuit breaker, risk limits, executor, or token-safety paths are flagged with a `🛡️ safety:` changelog marker

[Unreleased]: https://github.com/mshafiee/chimera/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/mshafiee/chimera/releases/tag/v1.0.0
