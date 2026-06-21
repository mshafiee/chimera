"""
Helius Developer Plan Optimizer

This module implements Helius-specific optimizations for Developer Plan constraints:
- 10M credits per month budget management
- 50 requests per second rate limiting
- Request batching and optimization
- Priority-based request queuing
- Intelligent resource allocation
- Growth goal optimization ($200 → $1000)

Features:
- Smart request batching
- Priority queue management
- Rate limit optimization
- Credit cost minimization
- Growth-focused resource allocation
"""

import os
import time
import asyncio
import logging
from typing import Dict, List, Optional, Tuple, Any, Callable
from dataclasses import dataclass
from enum import Enum
from collections import deque
import threading

logger = logging.getLogger(__name__)


class RequestType(Enum):
    """Types of Helius API requests."""

    # Discovery operations
    WALLET_DISCOVERY = "wallet_discovery"
    TRANSACTION_FETCH = "transaction_fetch"
    SIGNATURE_QUERY = "signature_query"

    # Analysis operations
    WALLET_ANALYSIS = "wallet_analysis"
    SWAP_PARSING = "swap_parsing"
    POSITION_TRACKING = "position_tracking"

    # Enrichment operations
    TOKEN_METADATA = "token_metadata"
    TOKEN_CREATION = "token_creation"
    PRICE_FETCH = "price_fetch"

    # Validation operations
    BACKTEST_VALIDATION = "backtest_validation"
    LIQUIDITY_CHECK = "liquidity_check"


class GrowthPriority(Enum):
    """Priority levels for growth goal optimization."""

    CRITICAL = 1   # Must execute for growth goal
    HIGH = 2       # Important for growth
    MEDIUM = 3     # Normal priority
    LOW = 4        # Optional


@dataclass
class QueuedRequest:
    """A queued API request with metadata."""

    request_type: RequestType
    priority: GrowthPriority
    credit_cost: int
    growth_value: float  # 0.0-1.0 expected impact on growth goal
    callback: Callable
    args: tuple
    kwargs: dict
    created_at: float
    timeout: float = 30.0
    retry_count: int = 0
    max_retries: int = 3

    def __post_init__(self):
        if self.created_at == 0.0:
            self.created_at = time.time()

    @property
    def age(self) -> float:
        """Request age in seconds."""
        return time.time() - self.created_at

    @property
    def is_expired(self) -> bool:
        """Check if request has expired."""
        return self.age > self.timeout

    @property
    def should_retry(self) -> bool:
        """Check if request should be retried."""
        return self.retry_count < self.max_retries


class HeliusOptimizer:
    """
    Helius Developer Plan optimizer.

    Features:
    - Smart request batching
    - Priority queue management
    - Rate limit optimization
    - Growth goal optimization
    """

    # Developer Plan constraints
    MAX_CREDITS_PER_MONTH = 10_000_000
    MAX_REQUESTS_PER_SECOND = 50
    DAILY_CREDIT_TARGET = MAX_CREDITS_PER_MONTH / 30  # ~333K per day

    # Credit costs per request type (approximate)
    CREDIT_COSTS = {
        RequestType.WALLET_DISCOVERY: 5,
        RequestType.TRANSACTION_FETCH: 10,
        RequestType.SIGNATURE_QUERY: 2,
        RequestType.WALLET_ANALYSIS: 50,
        RequestType.SWAP_PARSING: 1,
        RequestType.POSITION_TRACKING: 2,
        RequestType.TOKEN_METADATA: 2,
        RequestType.TOKEN_CREATION: 3,
        RequestType.PRICE_FETCH: 1,
        RequestType.BACKTEST_VALIDATION: 20,
        RequestType.LIQUIDITY_CHECK: 5,
    }

    def __init__(self):
        """Initialize the optimizer."""
        # Request queue
        self._request_queue: deque[QueuedRequest] = deque()
        self._queue_lock = threading.Lock()

        # Rate limiting
        self._request_times: List[float] = []
        self._rate_limit_window = 1.0  # 1 second window
        self._max_requests_per_second = self.MAX_REQUESTS_PER_SECOND

        # Credit tracking
        self._credits_used_today = 0
        self._day_start_time = time.time()

        # Growth optimization
        self._growth_optimized = os.getenv("SCOUT_GROWTH_OPTIMIZED", "true").lower() == "true"
        self._current_capital = float(os.getenv("SCOUT_CURRENT_CAPITAL", "200.0"))
        self._target_capital = float(os.getenv("SCOUT_TARGET_CAPITAL", "1000.0"))

        # Configuration
        self._enable_batching = os.getenv("SCOUT_ENABLE_BATCHING", "true").lower() == "true"
        self._batch_size = int(os.getenv("SCOUT_BATCH_SIZE", "10"))
        self._batch_timeout = float(os.getenv("SCOUT_BATCH_TIMEOUT", "2.0"))

        # Performance tracking
        self._stats = {
            'requests_processed': 0,
            'requests_failed': 0,
            'credits_saved': 0,
            'batches_processed': 0,
        }

        logger.info("Helius Optimizer initialized")
        logger.info(f"  Growth optimized: {self._growth_optimized}")
        logger.info(f"  Current capital: ${self._current_capital:.0f}")
        logger.info(f"  Target capital: ${self._target_capital:.0f}")
        logger.info(f"  Request batching: {self._enable_batching}")

    def _can_make_request(self, cost: int) -> bool:
        """Check if we can make a request given current constraints."""
        # Check rate limit
        if not self._check_rate_limit():
            return False

        # Check daily budget
        remaining_budget = self.DAILY_CREDIT_TARGET - self._credits_used_today
        return remaining_budget >= cost

    def _check_rate_limit(self) -> bool:
        """Check if we're within rate limits."""
        now = time.time()

        # Clean old request times
        self._request_times = [t for t in self._request_times
                              if now - t < self._rate_limit_window]

        current_rps = len(self._request_times) / self._rate_limit_window

        return current_rps < self._max_requests_per_second

    def _wait_for_rate_limit(self):
        """Wait until we can make a request."""
        while not self._check_rate_limit():
            time.sleep(0.1)

    def queue_request(self, request_type: RequestType, callback: Callable,
                    priority: GrowthPriority = GrowthPriority.MEDIUM,
                    growth_value: float = 0.5, *args, **kwargs) -> bool:
        """
        Queue a request for execution.

        Args:
            request_type: Type of request
            callback: Function to execute
            priority: Growth priority
            growth_value: Expected impact on growth goal (0.0-1.0)
            *args: Callback arguments
            **kwargs: Callback keyword arguments

        Returns:
            True if queued successfully
        """
        credit_cost = self.CREDIT_COSTS.get(request_type, 10)

        request = QueuedRequest(
            request_type=request_type,
            priority=priority,
            credit_cost=credit_cost,
            growth_value=growth_value,
            callback=callback,
            args=args,
            kwargs=kwargs,
            created_at=time.time(),
        )

        with self._queue_lock:
            self._request_queue.append(request)

        logger.debug(f"Queued {request_type.value} request (priority: {priority.name})")
        return True

    def _should_process_request(self, request: QueuedRequest) -> bool:
        """Determine if a request should be processed."""
        # Check if expired
        if request.is_expired:
            logger.debug(f"Request expired: {request.request_type.value}")
            return False

        # Check credit availability
        if not self._can_make_request(request.credit_cost):
            return False

        # Growth optimization: prioritize high-growth requests
        if self._growth_optimized:
            # Skip low-growth requests when budget is tight
            budget_ratio = (self.DAILY_CREDIT_TARGET - self._credits_used_today) / self.DAILY_CREDIT_TARGET

            if budget_ratio < 0.3 and request.priority == GrowthPriority.LOW:
                return False

            # Prioritize critical growth requests
            if request.priority == GrowthPriority.CRITICAL:
                return True

        return True

    async def process_queue(self):
        """Process queued requests."""
        if not self._request_queue:
            return

        logger.info(f"Processing {len(self._request_queue)} queued requests")

        processed = 0
        failed = 0

        with self._queue_lock:
            # Sort queue by priority and growth value
            sorted_requests = sorted(
                self._request_queue,
                key=lambda r: (r.priority.value, -r.growth_value, r.created_at)
            )

            # Clear the queue
            self._request_queue.clear()

        # Process requests
        for request in sorted_requests:
            if not self._should_process_request(request):
                continue

            try:
                # Wait for rate limit if needed
                self._wait_for_rate_limit()

                # Execute request
                if asyncio.iscoroutinefunction(request.callback):
                    await request.callback(*request.args, **request.kwargs)
                else:
                    request.callback(*request.args, **request.kwargs)

                # Update credits used
                self._credits_used_today += request.credit_cost
                self._request_times.append(time.time())

                processed += 1
                logger.debug(f"Processed {request.request_type.value} request")

            except Exception as e:
                logger.warning(f"Request failed: {request.request_type.value} - {e}")

                # Retry if needed
                if request.should_retry:
                    request.retry_count += 1
                    with self._queue_lock:
                        self._request_queue.append(request)

                failed += 1

        self._stats['requests_processed'] += processed
        self._stats['requests_failed'] += failed

        logger.info(f"Processed {processed} requests, {failed} failed")

    async def batch_requests(self, requests: List[Tuple[RequestType, Callable, tuple, dict]]) -> List[Any]:
        """
        Execute multiple requests in batch.

        Args:
            requests: List of (request_type, callback, args, kwargs) tuples

        Returns:
            List of results
        """
        if not self._enable_batching:
            results = []
            for request_type, callback, args, kwargs in requests:
                self._wait_for_rate_limit()
                try:
                    if asyncio.iscoroutinefunction(callback):
                        result = await callback(*args, **kwargs)
                    else:
                        result = callback(*args, **kwargs)
                    results.append(result)
                except Exception as e:
                    logger.warning(f"Batch request failed: {e}")
                    results.append(None)
            return results

        # Batch processing with optimized grouping
        grouped = self._group_requests_by_type(requests)
        results = []

        for request_type, group in grouped.items():
            # Calculate credit cost
            credit_cost = self.CREDIT_COSTS.get(request_type, 10) * len(group)

            if not self._can_make_request(credit_cost):
                logger.warning(f"Insufficient credits for batch of {len(group)} {request_type.value}")
                # Process individually
                for callback, args, kwargs in group:
                    results.append(await self._execute_single(callback, args, kwargs))
                continue

            # Process batch
            batch_results = await self._process_batch(request_type, group)
            results.extend(batch_results)

            self._stats['batches_processed'] += 1

        return results

    def _group_requests_by_type(self, requests: List[Tuple[RequestType, Callable, tuple, dict]]) -> Dict[RequestType, List[Tuple[Callable, tuple, dict]]]:
        """Group requests by type for batch processing."""
        grouped = {}

        for request_type, callback, args, kwargs in requests:
            if request_type not in grouped:
                grouped[request_type] = []
            grouped[request_type].append((callback, args, kwargs))

        return grouped

    async def _process_batch(self, request_type: RequestType,
                           group: List[Tuple[Callable, tuple, dict]]) -> List[Any]:
        """Process a batch of requests of the same type."""
        results = []

        for callback, args, kwargs in group:
            self._wait_for_rate_limit()

            try:
                if asyncio.iscoroutinefunction(callback):
                    result = await callback(*args, **kwargs)
                else:
                    result = callback(*args, **kwargs)
                results.append(result)
            except Exception as e:
                logger.warning(f"Batch item failed: {e}")
                results.append(None)

        return results

    async def _execute_single(self, callback: Callable, args: tuple, kwargs: dict) -> Any:
        """Execute a single request."""
        self._wait_for_rate_limit()

        try:
            if asyncio.iscoroutinefunction(callback):
                return await callback(*args, **kwargs)
            else:
                return callback(*args, **kwargs)
        except Exception as e:
            logger.warning(f"Single request failed: {e}")
            return None

    def optimize_wallet_analysis(self, wallet_count: int) -> int:
        """
        Calculate optimal wallet analysis count given current budget.

        Args:
            wallet_count: Total wallets to analyze

        Returns:
            Optimized number of wallets to analyze
        """
        # Calculate remaining budget
        remaining_budget = self.DAILY_CREDIT_TARGET - self._credits_used_today

        # Estimate cost per wallet
        cost_per_wallet = self.CREDIT_COSTS[RequestType.WALLET_ANALYSIS]

        # Calculate max wallets we can afford
        max_wallets = int(remaining_budget / cost_per_wallet)

        # Growth optimization: focus on high-conviction wallets
        if self._growth_optimized:
            # Reduce analysis count to focus on best candidates
            return min(max_wallets, wallet_count, 50)  # Cap at 50 for growth focus

        return min(max_wallets, wallet_count)

    def optimize_discovery_depth(self, current_depth_hours: int) -> int:
        """
        Optimize discovery depth for budget efficiency.

        Args:
            current_depth_hours: Current discovery lookback hours

        Returns:
            Optimized discovery depth
        """
        budget_ratio = (self.DAILY_CREDIT_TARGET - self._credits_used_today) / self.DAILY_CREDIT_TARGET

        # Reduce discovery depth when budget is tight
        if budget_ratio > 0.5:
            return current_depth_hours
        elif budget_ratio > 0.3:
            return max(current_depth_hours // 2, 24)  # At least 24 hours
        else:
            return max(current_depth_hours // 4, 4)  # At least 4 hours

    def get_growth_allocation(self, wallet_predictions: List[Tuple[str, float]]) -> Dict[str, float]:
        """
        Calculate growth-optimized capital allocation.

        Args:
            wallet_predictions: List of (wallet_address, expected_return) tuples

        Returns:
            Dictionary mapping wallet_address to allocation amount
        """
        if not wallet_predictions:
            return {}

        # Filter for profitable wallets
        profitable = [(addr, ret) for addr, ret in wallet_predictions if ret > 0]

        if not profitable:
            return {}

        # Calculate growth-optimized allocation
        # Focus on high-conviction wallets for growth goal
        sorted_wallets = sorted(profitable, key=lambda x: x[1], reverse=True)

        # Allocate more capital to top performers
        allocation = {}
        total_weight = 0

        for i, (addr, expected_return) in enumerate(sorted_wallets[:20]):  # Top 20
            # Weight by expected return
            weight = expected_return ** 2  # Square to emphasize top performers
            total_weight += weight
            allocation[addr] = weight

        # Normalize and apply current capital
        if total_weight > 0:
            for addr in allocation:
                allocation[addr] = (allocation[addr] / total_weight) * self._current_capital

        logger.info(f"Growth allocation: ${self._current_capital:.0f} across {len(allocation)} wallets")
        return allocation

    def get_optimization_suggestions(self) -> List[str]:
        """Get optimization suggestions based on current state."""
        suggestions = []

        # Budget status
        budget_ratio = (self.DAILY_CREDIT_TARGET - self._credits_used_today) / self.DAILY_CREDIT_TARGET

        if budget_ratio < 0.2:
            suggestions.append("CRITICAL: Budget below 20% - enable conservative mode")
            suggestions.append("Reduce wallet discovery scope immediately")
            suggestions.append("Skip all non-critical enrichment")
        elif budget_ratio < 0.4:
            suggestions.append("Budget below 40% - prioritize high-conviction wallets")
            suggestions.append("Reduce analysis depth for emerging wallets")

        # Growth optimization
        if self._growth_optimized:
            suggestions.append("Growth mode: Focus on top 20 high-conviction wallets")
            suggestions.append(f"Target: ${self._current_capital:.0f} → ${self._target_capital:.0f}")

        # Performance suggestions
        if self._stats['requests_failed'] > 0:
            fail_rate = self._stats['requests_failed'] / max(1, self._stats['requests_processed'])
            if fail_rate > 0.1:
                suggestions.append(f"High failure rate ({fail_rate*100:.1f}%) - check RPC health")

        return suggestions

    def print_status_report(self):
        """Print comprehensive status report."""
        print("\n" + "="*70)
        print("HELIUS DEVELOPER PLAN OPTIMIZER - STATUS")
        print("="*70)

        # Budget status
        budget_used = self._credits_used_today
        budget_remaining = self.DAILY_CREDIT_TARGET - budget_used
        budget_ratio = budget_remaining / self.DAILY_CREDIT_TARGET

        print("\nCredit Budget:")
        print(f"  Used today: {budget_used:,.0f} / {self.DAILY_CREDIT_TARGET:,.0f}")
        print(f"  Remaining: {budget_remaining:,.0f} ({budget_ratio*100:.1f}%)")
        print(f"  Monthly target: {self.MAX_CREDITS_PER_MONTH:,.0f}")

        # Queue status
        print("\nRequest Queue:")
        print(f"  Pending requests: {len(self._request_queue)}")

        # Performance stats
        print("\nPerformance:")
        print(f"  Requests processed: {self._stats['requests_processed']}")
        print(f"  Requests failed: {self._stats['requests_failed']}")
        print(f"  Batches processed: {self._stats['batches_processed']}")

        # Growth status
        if self._growth_optimized:
            growth_progress = (self._current_capital / self._target_capital) * 100
            print("\nGrowth Goal:")
            print(f"  Current: ${self._current_capital:.0f}")
            print(f"  Target: ${self._target_capital:.0f}")
            print(f"  Progress: {growth_progress:.1f}%")

        # Optimization suggestions
        suggestions = self.get_optimization_suggestions()
        if suggestions:
            print("\nOptimization Suggestions:")
            for i, suggestion in enumerate(suggestions, 1):
                print(f"  {i}. {suggestion}")

        print("="*70 + "\n")

    async def shutdown(self):
        """Cleanup and shutdown."""
        # Process remaining queue
        if self._request_queue:
            logger.info(f"Processing {len(self._request_queue)} remaining requests")
            await self.process_queue()

        logger.info("Helius Optimizer shut down")


# Global singleton instance
_optimizer: Optional[HeliusOptimizer] = None


def get_helius_optimizer() -> HeliusOptimizer:
    """Get the global Helius optimizer singleton."""
    global _optimizer

    if _optimizer is None:
        _optimizer = HeliusOptimizer()

    return _optimizer


if __name__ == "__main__":
    # Test the optimizer
    optimizer = get_helius_optimizer()
    optimizer.print_status_report()

    # Test optimization functions
    print("\nTesting optimization functions:")
    print(f"  Optimize wallet analysis (100 wallets): {optimizer.optimize_wallet_analysis(100)}")
    print(f"  Optimize discovery depth (168 hours): {optimizer.optimize_discovery_depth(168)}")

    # Test growth allocation
    test_predictions = [
        ("wallet1", 0.25),
        ("wallet2", 0.15),
        ("wallet3", 0.08),
    ]
    allocation = optimizer.get_growth_allocation(test_predictions)
    print(f"  Growth allocation: {allocation}")