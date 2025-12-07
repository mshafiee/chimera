# Chaos Tests

Fault injection and resilience testing for Chimera Operator.

## Tests

### 1. RPC Failure Test

Tests RPC endpoint failure and fallback:
- Simulate Helius connection failure
- Verify automatic fallback to QuickNode
- Verify Spear is disabled during fallback
- Measure recovery time

### 2. Database Lock Test

Tests SQLite busy timeout behavior:
- Simulate concurrent database access
- Verify retry 3x with backoff
- Verify proper error handling

### 3. Network Partition Test

Tests handling of network issues:
- Simulate network timeout
- Verify graceful degradation
- Verify alert notifications

## Running Chaos Tests

```bash
# Run all chaos tests
cargo test --test chaos_tests

# Run specific test
cargo test --test chaos_tests rpc_failure

# With logging
RUST_LOG=debug cargo test --test chaos_tests
```

## Test Scenarios from PDD

| Scenario | Expected Behavior |
|----------|-------------------|
| Kill Helius connection mid-trade | Fallback to QuickNode, Spear disabled |
| SQLite locked | Retry 3x with backoff |
| Memory pressure | Graceful degradation |
| RPC rate limited | Exponential backoff |

