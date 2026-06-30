# Changelog

All notable changes to Project Chimera will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Jupiter API key: `jupiter.api_key` config (env `CHIMERA_JUPITER__API_KEY`), sent as
  `x-api-key` on every Jupiter request (quote/swap/price). Fail-fast in `Live` trade mode
  when absent. Implements the DEX plan P0-1.
- Unified slippage model (`engine::slippage`): expected impact (Jupiter `priceImpactPct` →
  liquidity sqrt-model → config tier) plus a `expected×2 + 30bps` buffer clamped to
  per-strategy bounds (Shield `[10,100]`, Spear `[30,300]`, Exit `[50,1500]` bps). The same
  estimate now drives both the on-chain `slippageBps` and cost bookkeeping (P1-4).
- Real multi-DEX routing: `DexComparator` now compares the aggregate Jupiter quote against
  `dexes=`-restricted per-DEX quotes and selects the highest `outAmount`; the winning quote is
  reused directly as the swap payload so `selected_dex` drives real routing (P0-2/P0-3).
- Swap API v2 self-sign path (`/swap/v2/build`), flag-gated via `jupiter.use_swap_v2`
  (default off; v1 retained until devnet-validated) (P1-8).
- Jito tip-account rotation: tip accounts fetched from Jito `getTipAccounts` and rotated
  round-robin (single verified account as default) (P2-14).
- Real per-route DEX fee (`routePlan[].swapInfo.feeAmount`) surfaced into cost tracking and
  the pre-execution cost gate, replacing the flat `dex_fee_rate` estimate (P2-17).

### Changed
- 🛡️ safety: V0 blockhash refresh is now a direct public-field swap + re-sign
  (`v0_reconstruction::refresh_v0_blockhash`), deleting the 280-line ALT-fetch/recompile path
  with its heuristic signer/writable derivation and per-ALT `getAccountData` RPCs (P1-7).
- 🛡️ safety: Bundle submission no longer treats the Jito/Helius `bundleId`/UUID as a
  transaction signature. Bundles are resolved to the real landed signature via
  `getBundleStatuses` before polling confirmation; unresolved bundles are marked unconfirmed
  for recovery (P1-9, F12).
- 🛡️ safety: Helius Sender path is legacy-only — V0 transactions short-circuit to TPU instead
  of silently failing in the bundle (P1-9, F11).
- 🛡️ safety: Legacy TPU fallback now polls confirmation instead of assuming `confirmed = true`
  (P1-10, F13).
- 🛡️ safety: One blockhash per swap — the builder's blockhash is threaded through build +
  submit, removing redundant `getLatestBlockhash`/`is_blockhash_valid` RPCs per live swap
  (P1-11, F14).
- SELL-to-zero guard: a scaled sell amount that rounds to 0 is now rejected with a clear
  validation error instead of submitting an empty sell (P1-12, F15).
- Unknown token decimals surface as an unknown fill price (sentinel) with a warning, instead of
  silently recording a `0` PnL/cost (P1-13, F16).
- `priceImpactPct` is parsed consistently as a percent string across builder and comparator
  (P1-6, F7). Duplicate Jupiter quote round-trips on the swap path eliminated (P1-5, F8).
- 🛡️ safety: Jito tip is now **inlined** into the swap transaction (decision D3).
  Legacy transactions get the tip appended as a System `transfer` instruction
  (`engine::tip_inlining`) and ship as a single-tx bundle — one signature,
  all-or-nothing at the transaction level, replacing the `[tip_tx, swap_tx]`
  two-tx bundle. The inline path decompiles the legacy message deterministically
  (static accounts only — no ALT resolution) and is round-trip unit-tested. V0
  transactions keep the separate-tip-bundle (inlining V0 requires ALT
  reconstruction; deferred per safety policy).
- 🛡️ safety: Jito tip accounts are hardcoded verified constants (the official 8
  from the Jito docs) and rotated round-robin, rather than blindly trusting a
  `getTipAccounts` RPC response (which could be diverted by a MITM'd/compromised
  endpoint). Both the direct-Jito and Helius paths rotate identically.
- 🛡️ safety: Helius bundle status now queries the Solana RPC host
  (`mainnet.helius-rpc.com`, JSON-RPC `sendBundle`/`getBundleStatuses`) per the
  official docs, fixing a resolver that previously posted to the wrong host and
  would have marked every landed Helius bundle `unconfirmed` (double-exec risk).
- Standardized the swap pipeline on the bincode 2.x `serde` API + `config::legacy()`; removed
  all `bincode1` call sites (identical wire format) (P2-15, F20).

### Notes
- 🛡️ safety items (V0 refresh, bundle-signature resolution, Helius legacy-only, blockhash
  threading, inline tip) reconstruct/re-sign the swap transaction. The pure-logic paths are now
  covered by automated unit tests (slippage model, V0 field-swap, inline-tip round-trip,
  bundle→swap-signature extraction, tip-account allowlist) and a real-data `#[ignore]`d harness
  (`safety_validation_tests`) that parses real Jupiter txs without funds. The full landing
  validation (BUY→SELL round-trip, bundle landing, cost gate) is documented in
  `docs/core/dex-safety-validation.md` and must be run on a funded small-balance wallet before any
  live trade.

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
