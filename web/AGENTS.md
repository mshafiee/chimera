<!-- Managed by agent: keep sections and order; edit content, not structure. Last updated: 2026-08-19 -->
# web/ — Chimera Dashboard (TypeScript/React)

Scoped rules for `chimera-dashboard`. Cross-cutting conventions (git flow,
deployment, versioning) live in the root `../AGENTS.md`.

## Overview

Operator-facing control surface: React 18 SPA (Vite) that monitors and controls the
trading engine. Reads the backend through `src/api/*` clients; exposes dashboards for
monitoring, risk, signals, wallets, reconciliation, and configuration.

## Commands (from this dir)

| Command | Purpose |
|---------|---------|
| `npm run dev` | Dev server (`make dev-web`) |
| `npm run build` | Type-check + build (`tsc && vite build`) |
| `npm run type-check` | `tsc --noEmit` |
| `npm run lint` | ESLint (`--max-warnings 50`) |
| `npm run lint:fix` | ESLint auto-fix |
| `npm run format` / `format:check` | Prettier write / check |
| `npm run test:unit` | Unit tests (Vitest) |
| `npm test` | E2E tests (Playwright) |
| `npm run preview` | Preview production build |

## Project Structure

| Path | Purpose |
|------|---------|
| `src/main.tsx` / `src/App.tsx` | App bootstrap / root |
| `src/api/` | Typed backend clients (`client.ts`, `config.ts`, `signals.ts`, `wallets.ts`, `risk.ts`, `trades.ts`, `positions.ts`, `health.ts`, `webhooks.ts`, …) |
| `src/components/` | Feature components: `dashboard`, `risk`, `signals`, `operations`, `wallet`, `webhooks`, `reconciliation`, `performance`, `charts`, `config`, `consensus`, `market`, `auth`, `layout` |
| `src/components/ui/` | Primitives: `Button`, `Card`, `Table`, `Badge`, `Modal`, `MetricCard`, `Toast`, `TimeRangePicker`, … |
| `src/stores/` | Zustand global state |
| `src/hooks/` | Shared hooks |
| `src/lib/` | Utilities |
| `src/types/` | Shared TS types |
| `tests/e2e/` | Playwright specs (`login`, `dashboard`, `circuit-breaker`, `wallet-promote`, …) |
| `src/__tests__`, `src/**/__tests__` | Vitest unit suites |

## Code Style (extends root TypeScript rules)

- React 18 functional components + hooks; TypeScript strict mode.
- State: Zustand for global state, React hooks for local state.
- Styling: TailwindCSS; use `clsx` for conditional classes.
- Imports: named imports preferred.
- Keep `src/api/*` response types in sync with the backend (`../api`) contract.
- Avoid `any`; prefer explicit interfaces.

## Boundaries

**Always**
- Keep api clients aligned with backend responses; update both together.
- Run `npm run lint` and `npm run type-check` before finishing edits.
- Add/adjust Vitest unit + Playwright E2E coverage for changed screens.

**Ask first**
- Changing routing or global store shape (affects many components).
- Modifying a shared `src/components/ui/*` primitive (used across all screens).

**Never**
- Commit secrets or real API keys in the bundle or env files.
- Bypass the typed api client with ad-hoc `fetch`/`axios` of backend internals.

## Setup & environment
- Node `^20.19.0 || >=22.12.0` (npm). Install with `npm install`; Playwright needs `npx playwright install` for E2E.

## Security & safety
- Never log tokens/secrets in the client; keep wallet auth flows server-side.
- Bind the app to the operator backend via `src/api/client.ts` only — no raw credentials in bundles.

## Examples
> Prefer real code in this repo — `src/api/*` clients and `src/components/ui/*` primitives are the canonical patterns.

## When stuck
- Check root `../AGENTS.md` for cross-cutting conventions.
- `npm run type-check` + `npm run lint` surface most contract/style issues early.
