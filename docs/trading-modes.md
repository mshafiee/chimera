# Trading Modes & Position Sizing Configuration

Chimera supports four operational modes. Each has different risk profiles,
sizing strategies, and safety guardrails.

---

## 1. Paper Trading (Default — Current Production)

**Purpose:** Strategy evaluation without real capital at risk.

**How it works:** Signals are processed, sized, and recorded as trades in the
database, but no on-chain transactions are submitted. PnL is simulated.

**Config:**
```yaml
environment:
  - CHIMERA_TRADE_MODE=paper
```

**Position sizing:** Production defaults (see below). Since no real SOL is at
risk, full-sized positions simulate real-world performance accurately.

**Default sizing values (from `config.rs`):**
| Parameter | Default | Description |
|-----------|---------|-------------|
| `base_size_sol` | 0.1 SOL | Base trade size before WQS/confidence multipliers |
| `min_size_sol` | 0.05 SOL | Floor — trades below this are rejected as dust |
| `max_size_sol` | 2.0 SOL | Global per-trade cap |
| `shield_max_size_sol` | 2.0 SOL | Per-trade cap for SHIELD strategy |
| `spear_max_size_sol` | 0.5 SOL | Per-trade cap for SPEAR strategy |
| `min_live_position_sol` | 0.05 SOL | Executor minimum for live trades |
| `total_capital_sol` | 10.0 SOL | Capital base for Kelly/heat calculations |
| `max_concurrent_positions` | 5 | Max simultaneous open positions |
| `friction_gating_enabled` | true | Reject trades where fees > expected profit |

**Sizing formula (non-Kelly, default):**
```
size = base_size_sol × (wqs / 100) × confidence
```
Where:
- `wqs / 100` — WQS factor (WQS-100 wallet = 1.0×, WQS-50 = 0.5×)
- `confidence` — 0.5 default for new wallets, 1.0 for wallets with 15+ closed trades

Then multiplied by boost/penalty factors (consensus, performance, token age,
slippage, volatility, regime), then clamped to `[min_size_sol, strategy_max]`.

---

## 2. Dust-Live (Hydra — isolated micro-size live lane)

**Purpose:** End-to-end live evaluation with minimal capital at risk.
Verifies the full pipeline: signal → sizing → on-chain execution →
position tracking → exit → PnL — all with real transactions but tiny sizes.

**Risk:** 0.01–0.10 SOL per trade, fixed 0.05 SOL per admitted cluster
signal (clamped in code — `operator/src/engine/dust_runner.rs`, not config,
so a misconfigured sizing file cannot escalate to full-live risk).

**Config:**
```yaml
environment:
  - CHIMERA_TRADE_MODE=dust_live
```

DustLive shares the Live executor (Jito atomic bundles, Hydra 1.5%
slippage assert) and requires cluster quorum (≥3 wallets) like all Hydra
buys. It is verdict-gated exactly like Live (see
`docs/profitability-gates.md` Hydra appendix) unless a dust carve-out is
configured. Size clamps live in code (`DUST_SIZE_SOL=0.05`,
band `[0.01, 0.10]`); do NOT layer the legacy sizing overrides below —
they predate the `DustLive` enum and fight the in-code clamp.
Lane metrics: `DustLaneMetrics` (fills/signals, slippage-vs-quote).

> Legacy recipe (pre-enum, `CHIMERA_TRADE_MODE=live` + dust sizing
> overrides) — retained for reference only:
> `BASE 0.05 / MIN 0.01 / MIN_LIVE 0.01 / MAX 0.1 / SHIELD 0.1 / SPEAR 0.05`
> + `FRICTION=false`. Prefer `dust_live`.
>
> | Wallet WQS | Legacy recipe size |
> |------------|--------------------|
> | WQS-25 | 0.01 SOL (floored) |
> | WQS-50 | 0.02 SOL |
> | WQS-100 | 0.05 SOL |
> | Max (any) | 0.10 SOL |

**Checklist before enabling:**
- [ ] Vault keypair configured and funded with SOL
- [ ] `CHIMERA_TRADE_MODE=dust_live` set (canonical; `dustlive` also accepted)
- [ ] Startup banner shows `TRADE MODE: DUST_LIVE` (not PAPER — garbage values fall back silently)
- [ ] RPC endpoint is mainnet (not devnet)
- [ ] Helius webhooks registered (program feed: `scripts/consolidate_program_webhooks.sh`)
- [ ] Jupiter API key valid for swap routing
- [ ] Jito tip account configured (if using Jito)

---

## 3. Full Live Trading

**Purpose:** Production copy-trading with real capital.

**Risk:** Full position sizes (0.05–2.0 SOL per trade). Up to 10 SOL deployed.

**Config:**
```yaml
environment:
  - CHIMERA_TRADE_MODE=live
  # Production sizing (defaults — no overrides needed)
  # All sizing params use config.rs defaults
```

**Or custom production sizing:**
```yaml
environment:
  - CHIMERA_TRADE_MODE=live
  - CHIMERA_POSITION_SIZING__BASE_SIZE_SOL=0.2
  - CHIMERA_POSITION_SIZING__MAX_SIZE_SOL=1.0
  - CHIMERA_POSITION_SIZING__SHIELD_MAX_SIZE_SOL=1.0
  - CHIMERA_POSITION_SIZING__SPEAR_MAX_SIZE_SOL=0.5
  - CHIMERA_POSITION_SIZING__USE_KELLY_SIZING=true
  - CHIMERA_POSITION_SIZING__KELLY_FRACTION=0.25
```

**With Kelly criterion enabled:**
- Requires 15+ closed trades per wallet
- Dynamically adjusts size based on historical win rate and payoff ratio
- `kelly_fraction=0.25` means 25% of full Kelly (conservative)
- Hard-capped at 50% of capital per trade

---

## Switching Modes

To switch modes, update `CHIMERA_TRADE_MODE` in `docker-compose.yml`:

```bash
# On production server
cd /opt/chimera
# Edit docker-compose.yml: change CHIMERA_TRADE_MODE
COMPOSE_PROFILE=mainnet-prod docker compose --profile mainnet-prod -f docker-compose.yml \
  -f docker-compose-haproxy.yml up -d --force-recreate operator
COMPOSE_PROFILE=mainnet-prod docker compose --profile mainnet-prod -f docker-compose.yml -f docker-compose-haproxy.yml restart haproxy
```

**Safety:** The operator logs `NO REAL TRANSACTIONS WILL BE SUBMITTED` on
startup in paper mode. In live mode, it logs the wallet address and trade mode.
