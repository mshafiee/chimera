# DEX remediation — 🛡️ safety validation runbook

This runbook validates the safety-critical paths introduced by the DEX
remediation (Jupiter API key, unified slippage, multi-DEX routing, V0 refresh,
bundle-signature resolution, inline Jito tips, Swap v2 flag-gating) **before any
live (mainnet) trade**.

Per the project safety policy, every change to signing / blockhash / V0 / tip
paths is flagged `🛡️ safety:` in the CHANGELOG and **must** pass the checks below
on a funded-but-small-balance wallet before mainnet.

---

## 1. Automated checks (run in CI — no credentials)

```bash
make lint-operator            # clippy (default features)
make test-operator            # cargo test
```

These cover the pure-logic safety paths with no network:

| 🛡️ Path | Validated by (unit test) |
|---|---|
| Unified slippage: `slippageBps = expected + buffer`, clamped to strategy bounds; thin vs deep pool | `engine::slippage::tests::{deep_pool_slippage_is_small_and_buffered, thin_pool_slippage_widens_within_ceiling, tolerance_tracks_expected_plus_buffer_within_bounds}` |
| V0 blockhash refresh is a pure field-swap (no ALT RPC) | `engine::v0_reconstruction::tests::*`, `tests/unit/v0_reconstruction_tests.rs` |
| Inline tip: decompile → append System transfer → recompile (round-trip exact) | `engine::tip_inlining::tests::*` |
| Bundle→signature returns the SWAP (last), reads `result.value`, not the tip | `engine::jito_searcher::tests::extract_swap_signature_*` |
| Official tip accounts parse, are unique, and rotation covers all 8 | `engine::jito_searcher::tests::{official_tip_accounts_are_valid_and_unique, tip_rotation_covers_all_official_accounts}` |

## 2. Real-data checks (run on demand — needs a Jupiter key, no funds)

These exercise the real Jupiter API and parse (never submit), so **no wallet or
funds** are required. Obtain a key from <https://developers.jup.ag/portal>.

```bash
CHIMERA_JUPITER__API_KEY=... \
  cargo test --test integration safety_validation -- --ignored --nocapture
```

| 🛡️ Path | Test | Asserts |
|---|---|---|
| V0 refresh on a **real** Jupiter V0 swap message | `safety_validation_tests::v0_refresh_preserves_real_jupiter_message` | Blockhash swapped; header / account_keys / instructions / ALTs byte-identical; no per-ALT RPC storm |
| Inline tip on a **real** Jupiter legacy swap tx | `safety_validation_tests::inline_tip_on_real_jupiter_legacy_tx` | Tip appended last; originals preserved verbatim; System transfer to an official tip account |

> If Jupiter returns V0 despite `asLegacyTransaction=true` (the lite-api ignores
> it), the legacy test skips gracefully — the inline-tip path is legacy-only; V0
> keeps the (atomic) separate-tip-bundle.

## 3. Landing checks (manual — needs a funded small-balance wallet)

These cannot be automated safely headless and require a real funded wallet on
**mainnet** (devnet cannot run Jupiter mainnet swap routes, and
`execute_devnet` intentionally submits a no-op self-transfer rather than a real
swap). Use the smallest viable size (e.g. 0.01 SOL).

Before each landing test, set the operator to `Paper` or use a throwaway wallet.

1. **BUY→SELL round-trip, decimals correctness**
   - BUY 0.01 SOL of a 9-decimal token, then a 6-decimal token.
   - SELL each back.
   - Assert the recorded `fill_price_sol_per_token` is **non-zero** and the
     amount is decimals-correct (unknown decimals surface as `None`, never `0`).
2. **Jito bundle landing + resolution**
   - Submit a small BUY via the direct Jito path.
   - Confirm `getBundleStatuses` resolves to the **swap** signature (block
     explorer opens the swap, not the tip) and `confirmed` flips true.
   - Repeat via the Helius fallback (only fires when direct Jito fails) to
     confirm the RPC-host resolver path.
3. **Cost gate**
   - On a thin pool, confirm a trade whose estimated total cost exceeds the
     Shield/Spear cap is **rejected** (and not recorded above the cap).

## 4. Feature-flag gating (Swap v2)

Swap v2 (`/swap/v2/build`) is **off by default** (`jupiter.use_swap_v2: false`).
Validate it in isolation before flipping:

1. `config.jupiter.use_swap_v2 = true`, `trade_mode = Paper`.
2. Confirm a quote+swap builds against `…/swap/v2/build` (watch logs for the v2
   URL, not the malformed `/swap/v1/v2/build`).
3. Run a landing round-trip (step 3.1) with the flag on.
4. Only then flip it on for live trading.

## Configuration required for landing tests

- `CHIMERA_JUPITER__API_KEY` (mandatory; Live mode hard-fails without it).
- A funded wallet in the vault (`secrets.wallet_private_key`).
- For the Helius path: `secrets.rpc_api_key` reused as the Helius key, and a
  Helius Solana RPC reachable at `mainnet.helius-rpc.com` (overridable via
  `HELIUS_RPC_BASE_URL`).

## Known limitations

- **Devnet cannot validate Jupiter swaps** — Jupiter builds mainnet routes.
  Devnet is only useful for the signing/V0/tip paths (covered headlessly in
  section 2 via parsing). The swap *landing* must be validated on mainnet with a
  small balance (section 3) or via `simulateTransaction` against a mainnet RPC
  with a funded fee payer.
- **`execute_devnet` submits a no-op** self-transfer; it does not exercise the
  swap path. (See `executor.rs:execute_devnet`.)
