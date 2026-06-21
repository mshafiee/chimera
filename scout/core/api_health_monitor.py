"""
API Health Monitoring and Fallback System

Provides:
- API health monitoring with automatic failover
- Graceful degradation using cached data
- Offline mode support
- Request queuing and rate limit awareness

Usage:
    monitor = APIHealthMonitor()
    if monitor.is_healthy("helius"):
        response = await make_api_call(...)
    else:
        # Use fallback or cached data
"""

import asyncio
import logging
import os
import time
from collections import deque
from dataclasses import dataclass
from datetime import datetime
from typing import Dict, Optional, Callable, Any
from enum import Enum

logger = logging.getLogger(__name__)


class APIStatus(Enum):
    """API health status."""
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    DOWN = "down"
    UNKNOWN = "unknown"


@dataclass
class APIMetrics:
    """Metrics for an API endpoint."""
    total_requests: int = 0
    successful_requests: int = 0
    failed_requests: int = 0
    total_latency_ms: float = 0.0
    last_success_time: Optional[datetime] = None
    last_failure_time: Optional[datetime] = None
    consecutive_failures: int = 0
    last_error: Optional[str] = None

    @property
    def success_rate(self) -> float:
        """Calculate success rate (0.0 to 1.0)."""
        if self.total_requests == 0:
            return 1.0
        return self.successful_requests / self.total_requests

    @property
    def average_latency_ms(self) -> float:
        """Calculate average latency in milliseconds."""
        if self.successful_requests == 0:
            return 0.0
        return self.total_latency_ms / self.successful_requests


@dataclass
class APIEndpoint:
    """Configuration for an API endpoint."""
    name: str
    base_url: str
    priority: int = 1  # Lower = higher priority
    rate_limit_rps: float = 10.0  # Requests per second
    timeout_seconds: float = 30.0
    enabled: bool = True


class APIHealthMonitor:
    """
    Monitors API health and manages failover between endpoints.

    Tracks metrics for each API endpoint and automatically fails over
    when an endpoint becomes unhealthy.
    """

    def __init__(self):
        """Initialize the health monitor."""
        self.endpoints: Dict[str, APIEndpoint] = {}
        self.metrics: Dict[str, APIMetrics] = {}
        self.current_primary: Optional[str] = None
        self.offline_mode = False
        self.offline_since: Optional[datetime] = None

        # Health check thresholds
        self.health_check_interval = int(os.getenv("SCOUT_HEALTH_CHECK_INTERVAL", "60"))
        self.unhealthy_threshold = float(os.getenv("SCOUT_UNHEALTHY_THRESHOLD", "0.5"))  # 50% success rate
        self.degraded_threshold = float(os.getenv("SCOUT_DEGRADED_THRESHOLD", "0.8"))  # 80% success rate
        self.max_consecutive_failures = int(os.getenv("SCOUT_MAX_CONSECUTIVE_FAILURES", "5"))

        # Rate limiting
        self.request_queue: deque = deque()
        self.last_request_time: Dict[str, float] = {}
        self.min_request_interval_ms = int(os.getenv("SCOUT_MIN_REQUEST_INTERVAL_MS", "100"))

        # Cache for graceful degradation
        self.cache: Dict[str, tuple[Any, datetime]] = {}
        self.cache_ttl_seconds = int(os.getenv("SCOUT_CACHE_TTL_SECONDS", "300"))

        # Background health check task
        self._health_check_task: Optional[asyncio.Task] = None

    def add_endpoint(self, endpoint: APIEndpoint) -> None:
        """
        Register an API endpoint for monitoring.

        Args:
            endpoint: APIEndpoint configuration
        """
        self.endpoints[endpoint.name] = endpoint
        self.metrics[endpoint.name] = APIMetrics()

        # Set as primary if it's the first or highest priority
        if self.current_primary is None or endpoint.priority < self.endpoints[self.current_primary].priority:
            self.current_primary = endpoint.name

        logger.info(f"Registered API endpoint: {endpoint.name} (priority={endpoint.priority})")

    def is_healthy(self, endpoint_name: Optional[str] = None) -> bool:
        """
        Check if an API endpoint is healthy.

        Args:
            endpoint_name: Name of endpoint to check (None for current primary)

        Returns:
            True if endpoint is healthy
        """
        if endpoint_name is None:
            endpoint_name = self.current_primary

        if endpoint_name is None or endpoint_name not in self.metrics:
            return False

        metrics = self.metrics[endpoint_name]

        # Check consecutive failures
        if metrics.consecutive_failures >= self.max_consecutive_failures:
            return False

        # Check success rate
        if metrics.total_requests >= 10:  # Only check after sufficient samples
            if metrics.success_rate < self.unhealthy_threshold:
                return False

        return True

    def get_status(self, endpoint_name: Optional[str] = None) -> APIStatus:
        """
        Get the status of an API endpoint.

        Args:
            endpoint_name: Name of endpoint to check

        Returns:
            APIStatus enum value
        """
        if endpoint_name is None:
            endpoint_name = self.current_primary

        if endpoint_name is None or endpoint_name not in self.metrics:
            return APIStatus.UNKNOWN

        metrics = self.metrics[endpoint_name]

        if metrics.consecutive_failures >= self.max_consecutive_failures:
            return APIStatus.DOWN

        if metrics.total_requests >= 10:
            if metrics.success_rate < self.unhealthy_threshold:
                return APIStatus.DOWN
            elif metrics.success_rate < self.degraded_threshold:
                return APIStatus.DEGRADED

        return APIStatus.HEALTHY

    def record_request(
        self,
        endpoint_name: str,
        success: bool,
        latency_ms: Optional[float] = None,
        error: Optional[str] = None
    ) -> None:
        """
        Record a request attempt to an API endpoint.

        Args:
            endpoint_name: Name of the endpoint
            success: Whether the request succeeded
            latency_ms: Request latency in milliseconds
            error: Error message if failed
        """
        if endpoint_name not in self.metrics:
            return

        metrics = self.metrics[endpoint_name]
        metrics.total_requests += 1

        if success:
            metrics.successful_requests += 1
            metrics.consecutive_failures = 0
            metrics.last_success_time = datetime.utcnow()
            if latency_ms is not None:
                metrics.total_latency_ms += latency_ms
        else:
            metrics.failed_requests += 1
            metrics.consecutive_failures += 1
            metrics.last_failure_time = datetime.utcnow()
            metrics.last_error = error

        # Trigger failover if needed
        if not self.is_healthy(endpoint_name) and endpoint_name == self.current_primary:
            logger.warning(f"Primary endpoint {endpoint_name} is unhealthy, triggering failover")
            self._trigger_failover()

    def get_best_endpoint(self) -> Optional[str]:
        """
        Get the best available endpoint based on health and priority.

        Returns:
            Name of the best endpoint, or None if no healthy endpoints
        """
        if self.offline_mode:
            return None

        # Filter to enabled endpoints
        enabled_endpoints = [
            (name, ep) for name, ep in self.endpoints.items()
            if ep.enabled
        ]

        if not enabled_endpoints:
            return None

        # Sort by priority (lower = better) and health
        def sort_key(item):
            name, endpoint = item
            status = self.get_status(name)
            priority = endpoint.priority

            # Prioritize healthy endpoints
            if status == APIStatus.HEALTHY:
                health_rank = 0
            elif status == APIStatus.DEGRADED:
                health_rank = 1
            else:
                health_rank = 2

            return (health_rank, priority)

        enabled_endpoints.sort(key=sort_key)
        best_name, _ = enabled_endpoints[0]

        # Update primary if changed
        if best_name != self.current_primary:
            logger.info(f"Switching primary endpoint: {self.current_primary} -> {best_name}")
            self.current_primary = best_name

        return best_name

    def _trigger_failover(self) -> None:
        """Trigger failover to a backup endpoint."""
        best_endpoint = self.get_best_endpoint()

        if best_endpoint and best_endpoint != self.current_primary:
            logger.warning(f"Failover triggered: {self.current_primary} -> {best_endpoint}")
            self.current_primary = best_endpoint
        else:
            logger.error("No healthy endpoints available, entering offline mode")
            self.enter_offline_mode()

    def enter_offline_mode(self) -> bool:
        """
        Enter offline mode (no API calls, use cached data only).

        Returns:
            True if successfully entered offline mode
        """
        if self.offline_mode:
            return False

        logger.warning("Entering offline mode - using cached data only")
        self.offline_mode = True
        self.offline_since = datetime.utcnow()
        return True

    def exit_offline_mode(self) -> bool:
        """
        Exit offline mode and resume API calls.

        Returns:
            True if successfully exited offline mode
        """
        if not self.offline_mode:
            return False

        # Check if any endpoint is healthy
        for name in self.endpoints.keys():
            if self.is_healthy(name):
                logger.info(f"Exiting offline mode - endpoint {name} is healthy")
                self.offline_mode = False
                self.offline_since = None
                return True

        logger.warning("Cannot exit offline mode - no healthy endpoints")
        return False

    async def execute_with_fallback(
        self,
        request_func: Callable,
        cache_key: Optional[str] = None,
        use_cache: bool = True
    ) -> Any:
        """
        Execute an API request with automatic failover and fallback.

        Args:
            request_func: Async function that makes the API request
            cache_key: Key for caching results
            use_cache: Whether to use cached data if available

        Returns:
            API response or cached data
        """
        # Check cache first
        if use_cache and cache_key and cache_key in self.cache:
            cached_data, cache_time = self.cache[cache_key]
            age_seconds = (datetime.utcnow() - cache_time).total_seconds()

            if age_seconds < self.cache_ttl_seconds:
                logger.debug(f"Using cached data for {cache_key} (age: {age_seconds:.0f}s)")
                return cached_data

        # If in offline mode, try to use cache
        if self.offline_mode:
            if cache_key and cache_key in self.cache:
                logger.warning(f"Offline mode: Using cached data for {cache_key}")
                return self.cache[cache_key][0]
            else:
                logger.error(f"Offline mode: No cached data for {cache_key}")
                return None

        # Get best endpoint and try request
        endpoint_name = self.get_best_endpoint()

        if endpoint_name is None:
            logger.error("No endpoints available")
            return None

        # Try request with failover
        max_attempts = 3
        for attempt in range(max_attempts):
            start_time = time.time()

            try:
                # Rate limiting
                await self._rate_limit(endpoint_name)

                result = await request_func(endpoint_name)

                # Record success
                latency_ms = (time.time() - start_time) * 1000
                self.record_request(endpoint_name, True, latency_ms)

                # Cache result
                if cache_key and result is not None:
                    self.cache[cache_key] = (result, datetime.utcnow())

                return result

            except Exception as e:
                latency_ms = (time.time() - start_time) * 1000
                self.record_request(endpoint_name, False, latency_ms, str(e))

                if attempt < max_attempts - 1:
                    # Try next endpoint
                    endpoint_name = self.get_best_endpoint()
                    if endpoint_name != self.current_primary:
                        logger.info(f"Retrying with endpoint {endpoint_name}")
                        continue

        # All attempts failed
        logger.error(f"All API attempts failed for {cache_key}")
        return None

    async def _rate_limit(self, endpoint_name: str) -> None:
        """Apply rate limiting for an endpoint."""
        if endpoint_name not in self.endpoints:
            return

        endpoint = self.endpoints[endpoint_name]
        now = time.time()
        last_time = self.last_request_time.get(endpoint_name, 0)

        min_interval = 1000.0 / endpoint.rate_limit_rps  # Convert RPS to ms
        time_since_last = (now - last_time) * 1000  # Convert to ms

        if time_since_last < min_interval:
            await asyncio.sleep((min_interval - time_since_last) / 1000)

        self.last_request_time[endpoint_name] = time.time()

    def get_metrics_summary(self) -> Dict[str, Any]:
        """
        Get a summary of all API metrics.

        Returns:
            Dictionary with metrics for all endpoints
        """
        summary = {}
        for name, metrics in self.metrics.items():
            endpoint = self.endpoints.get(name)
            summary[name] = {
                'status': self.get_status(name).value,
                'success_rate': f"{metrics.success_rate:.2%}",
                'total_requests': metrics.total_requests,
                'failed_requests': metrics.failed_requests,
                'consecutive_failures': metrics.consecutive_failures,
                'average_latency_ms': f"{metrics.average_latency_ms:.1f}",
                'last_success': metrics.last_success_time.isoformat() if metrics.last_success_time else None,
                'last_failure': metrics.last_failure_time.isoformat() if metrics.last_failure_time else None,
                'last_error': metrics.last_error,
                'priority': endpoint.priority if endpoint else None,
            }

        summary['current_primary'] = self.current_primary
        summary['offline_mode'] = self.offline_mode
        summary['offline_since'] = self.offline_since.isoformat() if self.offline_since else None

        return summary


# Global singleton instance
_global_monitor: Optional[APIHealthMonitor] = None


def get_api_monitor() -> APIHealthMonitor:
    """Get the global API health monitor instance."""
    global _global_monitor
    if _global_monitor is None:
        _global_monitor = APIHealthMonitor()

        # Register default Helius endpoint
        helius_url = os.getenv("CHIMERA_RPC__PRIMARY_URL", "")
        if helius_url:
            _global_monitor.add_endpoint(APIEndpoint(
                name="helius",
                base_url=helius_url,
                priority=1,
                rate_limit_rps=40.0,  # Helius Developer Plan limit
                timeout_seconds=30.0,
            ))

        # Register fallback endpoints if configured
        fallback_url = os.getenv("CHIMERA_RPC__FALLBACK_URL", "")
        if fallback_url:
            _global_monitor.add_endpoint(APIEndpoint(
                name="fallback",
                base_url=fallback_url,
                priority=2,
                rate_limit_rps=20.0,
                timeout_seconds=30.0,
            ))

    return _global_monitor
