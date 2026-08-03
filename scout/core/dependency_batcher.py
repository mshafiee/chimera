"""
Dependency-Aware Request Batching for Optimized API Usage

This module implements smart request batching that respects dependencies between
operations to maximize parallelization while maintaining correctness under Helius
Developer Plan constraints.

Dependency Types:
- NONE: Can run immediately (no dependencies)
- WALLET_EXISTS: Requires wallet discovery first
- TOKEN_METADATA: Requires token metadata fetch
- PRICE_DATA: Requires price data fetch
- WQS_CALCULATED: Requires WQS calculation

Features:
- Dependency resolution and topological sorting
- Parallel execution of independent chains
- Credit-aware batch sizing
- Execution plan optimization
"""

import time
import logging
from typing import Dict, List, Optional, Any, Set, Callable
from dataclasses import dataclass, field
from enum import Enum
import threading
from collections import deque

logger = logging.getLogger(__name__)


class DependencyType(Enum):
    """Types of dependencies between requests."""
    NONE = "none"                    # No dependencies
    WALLET_EXISTS = "wallet_exists"  # Requires wallet discovery
    TOKEN_METADATA = "token_meta"    # Requires token metadata
    PRICE_DATA = "price_data"         # Requires price data
    WQS_CALCULATED = "wqs_ready"     # Requires WQS calculation
    POSITION_TRACKED = "position_tracked"  # Requires position tracking


class RequestPriority(Enum):
    """Request priority levels."""
    CRITICAL = 1    # Must complete (validation, exits)
    HIGH = 2        # Important (discovery, candidate analysis)
    MEDIUM = 3      # Valuable but skippable (enrichment)
    LOW = 4         # Nice to have (archival data)


@dataclass
class APIRequest:
    """API request with metadata."""
    request_id: str
    category: str
    dependency_type: DependencyType
    priority: RequestPriority
    credit_cost: int
    depends_on: List[str] = field(default_factory=list)  # IDs of dependent requests
    target: Optional[str] = None  # Target wallet/token address
    callback: Optional[Callable] = None
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass
class RequestBatch:
    """Batch of requests that can be executed together."""
    batch_id: str
    requests: List[APIRequest]
    total_credits: int
    can_parallelize: bool
    dependencies_resolved: bool
    estimated_time_ms: float


@dataclass
class ExecutionPlan:
    """Execution plan for request batches."""
    batches: List[RequestBatch]
    total_credits: int
    estimated_time_ms: float
    parallel_potential: float  # 0-1, higher = more parallelization


@dataclass
class BatcherConfig:
    """Configuration for dependency-aware batching."""
    MAX_BATCH_SIZE: int = 50  # Max requests per batch
    MAX_BATCH_CREDITS: int = 10000  # Max credits per batch


class DependencyAwareBatcher:
    """
    Dependency-aware request batching for optimized API usage.

    Strategy:
    - Group independent requests for parallel execution
    - Chain dependent requests sequentially
    - Optimize batch order for minimum total time
    - Respect credit limits

    Features:
    - Topological sorting for dependencies
    - Parallel execution planning
    - Credit-aware batching
    - Performance tracking
    """

    def __init__(self, config: Optional[BatcherConfig] = None):
        """Initialize the dependency-aware batcher."""
        self._config = config or BatcherConfig()
        self._lock = threading.Lock()

        # Request queue
        self._request_queue: deque = deque()

        # Completed requests
        self._completed: Set[str] = set()

        # In-flight requests
        self._in_flight: Set[str] = set()

        # Statistics
        self._stats = {
            'requests_processed': 0,
            'batches_created': 0,
            'parallel_batches': 0,
        }

        logger.info("DependencyAwareBatcher initialized")

    def add_request(self, request: APIRequest) -> None:
        """
        Add a request to the batcher.

        Args:
            request: Request to add
        """
        with self._lock:
            self._request_queue.append(request)

            logger.debug(
                "Added request %s (%s) with %d dependencies",
                request.request_id, request.category, len(request.depends_on),
            )

    def batch_requests(self, requests: List[APIRequest]) -> List[RequestBatch]:
        """
        Batch requests respecting dependencies.

        Args:
            requests: List of requests to batch

        Returns:
            List of batches in execution order
        """
        with self._lock:
            # Build execution plan
            plan = self._create_execution_plan(requests)

            self._stats['batches_created'] += len(plan.batches)

            return plan.batches

    def _create_execution_plan(self, requests: List[APIRequest]) -> ExecutionPlan:
        """Create execution plan from requests."""
        if not requests:
            return ExecutionPlan(
                batches=[],
                total_credits=0,
                estimated_time_ms=0,
                parallel_potential=0,
            )

        # Separate by dependency status
        independent = [r for r in requests if not r.depends_on or
                      all(d in self._completed for d in r.depends_on)]
        dependent = [r for r in requests if r not in independent]

        batches = []
        total_credits = 0
        total_time = 0

        # Batch independent requests together (respecting size/credit limits)
        if independent:
            independent_sorted = sorted(independent, key=lambda r: r.priority.value)
            ind_batches, ind_credits, ind_time = self._chunk_batches(independent_sorted, can_parallelize=True)
            batches.extend(ind_batches)
            total_credits += ind_credits
            total_time += ind_time

        # Chain dependent requests in dependency waves (true topological
        # ordering): a request becomes ready when its dependencies are in
        # `self._completed` OR were scheduled in an earlier wave of this plan.
        if dependent:
            request_map = {r.request_id: r for r in requests}
            ready: Set[str] = set(self._completed)
            scheduled: Set[str] = set()
            remaining = list(dependent)

            while remaining:
                wave = [
                    r for r in remaining
                    if all(d in ready for d in r.depends_on)
                ]
                if not wave:
                    # No progress: dependencies that will never be satisfied.
                    # Distinguish true cycles from missing request IDs.
                    for r in remaining:
                        missing = [d for d in r.depends_on if d not in ready]
                        if any(d in request_map for d in missing):
                            logger.warning(f"Dependency cycle involving request {r.request_id}: {missing}")
                        else:
                            logger.warning(f"Dependency {missing} not found for request {r.request_id}")
                    break

                wave_sorted = sorted(wave, key=lambda r: r.priority.value)
                wave_can_parallelize = all(
                    all(d in self._completed for d in r.depends_on) for r in wave
                )
                wave_batches, wave_credits, wave_time = self._chunk_batches(
                    wave_sorted, can_parallelize=wave_can_parallelize
                )
                batches.extend(wave_batches)
                total_credits += wave_credits
                total_time += wave_time

                for req in wave:
                    ready.add(req.request_id)
                    scheduled.add(req.request_id)
                remaining = [r for r in remaining if r.request_id not in scheduled]

        # Calculate parallel potential
        independent_ratio = len(independent) / max(1, len(requests))
        parallel_potential = independent_ratio

        return ExecutionPlan(
            batches=batches,
            total_credits=total_credits,
            estimated_time_ms=total_time,
            parallel_potential=parallel_potential,
        )

    def _chunk_batches(
        self, requests: List[APIRequest], can_parallelize: bool
    ) -> tuple:
        """Split requests into batches respecting MAX_BATCH_SIZE/MAX_BATCH_CREDITS.

        Returns (batches, total_credits, total_time).
        """
        batches = []
        total_credits = 0
        total_time = 0
        current: List[APIRequest] = []
        current_credits = 0

        def _flush() -> None:
            nonlocal current, current_credits, total_credits, total_time
            if not current:
                return
            batch = self._create_batch(current, can_parallelize=can_parallelize)
            batches.append(batch)
            total_credits += batch.total_credits
            total_time += batch.estimated_time_ms
            if can_parallelize:
                self._stats['parallel_batches'] += 1
            current = []
            current_credits = 0

        for req in requests:
            current.append(req)
            current_credits += req.credit_cost
            if (len(current) >= self._config.MAX_BATCH_SIZE or
                    current_credits >= self._config.MAX_BATCH_CREDITS):
                _flush()
        _flush()

        return batches, total_credits, total_time

    def _create_batch(self, requests: List[APIRequest], can_parallelize: bool) -> RequestBatch:
        """Create a batch from requests."""
        batch_id = f"batch_{int(time.time() * 1000)}_{len(requests)}"
        total_credits = sum(r.credit_cost for r in requests)

        # Estimate time (rough heuristic: 10ms per request + 50ms overhead)
        estimated_time = len(requests) * 10 + 50

        return RequestBatch(
            batch_id=batch_id,
            requests=requests,
            total_credits=total_credits,
            can_parallelize=can_parallelize,
            dependencies_resolved=all(
                all(d in self._completed for d in r.depends_on) for r in requests
            ),
            estimated_time_ms=estimated_time,
        )

    def resolve_dependencies(self, batch: RequestBatch) -> ExecutionPlan:
        """
        Resolve dependencies for a batch.

        Args:
            batch: Batch to resolve dependencies for

        Returns:
            Execution plan with resolved dependencies
        """
        with self._lock:
            # Check if all dependencies are satisfied
            unsatisfied = []
            for req in batch.requests:
                for dep_id in req.depends_on:
                    if dep_id not in self._completed:
                        unsatisfied.append(dep_id)

            if unsatisfied:
                logger.warning(f"Batch {batch.batch_id} has unsatisfied dependencies: {unsatisfied}")

                # Create plan with unsatisfied dependencies noted
                return ExecutionPlan(
                    batches=[batch],
                    total_credits=batch.total_credits,
                    estimated_time_ms=batch.estimated_time_ms,
                    parallel_potential=0.0,
                )

            batch.dependencies_resolved = True
            return ExecutionPlan(
                batches=[batch],
                total_credits=batch.total_credits,
                estimated_time_ms=batch.estimated_time_ms,
                parallel_potential=1.0 if batch.can_parallelize else 0.0,
            )

    def optimize_batch_order(self, batches: List[RequestBatch]) -> List[RequestBatch]:
        """
        Optimize batch order for minimum total execution time.

        Args:
            batches: List of batches to optimize

        Returns:
            Optimized batch order
        """
        with self._lock:
            if not batches:
                return []

            # Separate parallelizable and sequential batches
            parallel = [b for b in batches if b.can_parallelize and b.dependencies_resolved]
            sequential = [b for b in batches if not b.can_parallelize or not b.dependencies_resolved]

            # Sort sequential by priority (higher priority first)
            sequential_sorted = sorted(
                sequential,
                key=lambda b: min(r.priority.value for r in b.requests) if b.requests else 999
            )

            # Parallel batches can go first
            optimized = parallel + sequential_sorted

            return optimized

    def calculate_parallel_potential(self, requests: List[APIRequest]) -> float:
        """
        Calculate how many requests can be executed in parallel.

        Args:
            requests: List of requests to analyze

        Returns:
            Parallel potential (0-1)
        """
        with self._lock:
            if not requests:
                return 0.0

            # Count independent requests
            independent = sum(
                1 for r in requests
                if not r.depends_on or all(d in self._completed for d in r.depends_on)
            )

            return independent / len(requests)

    def mark_completed(self, request_id: str) -> None:
        """
        Mark a request as completed.

        Args:
            request_id: ID of completed request
        """
        with self._lock:
            self._completed.add(request_id)
            self._in_flight.discard(request_id)
            self._stats['requests_processed'] += 1

            logger.debug(f"Marked request {request_id} as completed")

    def mark_in_flight(self, request_id: str) -> None:
        """
        Mark a request as in-flight (executing).

        Args:
            request_id: ID of in-flight request
        """
        with self._lock:
            self._in_flight.add(request_id)

    def get_ready_requests(self) -> List[APIRequest]:
        """
        Get requests that are ready to execute (dependencies satisfied).

        Returns:
            List of ready requests
        """
        with self._lock:
            ready = []

            for req in list(self._request_queue):
                # Check if all dependencies are completed
                deps_satisfied = all(d in self._completed for d in req.depends_on)

                if deps_satisfied and req.request_id not in self._in_flight:
                    ready.append(req)

            # Identity-safe removal: rebuild the queue without ready requests
            # (deque.remove uses value equality, which could drop an earlier
            # duplicate of an identical request)
            ready_ids = {r.request_id for r in ready}
            self._request_queue = deque(
                r for r in self._request_queue if r.request_id not in ready_ids
            )

            return ready

    def get_batcher_stats(self) -> Dict[str, Any]:
        """Get batcher statistics."""
        with self._lock:
            return {
                'requests_processed': self._stats['requests_processed'],
                'batches_created': self._stats['batches_created'],
                'parallel_batches': self._stats['parallel_batches'],
                'queue_size': len(self._request_queue),
                'completed': len(self._completed),
                'in_flight': len(self._in_flight),
            }

    def reset_statistics(self) -> None:
        """Reset batcher statistics."""
        with self._lock:
            self._stats = {
                'requests_processed': 0,
                'batches_created': 0,
                'parallel_batches': 0,
            }
            logger.info("Statistics reset")

    def clear_completed(self) -> None:
        """Clear completed requests tracking."""
        with self._lock:
            self._completed.clear()
            logger.info("Cleared completed requests")
