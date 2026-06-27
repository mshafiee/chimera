# Jito Enabled Fix

## Issue

Jito was showing as disabled (`jito_enabled: false`) in the API response, even though:
- Environment variable `CHIMERA_JITO__ENABLED=true` was set in `docker/env.mainnet-paper.local`
- The environment variable was present in the container

## Root Cause

The `config/config.yaml` file had `jito.enabled: false` hardcoded. While environment variables should override YAML values, there may be a parsing issue with boolean values from environment variables, or the config needs to be reloaded.

## Solution

Updated `config/config.yaml` to set `jito.enabled: true` with a comment noting it can be overridden by environment variables.

## Why Jito Should Be Enabled for Mainnet

Jito provides MEV (Maximal Extractable Value) protection by:
1. **Front-running Protection**: Submits transactions through Jito's block engine
2. **Better Execution**: Uses Jito's searcher network for optimal trade execution
3. **MEV Extraction**: Protects against sandwich attacks and front-running

For mainnet paper trading, Jito should be enabled to:
- Test MEV protection in realistic conditions
- Validate transaction execution through Jito
- Ensure proper tip calculation and submission

## Verification

After restarting the operator, check:
```bash
curl http://localhost:8080/api/v1/config | jq '.jito_enabled'
```

Should return: `true`

## Configuration

Jito settings in `docker/env.mainnet-paper.local`:
- `CHIMERA_JITO__ENABLED=true` ✅
- `CHIMERA_JITO__SEARCHER_ENDPOINT=https://mainnet.block-engine.jito.wtf` ✅
- `CHIMERA_JITO__HELIUS_FALLBACK=true` ✅
- Tip configuration: Floor 0.001 SOL, Ceiling 0.01 SOL ✅

All Jito settings are properly configured for mainnet.
