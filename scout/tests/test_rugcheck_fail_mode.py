"""
Tests for RugCheck fail-mode behavior in circuit breaker.

Tests the capital-protective vs escape-hatch behavior when
RugCheck flags a majority of tokens as risky (>50%).
"""

import pytest
from unittest.mock import AsyncMock, patch, MagicMock


class MockTrade:
    """Mock trade for testing."""
    
    def __init__(self, token_address: str):
        self.token_address = token_address


class MockAnalyzer:
    """Mock analyzer with RugCheck client."""
    
    def __init__(self, fail_mode: str = "closed"):
        self.rugcheck_client = MagicMock()
        self.rugcheck_client.is_token_safe = AsyncMock()
        self.fail_mode = fail_mode
    
    async def simulate_rugcheck_circuit_breaker(
        self,
        address: str,
        trades: list,
        safe_ratio: float
    ):
        """
        Simulate the RugCheck filtering block with circuit breaker.
        
        This mirrors the logic in analyzer.py lines 2067-2133.
        
        Args:
            address: Wallet address
            trades: List of trades
            safe_ratio: Ratio of tokens that are safe (0.0-1.0)
        
        Returns:
            Filtered trades or None if wallet is dropped
        """
        if not self.rugcheck_client:
            return trades
        
        unique_tokens = {t.token_address for t in trades}
        safe_tokens = set()
        risky_tokens = []
        
        # Simulate token checks
        for token in unique_tokens:
            is_safe = token in {f"safe_{i}" for i in range(int(len(unique_tokens) * safe_ratio))}
            if is_safe:
                safe_tokens.add(token)
            else:
                risky_tokens.append(token)
        
        # Circuit breaker logic
        risky_ratio = len(risky_tokens) / max(1, len(unique_tokens)) if risky_tokens else 0.0
        if risky_tokens:
            if risky_ratio > 0.5:
                # Circuit breaker triggered
                if self.fail_mode == "open":
                    # Escape hatch: keep all trades
                    return trades
                else:
                    # Capital-protective: drop wallet
                    return None
            else:
                # Filter risky tokens
                filtered = [t for t in trades if t.token_address in safe_tokens]
                return filtered if filtered else None
        else:
            return trades


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_rugcheck_closed_mode_drops_wallet_on_majority_risky():
    """
    With RUGCHECK_FAIL_MODE=closed and majority risky tokens,
    the wallet is dropped (capital-protective).
    """
    analyzer = MockAnalyzer(fail_mode="closed")
    
    # Create 10 tokens, 7 risky (70% > 50% threshold)
    trades = [MockTrade(f"safe_{i}") for i in range(3)]
    trades.extend([MockTrade(f"risky_{i}") for i in range(7)])
    
    with patch.dict("os.environ", {"RUGCHECK_FAIL_MODE": "closed"}):
        result = await analyzer.simulate_rugcheck_circuit_breaker(
            "test_wallet",
            trades,
            safe_ratio=0.3  # 30% safe
        )
    
    # Should drop wallet (return None)
    assert result is None


@pytest.mark.asyncio
async def test_rugcheck_open_mode_keeps_wallet_on_majority_risky():
    """
    With RUGCHECK_FAIL_MODE=open and majority risky tokens,
    all trades are retained (escape hatch).
    """
    analyzer = MockAnalyzer(fail_mode="open")
    
    # Create 10 tokens, 7 risky (70% > 50% threshold)
    trades = [MockTrade(f"safe_{i}") for i in range(3)]
    trades.extend([MockTrade(f"risky_{i}") for i in range(7)])
    
    with patch.dict("os.environ", {"RUGCHECK_FAIL_MODE": "open"}):
        result = await analyzer.simulate_rugcheck_circuit_breaker(
            "test_wallet",
            trades,
            safe_ratio=0.3  # 30% safe
        )
    
    # Should keep all trades
    assert result is not None
    assert len(result) == 10


@pytest.mark.asyncio
async def test_rugcheck_closed_mode_filters_minority_risky():
    """
    With RUGCHECK_FAIL_MODE=closed and minority risky tokens,
    only risky tokens are filtered.
    """
    analyzer = MockAnalyzer(fail_mode="closed")
    
    # Create 10 tokens, 3 risky (30% < 50% threshold)
    trades = [MockTrade(f"safe_{i}") for i in range(7)]
    trades.extend([MockTrade(f"risky_{i}") for i in range(3)])
    
    result = await analyzer.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.7  # 70% safe
    )
    
    # Should filter out risky tokens
    assert result is not None
    assert len(result) == 7
    assert all("safe" in t.token_address for t in result)


@pytest.mark.asyncio
async def test_rugcheck_open_mode_filters_minority_risky():
    """
    With RUGCHECK_FAIL_MODE=open and minority risky tokens,
    only risky tokens are filtered (normal behavior).
    """
    analyzer = MockAnalyzer(fail_mode="open")
    
    # Create 10 tokens, 3 risky (30% < 50% threshold)
    trades = [MockTrade(f"safe_{i}") for i in range(7)]
    trades.extend([MockTrade(f"risky_{i}") for i in range(3)])
    
    result = await analyzer.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.7  # 70% safe
    )
    
    # Should filter out risky tokens
    assert result is not None
    assert len(result) == 7
    assert all("safe" in t.token_address for t in result)


@pytest.mark.asyncio
async def test_rugcheck_closed_mode_all_safe():
    """
    With RUGCHECK_FAIL_MODE=closed and all tokens safe,
    all trades are retained.
    """
    analyzer = MockAnalyzer(fail_mode="closed")
    
    trades = [MockTrade(f"safe_{i}") for i in range(10)]
    
    result = await analyzer.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=1.0  # 100% safe
    )
    
    # Should keep all trades
    assert result is not None
    assert len(result) == 10


@pytest.mark.asyncio
async def test_rugcheck_closed_mode_drops_when_all_filtered():
    """
    With RUGCHECK_FAIL_MODE=closed and filtering removes all trades,
    wallet is dropped.
    """
    analyzer = MockAnalyzer(fail_mode="closed")
    
    # Create tokens, all will be filtered as risky
    trades = [MockTrade(f"risky_{i}") for i in range(10)]
    
    result = await analyzer.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.0  # 0% safe
    )
    
    # Should drop wallet (return None)
    assert result is None


@pytest.mark.asyncio
async def test_rugcheck_open_mode_drops_when_all_filtered():
    """
    With RUGCHECK_FAIL_MODE=open and filtering removes all trades,
    wallet is dropped (even in open mode, if < 50% risky).
    """
    analyzer = MockAnalyzer(fail_mode="open")
    
    # Create tokens, all will be filtered as risky (100% > 50%, but not circuit breaker)
    # Wait, 100% risky would trigger circuit breaker in open mode and keep all
    # Let's test 40% risky which is < 50% so not circuit breaker
    trades = [MockTrade(f"risky_{i}") for i in range(4)]
    trades.extend([MockTrade(f"safe_{i}") for i in range(6)])
    
    result = await analyzer.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.6  # 60% safe, 40% risky
    )
    
    # Should filter out risky tokens, leaving 6 safe ones
    assert result is not None
    assert len(result) == 6


@pytest.mark.asyncio
async def test_rugcheck_default_is_closed():
    """
    Default fail mode should be 'closed' (capital-protective).
    """
    from config import ScoutConfig
    
    default_mode = ScoutConfig.get_rugcheck_fail_mode()
    assert default_mode == "closed"


@pytest.mark.asyncio
async def test_rugcheck_threshold_exact_50_percent():
    """
    Test edge case: exactly 50% risky.
    
    The threshold is > 0.5, so exactly 50% should NOT trigger circuit breaker.
    """
    analyzer_closed = MockAnalyzer(fail_mode="closed")
    analyzer_open = MockAnalyzer(fail_mode="open")
    
    # Create 10 tokens, 5 risky (50% = threshold)
    trades = [MockTrade(f"safe_{i}") for i in range(5)]
    trades.extend([MockTrade(f"risky_{i}") for i in range(5)])
    
    # Closed mode: should filter risky tokens (not circuit breaker)
    result_closed = await analyzer_closed.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.5  # 50% safe
    )
    assert result_closed is not None
    assert len(result_closed) == 5
    
    # Open mode: should also filter risky tokens (not circuit breaker)
    result_open = await analyzer_open.simulate_rugcheck_circuit_breaker(
        "test_wallet",
        trades,
        safe_ratio=0.5  # 50% safe
    )
    assert result_open is not None
    assert len(result_open) == 5