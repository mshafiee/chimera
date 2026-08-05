# Fix Rollout Baseline — 2026-08-05T20:36:11Z

48h measurement window starts. Compare these metrics at 2026-08-07T20:36Z.

| Metric | Baseline |
|--------|----------|
| ACTIVE wallets | 31 (was 25 at session start) |
| Scored candidates | 83 (scout recovering; was 85 stale) |
| Admitted BUY decisions / 6h | 12 (5 at quality >= 0.50) |
| Closed trades / 6h | 3 |
| PnL / 6h | -0.0872 SOL |
| Jito tips / 6h | 0.0056 SOL (cap deployed) |
| Shadow exits / 6h | 6,719 |

## All fixes deployed this session (chronological)
1. ad80732 — retry tip escalation capped at tip_ceiling_sol
2. e4a80d5 — price impact 5→2%, cost caps 5/8→2.5/3%, liquidity $5/10K→$20/30K, WQS 30→45, positions 20→12, max age 14→7d
3. b6f841b — signal quality 0.30→0.40, token age gate re-enabled 0.5h
4. 316d466 — parser: Helius events.swap primary path (98% signal loss fixed)
5. ff5e77d — on-chain verify + promote active CANDIDATE wallets (2 promoted)
6. e5ceece — scout: timeout hung discovery scans, deep scan 720→168h
7. 2e25a43 — Dune 24h-active trader query (8235367), prioritized over 7d
8. (earlier session) 954c3af, 7ea8299, 85bbed2, f90a067, 9cf8b5f, e6666dd

## Verification checks passed
- Parser: parsed swaps 5/hr → 44/hr post-fix
- Candidate promotion: 10 assessed, 2 promoted (3nMNd89AxwHU, 2snHHreXbpJ7)
- Dune 24h query: 128 wallets returned, 4 promoted through on-chain gate
- Scout: deep scan 168h completed 124.6s (was hung forever); WQS analysis running
- Gates: rejections are legitimate (toxic 25, liquidity 4, pump.fun 4, unsafe 2, honeypot 3, shadow blacklist 2); no over-restriction
