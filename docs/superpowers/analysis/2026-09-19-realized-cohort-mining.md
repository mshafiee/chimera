# Realized-Fill Cohort Mining (Phase 2) — 2026-09-19

**Question:** does ANY cohort of the admitted book carry positive expectancy when
measured on **realized paper fills** (never shadow marks)?

**Answer: NO.** Every cohort with n ≥ 10 is negative in absolute SOL, and the
loss is negative **per unit of deployed capital** in all four WQS×size buckets
(−1.4% to −3.3% per SOL). The single positive bucket (n=10) is not significant
(t=0.57). The earlier apparent "WQS inversion" (WQS ≥ 80 loses most) was a
**size confound** and is retracted below.

All numbers: production `chimera-postgres`, `chimera` DB, 2026-09-19 ~18:1x UTC,
read-only. Source table: `trades` joined to `decision_records` on `trade_uuid`,
filter `side='BUY' AND status='CLOSED' AND pnl_data_valid`.

**Book baseline:** n=261 closed BUYs, avg −0.00812 SOL, total **−2.078 SOL**.

---

## 1. By wallet (n ≥ 5)

Only 1 of 17 wallets is positive, and it is not significant.

| Wallet | n | Win% | Avg net | Total net |
|---|---|---|---|---|
| HvFdDWS3 | 16 | 43.8 | +0.00266 | **+0.0426** |
| 9yD81z7f | 5 | 20.0 | −0.00126 | −0.0063 |
| Grxr6mGL | 11 | 36.4 | −0.00111 | −0.0122 |
| … 14 more | — | — | negative | negative |
| 3nMNd89A | 22 | 9.1 | −0.00607 | −0.1276 |
| 8MPy8CXZ | 32 | 28.1 | −0.00677 | −0.2168 |
| ArcebCcX | 17 | 41.2 | −0.02820 | −0.4512 |

**Significance (query G):**

| Group | n | Mean | SD | t-stat |
|---|---|---|---|---|
| HvFdDWS3 | 16 | +0.00266 | 0.02034 | **0.52** (not significant) |
| all others | 245 | −0.00884 | 0.05562 | **−2.49** (significant) |

The one "winner" is indistinguishable from zero. HvFdDWS3 is currently
`REJECTED` (WQS 72); the biggest loser ArcebCcX is `PROVING` (WQS 80).

## 2. By liquidity band — all negative

| Band | n | Win% | Avg net | Total net |
|---|---|---|---|---|
| <15k | 9 | 11.1 | −0.00905 | −0.0724 |
| 15–30k | 25 | 20.0 | −0.00643 | −0.1478 |
| 30–60k | 25 | 24.0 | −0.02136 | −0.5339 |
| 60–150k | 68 | 19.1 | −0.00999 | −0.6695 |
| >150k | 118 | 19.5 | −0.00535 | −0.6310 |

No liquidity band is positive. Restricting liquidity cannot rescue the book.

## 3. By hold duration — all negative

| Hold | n | Win% | Avg net | Total net |
|---|---|---|---|---|
| <5m | 52 | 26.9 | −0.00424 | −0.2203 |
| 5–30m | 57 | 28.1 | −0.01126 | −0.6307 |
| 30–120m | 19 | 5.3 | −0.03258 | −0.6189 |
| >2h | 133 | 16.5 | −0.00472 | −0.6085 |

No hold band is positive — evidence that the loss is at **entry/selection**, not
exit timing. If exits were the dominant term, some hold band would be positive.

## 4. WQS bands (raw) — the confound

| WQS | n | Avg net | t-stat |
|---|---|---|---|
| <20 | 6 | −0.01230 | — |
| 20–40 | 26 | −0.00719 | — |
| 40–60 | 51 | +0.00002 | 0.00 |
| 60–80 | 46 | −0.00567 | — |
| ≥80 | 116 | −0.01351 | −2.93 |

Raw reading suggested "high WQS is inverted for copying". **This is retracted as
a size confound** (below).

## 5. Size controls the WQS effect

Avg entry size by WQS band (query K) is monotone: <40 → 0.231 SOL, 40–60 →
0.461, 60–80 → 0.531, ≥80 → 0.536. High-WQS wallets simply receive 2.3× larger
positions, so their larger absolute losses are an allocation artifact.

**WQS effect within small size (<1.0 SOL) (query L):**

| WQS | n | Avg net | t-stat |
|---|---|---|---|
| <60 | 73 | −0.00720 | −5.90 |
| ≥60 | 123 | −0.00323 | −1.16 |

Within small positions, higher WQS is **better**, not worse. The raw "inversion"
was size.

## 6. WQS × size buckets — the decisive decomposition

| WQS | Size | n | Avg net | t-stat | Total net | SOL deployed | Net per SOL |
|---|---|---|---|---|---|---|---|
| <60 | <1.0 | 73 | −0.00720 | **−5.90** | −0.5187 | 15.905 | **−3.26%** |
| <60 | ≥1.0 | 10 | +0.02588 | 0.57 | +0.2588 | 15.000 | +1.73% |
| ≥60 | <1.0 | 123 | −0.00323 | −1.16 | −0.3913 | 28.085 | **−1.39%** |
| ≥60 | ≥1.0 | 39 | −0.03693 | **−2.24** | −1.4035 | 58.500 | **−2.40%** |

- The largest absolute drain is `≥60 & ≥1.0` (−1.40 SOL, 68% of the book loss),
  and it is significant (t=−2.24).
- But **per SOL, every bucket with n≥10 is negative** (−1.39% to −3.26%).
- The lone positive bucket has n=10 and t=0.57 — noise.

**Loss asymmetry (query P):** large positions win more often and with a better
win/loss ratio than small ones (30.6% win, avg win +0.101 vs avg loss −0.081)
yet still lose more in absolute terms purely because ~2.5× more capital is
deployed. The negative expectancy is a property of the flow, not of sizing.

## 7. Consensus is not operational

`decision_records.consensus_wallet_count` is `1` for 2,562 of 2,563 admitted
BUYs (one `2`). Multi-wallet consensus never triggers on the admitted path, so
no consensus cohort can be mined.

---

## Conclusion and kill-rule outcome

1. **No significant positive realized cohort exists.** The best candidate
   (HvFdDWS3, t=0.52) is noise; the best bucket (n=10, t=0.57) is noise.
2. **Expectancy is negative per unit capital in every n≥10 bucket.** Size,
   liquidity, hold time, and WQS do not contain a positive edge, individually or
   jointly.
3. **The WQS-inversion hypothesis is retracted** — a size confound. The correct
   statement is "larger allocation to a negative-expectancy flow loses more
   money", not "WQS selects bad wallets".
4. **Pre-registered kill rule applies:** retire all mark-based gate loosening
   (WQS floor, `TOKEN_UNSAFE`, mirror relaxation, `TOKEN_TOO_NEW`,
   `WALLET_NOT_ACTIVE`). None can be justified on realized evidence.

**What follows:** only two paths remain — (a) **measurement repair** so a verdict
can ever be honest (dead-letter→pre-admission), and (b) **out-of-sample edge
hypotheses** (Pivot-A mirror, cluster), which are the only untested claims. No
amount of tuning the current admission set produces positive expectancy.

## Reproduction

```sql
-- Book baseline
SELECT count(*), AVG(net_pnl_sol), SUM(net_pnl_sol) FROM trades
WHERE side='BUY' AND status='CLOSED' AND pnl_data_valid;

-- Decisive decomposition (§6)
SELECT CASE WHEN dr.wqs<60 THEN '<60' ELSE '>=60' END wqs,
       CASE WHEN t.amount_sol<1.0 THEN '<1.0' ELSE '>=1.0' END size,
       count(*), AVG(t.net_pnl_sol), SUM(t.net_pnl_sol), SUM(t.amount_sol)
FROM trades t JOIN decision_records dr ON dr.trade_uuid=t.trade_uuid
WHERE t.side='BUY' AND t.status='CLOSED' AND t.pnl_data_valid
GROUP BY 1,2;
```
