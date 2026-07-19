# Vault Keypair Management

Procedure for importing the Solana trading keypair into the encrypted vault
(`config/secrets.enc`) so the operator can sign Live/Devnet transactions
without reading the raw keypair from environment variables.

**Paper mode is unaffected** — the vault is only consulted for
`TradeMode::Devnet` or `TradeMode::Live` (`main.rs:123-143`). This runbook is
go-live preparation, not an urgent fix.

## Background

- The vault file is **AES-256-GCM** encrypted with `CHIMERA_VAULT_KEY`
  (a 32-byte / 64-hex-char key).
- `Vault::save_secrets` writes atomically: a sibling `.tmp` file with mode
  `0600` is renamed into place, so a mid-write crash cannot corrupt the vault.
- Once `CHIMERA_VAULT_KEY` is set **AND** `config/secrets.enc` exists,
  environment-variable fallback for secrets is **disabled**
  (`vault.rs:276-303`). The vault becomes the sole source of truth, and a
  decryption failure is a hard error (operator refuses to start).
- The operator's normal container mount is `./config:/app/config:ro`
  (`docker-compose.yml:117`). Population is therefore a one-off `docker compose
  run` with a `:rw` volume override — the running operator never writes the
  vault itself.

## Prerequisites

- Operator currently running in **Paper mode** (`CHIMERA_TRADE_MODE=paper`).
- A Solana keypair file accessible on the host (e.g. `/opt/chimera/id.json`).
  Acceptable input formats (auto-detected):
  - Solana CLI JSON byte-array (`~/.config/solana/id.json`).
  - Base58 (87-88 chars — the **64-byte keypair**, NOT the 32-byte pubkey).
  - Hex (128 chars).
- `HELIUS_API_KEY` and `CHIMERA_SECURITY__WEBHOOK_SECRET` already set in
  `/opt/chimera/.env`.

## Step 1 — Generate the vault key

The vault key is the **only** secret that must be backed up offline. If it is
lost, the vault is unrecoverable.

```bash
openssl rand -hex 32
```

Append to `/opt/chimera/.env`:

```bash
CHIMERA_VAULT_KEY=<hex>
```

> Do **not** auto-generate this inside the operator — the key must outlive any
> single process and be backed up offline (e.g. password manager, hardware
> security module). The `import_keypair` tool refuses to run without it.

## Step 2 — Import the keypair

Run on the production server, in-container, with a writable config override:

```bash
cd /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-paper docker compose -f docker-compose.yml \
  -f docker-compose-haproxy.yml build operator

# Dry-run first — validates the keypair and prints the plan without writing:
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml \
  run --rm --no-deps \
  -v /opt/chimera/config:/app/config:rw \
  -e CHIMERA_SECURITY__WEBHOOK_SECRET \
  -e HELIUS_API_KEY \
  operator /app/import_keypair --dry-run < /path/to/id.json

# Real import:
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml \
  run --rm --no-deps \
  -v /opt/chimera/config:/app/config:rw \
  -e CHIMERA_SECURITY__WEBHOOK_SECRET \
  -e HELIUS_API_KEY \
  operator /app/import_keypair < /path/to/id.json
```

Expected output:

```
Derived pubkey: <base58>
No existing vault at config/secrets.enc — creating a new one.

=== Vault plan ===
  vault path:           config/secrets.enc
  wallet pubkey:        <base58>
  wallet_private_key:   [REDACTED, 128 hex chars]
  webhook_secret:       [REDACTED, N chars]
  ...
Vault written: config/secrets.enc (mode 0600, atomic rename)
Round-trip validation OK — vault decrypts and keypair loads.

Done. Derived trading pubkey: <base58>
```

Confirm the file exists with the correct permissions:

```bash
ls -l /opt/chimera/config/secrets.enc
# Expect: -rw------- (mode 0600), non-zero size (~400 bytes)
```

Secure the source keypair file once the round-trip succeeds:

```bash
shred -u /path/to/id.json   # or: gshred -u on BSD/macOS
```

## Step 3 — Restart and verify

```bash
COMPOSE_PROFILE=mainnet-paper docker compose -f docker-compose.yml \
  -f docker-compose-haproxy.yml up -d --force-recreate operator

docker logs chimera-operator 2>&1 | grep -E \
  "Loaded secrets from encrypted vault|Portfolio capital refresh|Wallet keypair unavailable"
```

Expect:

- `"Loaded secrets from encrypted vault"` (`vault.rs:281`) — vault path active.
- `"Portfolio capital refresh task spawned (60s interval)"` (`main.rs:2041`) —
  wallet keypair loads successfully.

**NOT** any of:

- `"Wallet keypair unavailable"` (`main.rs:2044`) — keypair load failed.
- `"CHIMERA_VAULT_KEY is set and vault file exists but decryption failed"` —
  wrong vault key or corrupt file.

Confirm the health endpoint still reports Paper mode with the circuit breaker
active and trading allowed — nothing about the import should have changed
runtime posture:

```bash
curl -s http://chimera-01.moez.tech:8080/health | jq .
# Expect: trade_mode=paper, cb=ACTIVE, trading_allowed=true
```

## Rotation procedure

To rotate the trading keypair, simply re-run Step 2 with a new keypair file.
The tool:

- Loads the existing vault with the current `CHIMERA_VAULT_KEY`.
- Preserves `webhook_secret`, `webhook_secret_previous`, `rpc_api_key`, and
  `fallback_rpc_api_key` (unless the corresponding env vars override them).
- Overwrites `wallet_private_key` with the new hex-canonical keypair.
- Re-encrypts with a fresh AES-GCM nonce.

The old keypair bytes are overwritten in place — there is no history of the
previous keypair inside the vault file. (Git history is unaffected because
`config/secrets.enc` is gitignored.)

## Live-mode preflight probe (optional)

Before the real go-live flip, you can validate the entire Live path without
actually placing trades:

```bash
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml \
  run --rm -e CHIMERA_TRADE_MODE=live operator /app/chimera_operator
```

Watch for `"Pre-flight passed: vault, keypair, and RPC reachable (Live)"`
(`main.rs:139-142`). Ctrl-C immediately afterwards — no signal will arrive
unless your scouts are already routing to this operator.

## Safety properties enforced by `import_keypair`

- The keypair is **never** accepted as a CLI argument (would be visible in
  `ps`). Read from stdin or `--keypair-file` only.
- The input buffer is explicitly `zeroize()`d after normalization.
- All log output redacts secrets — only the derived base58 pubkey and vault
  path are printed.
- The tool hard-fails if `CHIMERA_SECURITY__WEBHOOK_SECRET` is unset/empty,
  preventing creation of a vault that would silently break inbound webhook
  HMAC verification.
- After writing, the tool immediately re-opens the file and round-trips
  through `load_wallet_keypair`. If anything fails, the prior vault is
  **restored from a `secrets.enc.bak` backup** (or removed if no prior vault
  existed) so the operator never starts against a corrupt vault. If the
  restore itself fails, the tool prints the backup path and instructs the
  operator to recover manually — do NOT re-run `import_keypair` without
  recovering the backup first.
- A wrong `CHIMERA_VAULT_KEY` against an existing vault is rejected — the
  tool never overwrites a vault it cannot first read.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `CHIMERA_VAULT_KEY environment variable not set` | Env var not loaded | `set -a; source /opt/chimera/.env; set +a` then re-run |
| `Key must be 32 bytes (64 hex chars)` | Vault key malformed | Regenerate with `openssl rand -hex 32` |
| `Vault file exists ... but could not be decrypted` | Wrong `CHIMERA_VAULT_KEY` vs. the one that created the vault | Restore the correct key from offline backup, or `rm config/secrets.enc` and re-import from scratch |
| `CHIMERA_SECURITY__WEBHOOK_SECRET is not set or empty` | Webhook secret missing from env | Set it before importing — required even in vault mode |
| `Decoded bytes are not a valid Ed25519 keypair` | The 64 bytes you supplied are not a real keypair (e.g. you passed a pubkey, or random bytes) | Regenerate with `solana-keygen new --no-bip39-passphrase --silent --outfile id.json` |
| `Decoded keypair is N bytes — expected exactly 64` | Wrong-length input | For JSON, ensure 64 entries; for base58, ensure 87-88 chars (the keypair, not the 44-char pubkey) |
| `Unrecognized keypair format` | Input isn't JSON/base58/hex | Re-export the keypair from `solana-keygen` or Phantom in a supported format |
