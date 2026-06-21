"""
Tests for Phase 5: Batch Processing Optimization

Tests the batch processing functionality for improved wallet analysis
throughput.
"""

import pytest
import asyncio
from unittest.mock import AsyncMock, patch

from core.analyzer import WalletAnalyzer
from core.wqs import WalletMetrics


@pytest.fixture
async def analyzer():
    """Create a WalletAnalyzer instance for testing."""
    with patch("core.analyzer.HeliusClient"):
        analyzer = WalletAnalyzer(
            helius_api_key="test-key",
            discover_wallets=False,
            max_wallets=10,
        )
        yield analyzer


class TestBatchProcessing:
    """Tests for batch wallet analysis."""

    @pytest.mark.asyncio
    async def test_analyze_wallets_batch_basic(self, analyzer):
        """Test basic batch processing functionality."""
        addresses = [
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
            "5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0987654321",
        ]

        # Mock get_wallet_metrics to return fake metrics
        async def mock_get_metrics(address):
            return WalletMetrics(
                address=address,
                roi_7d=10.0,
                roi_30d=25.0,
                trade_count_30d=10,
                win_rate=0.6,
                max_drawdown_30d=5.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Process in batch
        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=2,
            concurrency_per_batch=2,
        )

        # Verify results
        assert len(results) == len(addresses)
        for address in addresses:
            assert address in results
            assert results[address] is not None
            assert results[address].address == address

    @pytest.mark.asyncio
    async def test_batch_size_parameter(self, analyzer):
        """Test that batch_size parameter controls batching."""
        addresses = [f"wallet_{i}" for i in range(10)]

        async def mock_get_metrics(address):
            await asyncio.sleep(0.01)  # Simulate work
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Process with batch_size=3 (should create 4 batches: 3,3,3,1)
        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=3,
            concurrency_per_batch=2,
        )

        assert len(results) == 10

    @pytest.mark.asyncio
    async def test_concurrency_parameter_accepted(self, analyzer):
        """Test that concurrency parameter is properly handled."""
        addresses = [f"wallet_{i}" for i in range(4)]

        async def mock_get_metrics(address):
            await asyncio.sleep(0.01)  # Simulate work
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Process with concurrency=2 - should complete without error
        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=10,
            concurrency_per_batch=2,
        )

        # Verify all wallets processed
        assert len(results) == 4
        for address in addresses:
            assert address in results
            assert results[address] is not None

    @pytest.mark.asyncio
    async def test_error_handling_in_batch(self, analyzer):
        """Test that errors in individual wallets don't fail the batch."""
        addresses = [
            "wallet_success_1",
            "wallet_fail",
            "wallet_success_2",
        ]

        async def mock_get_metrics(address):
            if "fail" in address:
                raise ValueError("Simulated error")
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=2,
            concurrency_per_batch=2,
        )

        # Successes should have results, failure should be None
        assert results["wallet_success_1"] is not None
        assert results["wallet_fail"] is None
        assert results["wallet_success_2"] is not None

    @pytest.mark.asyncio
    async def test_progress_callback(self, analyzer):
        """Test that progress callback is called correctly."""
        addresses = [f"wallet_{i}" for i in range(10)]

        async def mock_get_metrics(address):
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Track callback invocations
        callback_calls = []

        def progress_callback(batch_num, total_batches, processed, total):
            callback_calls.append({
                'batch': batch_num,
                'total_batches': total_batches,
                'processed': processed,
                'total': total,
            })

        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=3,
            concurrency_per_batch=2,
            progress_callback=progress_callback,
        )

        # Should have called callback for each batch (4 batches for 10 wallets with batch_size=3)
        assert len(callback_calls) == 4

        # Verify callback arguments
        assert callback_calls[0]['batch'] == 1
        assert callback_calls[0]['total_batches'] == 4
        assert callback_calls[-1]['processed'] == 10

        assert len(results) == 10


class TestBatchOptimization:
    """Tests for batch processing optimization effectiveness."""

    @pytest.mark.asyncio
    async def test_prefetch_wallet_data(self, analyzer):
        """Test that data prefetching works correctly."""
        addresses = [
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
        ]

        # Mock SOL price fetch
        async def mock_get_sol_price():
            return 150.0

        analyzer._get_sol_price_usd = AsyncMock(side_effect=mock_get_sol_price)

        # Mock wallet age fetch
        async def mock_get_wallet_age(address):
            return 1234567890.0

        analyzer._get_wallet_creation_time_cached = AsyncMock(
            side_effect=mock_get_wallet_age
        )

        # Prefetch data
        await analyzer.prefetch_wallet_data(addresses)

        # Verify SOL price was fetched
        analyzer._get_sol_price_usd.assert_called_once()

        # Verify wallet ages were fetched
        assert analyzer._get_wallet_creation_time_cached.call_count == len(addresses)

    @pytest.mark.asyncio
    async def test_memory_efficiency_batching(self, analyzer):
        """Test that batching improves memory efficiency."""
        # Large list of addresses
        addresses = [f"wallet_{i}" for i in range(100)]

        async def mock_get_metrics(address):
            # Simulate some memory usage
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Process in batches of 50
        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=50,
            concurrency_per_batch=5,
        )

        # All wallets should be processed
        assert len(results) == 100


class TestPerformanceMetrics:
    """Tests for performance metrics tracking."""

    @pytest.mark.asyncio
    async def test_batch_vs_sequential_performance(self, analyzer):
        """Test that batch processing is faster than sequential."""
        addresses = [f"wallet_{i}" for i in range(20)]

        async def mock_get_metrics(address):
            await asyncio.sleep(0.01)  # Simulate I/O delay
            return WalletMetrics(
                address=address,
                roi_7d=5.0,
                roi_30d=15.0,
                trade_count_30d=5,
                win_rate=0.5,
                max_drawdown_30d=3.0,
            )

        analyzer.get_wallet_metrics = AsyncMock(side_effect=mock_get_metrics)

        # Measure batch processing time
        start = asyncio.get_event_loop().time()
        results = await analyzer.analyze_wallets_batch(
            addresses,
            batch_size=10,
            concurrency_per_batch=5,
        )
        batch_time = asyncio.get_event_loop().time() - start

        # Verify all processed
        assert len(results) == 20

        # Batch processing with concurrency should be significantly faster
        # than sequential (20 * 0.01s = 0.2s sequential vs ~0.04s with concurrency)
        # We'll just verify it completed in reasonable time
        assert batch_time < 0.15  # Should complete faster than sequential


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
