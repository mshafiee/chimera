# Leaked Credentials — Rotation Runbook (2026-08-02 scan)

> **Status: OPERATIONAL GATE — must be completed before merge/deploy.**
> The code-side scrubbing is done (see "Code remediation" below). Rotating the
> keys at the providers and recording evidence here is the remaining step. No
> code change can close this — a leaked credential in git history stays
> compromised until it is invalidated at the issuer.

## What happened

The 2026-08-02 full-repo scan (`docs/reviews/full-repo-scan-2026-08-02.md`)
found live production credentials committed in plaintext inside the
`prod-*.sh` / `*production*.sh` deploy scripts:

| Credential                  | Used for                                  |
|-----------------------------|------------------------------------------|
| Helius RPC API key          | Solana RPC / enhanced websocket access   |
| `CHIMERA_SECURITY__WEBHOOK_SECRET` | HMAC auth for inbound Helius webhooks |
| Jupiter API key             | Jupiter swap/price API access            |

Commit `3ff2a7e` deleted those scripts and scrubbed the literals from the
working tree, but **the old values are still reachable in git history**. Anyone
with read access to the repository must be treated as having had access to
these credentials until each one is rotated.

## Code remediation (DONE)

Verified against the working tree:

- The four leaking scripts (`prod-deploy.sh`, `deploy-production.sh`,
  `production-deploy.sh`, `production-deploy-simple.sh`) are deleted.
- Surviving deploy scripts (`final-production-deploy.sh`,
  `minimal-production-deploy.sh`) generate per-deploy credentials via
  `openssl rand` and write env files into a `mktemp` directory.
- `docker-compose.yml` requires `${JUPITER_API_KEY:?...}` with no leaked
  fallback.
- A repo-wide scan for `sk_…`, `helius…=`, `WEBHOOK_SECRET=`, and long hex
  literals in `*.sh`/`*.env`/`*.yml`/`*.toml`/`*.json` returns **zero**
  hardcoded credentials (only `${VAR:?…}` / `${VAR:-$(openssl rand …)}`
  patterns remain).

## Rotation checklist (TODO before merge)

Complete each, then fill in the evidence table at the bottom.

### 1. Helius RPC API key
1. Dashboard: <https://dashboard.helius.dev/> → API Keys.
2. Create a new key; update `HELIUS_API_KEY` in the production secret store
   (vault / `.env` on `chimera-01`, never in git).
3. Revoke / delete the old key.
4. Confirm the operator still connects: `curl` the new RPC endpoint and watch
   `helius_rpc_*` metrics.

### 2. Webhook HMAC secret (`CHIMERA_SECURITY__WEBHOOK_SECRET`)
1. Generate a new secret: `openssl rand -hex 32`.
2. Update `CHIMERA_SECURITY__WEBHOOK_SECRET` in the production secret store.
3. Re-register Helius webhooks with the new secret
   (`tools/register_helius_webhooks.sh`) and confirm Helius is signing with it.
4. Restart the operator so inbound webhook validation uses the new secret.
5. Verify a real webhook is accepted (200) and a bad-signature one is rejected
   (401).

### 3. Jupiter API key
1. <https://portal.jup.ag/> → API keys.
2. Create a new key; update `JUPITER_API_KEY` in the production secret store.
3. Revoke the old key.
4. Confirm Jupiter quote/swap calls succeed with the new key.

### 4. History cleanup (optional, defense-in-depth)
Rotating makes the leaked values useless, so this is optional. If repo history
must also be rewritten, use `git filter-repo` to purge the old deploy scripts
across all history, then force-push and have all clones re-clone. Coordinate
this — it rewrites public history.

## Evidence record (fill in before merge)

| Credential           | Rotated? (Y/N) | Rotation timestamp (UTC) | New key id / first-8 | Verified by |
|----------------------|----------------|--------------------------|----------------------|-------------|
| Helius RPC key       |                |                          |                      |             |
| Webhook HMAC secret  |                |                          | n/a (random)         |             |
| Jupiter API key      |                |                          |                      |             |

**Merge is blocked until all three rows are "Y" with a verification signature.**
