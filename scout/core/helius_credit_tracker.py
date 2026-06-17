"""
Helius Credit Tracking and Optimization System

This module implements intelligent API credit tracking and optimization for Helius Developer Plan constraints:
- 10M credits per month
- 50 requests per second rate limit
- 5 sendTransaction per second
- Credit cost optimization for maximum profitability

Key Features:
- Real-time credit tracking and forecasting
- Request prioritization and optimization
- Cost-benefit analysis for API calls
- Budget allocation for different analysis types
- Developer Plan constraint enforcement
"""

import os
import time
import json
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, asdict
from enum import Enum
import threading

logger = logging.getLogger(__name__)


class CreditCost(Enum):
    """Helius API credit costs for different endpoints."""

    # Transaction fetching
    GET_TRANSACTIONS = 1  # Per page of transactions
    GET_TRANSACTION = 2   # Single transaction details
    PARSE_TRANSACTION = 1 # Transaction parsing

    # Wallet discovery
    DISCOVER_WALLETS = 5  # Per discovery batch
    WALLET_BALANCES = 10  # Bulk balance check

    # Token metadata
    TOKEN_METADATA = 2    # Token metadata fetch
    TOKEN_CREATION = 3     # Token creation time

    # Signature queries
    SIGNATURES = 1         # Per signature page
    WALLET_FIRST_TX = 2    # Wallet creation time

    # Analysis operations
    SWAP_ANALYSIS = 1      # Per swap parsed
    POSITION_TRACK = 2     # Position reconciliation


class RequestPriority(Enum):
    """Priority levels for API requests."""

    CRITICAL = 1    # Must complete (backtest validation, final promotion)
    HIGH = 2        # Important (wallet discovery, candidate analysis)
    MEDIUM = 3      # Valuable but skippable (enrichment, metadata)
    LOW = 4         # Nice to have (archival data, extra metrics)


@dataclass
class CreditBudget:
    """Monthly credit budget allocation."""

    # Budget allocation (as percentages of total)
    DISCOVERY_RATIO = 0.30      # 30% for wallet discovery
    ANALYSIS_RATIO = 0.40       # 40% for wallet analysis
    VALIDATION_RATIO = 0.20     # 20% for backtest validation
    RESERVE_RATIO = 0.10        # 10% reserve for critical operations

    # Developer Plan constraints
    MONTHLY_CREDITS = 10_000_000
    MAX_REQUESTS_PER_SECOND = 50
    DAILY_TARGET = MONTHLY_CREDITS / 30  # ~333K credits per day

    # Growth goal optimization ($200 → $1000)
    # Allocate more budget to high-conviction wallets
    HIGH_CONVICTIO_BONUS = 1.5  # 1.5x credits for WQS > 70
    EMERGING_WALLET_LIMIT = 0.3  # Max 30% of budget for WQS < 50


@dataclass
class RequestCost:
    """Cost analysis for an API request."""

    endpoint: str
    credit_cost: int
    priority: RequestPriority
    expected_value: float  # 0.0-1.0 expected benefit
    wallet_address: Optional[str] = None
    timestamp: float = 0.0
    completed: bool = False
    success: bool = False

    def __post_init__(self):
        if self.timestamp == 0.0:
            self.timestamp = time.time()


@dataclass
class CreditSnapshot:
    """Snapshot of credit usage at a point in time."""

    timestamp: float
    credits_used: int
    credits_remaining: int
    daily_usage: int
    requests_made: int
    requests_per_second: float
    projected_monthly: int
    budget_status: str  # "healthy", "warning", "critical"


class HeliusCreditTracker:
    """
    Intelligent Helius API credit tracking and optimization system.

    Features:
    - Real-time credit tracking with forecasting
    - Request prioritization and queuing
    - Cost-benefit optimization
    - Budget enforcement for Developer Plan
    - Growth goal optimization
    """

    def __init__(self):
        """Initialize the credit tracker."""

        # Credit tracking
        self._credits_used_today = 0
        self._credits_used_month = 0
        self._requests_today = 0
        self._day_start_time = time.time()
        self._month_start_time = time.time()

        # Request queue (priority queue)
        self._request_queue: List[RequestCost] = []
        self._queue_lock = threading.Lock()

        # Performance tracking
        self._request_history: List[RequestCost] = []
        self._max_history_size = 1000

        # Budget tracking
        self._budget = CreditBudget()
        self._daily_budget = self._budget.DAILY_TARGET
        self._discovery_budget = self._daily_budget * self._budget.DISCOVERY_RATIO
        self._analysis_budget = self._daily_budget * self._budget.ANALYSIS_RATIO
        self._validation_budget = self._daily_budget * self._budget.VALIDATION_RATIO
        self._reserve_budget = self._daily_budget * self._budget.RESERVE_RATIO

        # Category-specific tracking
        self._discovery_spent = 0
        self._analysis_spent = 0
        self._validation_spent = 0
        self._reserve_spent = 0

        # Rate limiting
        self._request_times: List[float] = []
        self._rate_limit_window = 1.0  # 1 second window

        # Configuration
        self._enable_optimization = os.getenv("SCOUT_CREDIT_OPTIMIZATION", "true").lower() == "true"
        self._conservative_mode = os.getenv("SCOUT_CONSERVATIVE_MODE", "false").lower() == "true"
        self._growth_optimized = os.getenv("SCOUT_GROWTH_OPTIMIZED", "true").lower() == "true"

        # Load previous state if available
        self._load_state()

        logger.info(f"Helius Credit Tracker initialized")
        logger.info(f"  Daily budget: {self._daily_budget:,.0f} credits")
        logger.info(f"  Discovery budget: {self._discovery_budget:,.0f} credits")
        logger.info(f"  Analysis budget: {self._analysis_budget:,.0f} credits")
        logger.info(f"  Validation budget: {self._validation_budget:,.0f} credits")
        logger.info(f"  Growth optimized: {self._growth_optimized}")

    def _load_state(self):
        """Load previous credit usage state from disk."""
        try:
            state_file = os.getenv("SCOUT_CREDIT_STATE_FILE",
                                   "/tmp/helius_credit_state.json")
            if os.path.exists(state_file):
                with open(state_file, 'r') as f:
                    state = json.load(f)

                # Check if state is from today
                state_date = datetime.fromtimestamp(state.get('timestamp', 0))
                today = datetime.now()

                if state_date.date() == today.date():
                    self._credits_used_today = state.get('credits_used_today', 0)
                    self._credits_used_month = state.get('credits_used_month', 0)
                    self._requests_today = state.get('requests_today', 0)
                    self._discovery_spent = state.get('discovery_spent', 0)
                    self._analysis_spent = state.get('analysis_spent', 0)
                    self._validation_spent = state.get('validation_spent', 0)

                    logger.info(f"Loaded credit state: {self._credits_used_today:,.0f} credits used today")
                else:
                    # New day, reset daily counters
                    logger.info("New day detected, resetting daily counters")
                    self._save_state()
        except Exception as e:
            logger.warning(f"Failed to load credit state: {e}")

    def _save_state(self):
        """Save current credit usage state to disk."""
        try:
            state_file = os.getenv("SCOUT_CREDIT_STATE_FILE",
                                   "/tmp/helius_credit_state.json")
            state = {
                'timestamp': time.time(),
                'credits_used_today': self._credits_used_today,
                'credits_used_month': self._credits_used_month,
                'requests_today': self._requests_today,
                'discovery_spent': self._discovery_spent,
                'analysis_spent': self._analysis_spent,
                'validation_spent': self._validation_spent,
            }

            os.makedirs(os.path.dirname(state_file), exist_ok=True)
            with open(state_file, 'w') as f:
                json.dump(state, f, indent=2)
        except Exception as e:
            logger.warning(f"Failed to save credit state: {e}")

    def _check_daily_reset(self):
        """Check if we need to reset daily counters."""
        now = time.time()
        hours_since_day_start = (now - self._day_start_time) / 3600

        if hours_since_day_start >= 24:
            logger.info("24 hours elapsed, resetting daily counters")
            self._credits_used_today = 0
            self._requests_today = 0
            self._discovery_spent = 0
            self._analysis_spent = 0
            self._validation_spent = 0
            self._reserve_spent = 0
            self._day_start_time = now
            self._save_state()

    def _check_rate_limit(self) -> bool:
        """Check if we're within rate limits (50 req/s for Developer Plan)."""
        now = time.time()

        # Clean old request times
        self._request_times = [t for t in self._request_times
                              if now - t < self._rate_limit_window]

        current_rps = len(self._request_times) / self._rate_limit_window
        max_rps = self._budget.MAX_REQUESTS_PER_SECOND

        if current_rps >= max_rps:
            logger.debug(f"Rate limit reached: {current_rps:.1f} req/s")
            return False

        return True

    def _get_category_budget(self, category: str) -> Tuple[int, int]:
        """Get budget and spent for a category."""
        if category == "discovery":
            return self._discovery_budget, self._discovery_spent
        elif category == "analysis":
            return self._analysis_budget, self._analysis_spent
        elif category == "validation":
            return self._validation_budget, self._validation_spent
        elif category == "reserve":
            return self._reserve_budget, self._reserve_spent
        else:
            return self._daily_budget, self._credits_used_today

    def _check_budget(self, cost: int, category: str = "analysis") -> bool:
        """Check if we have budget for a request."""
        self._check_daily_reset()

        budget, spent = self._get_category_budget(category)
        remaining = budget - spent

        # In conservative mode, keep 20% buffer
        if self._conservative_mode:
            remaining *= 0.8

        return remaining >= cost

    def can_make_request(self, cost: int, category: str = "analysis",
                        priority: RequestPriority = RequestPriority.MEDIUM,
                        expected_value: float = 0.5) -> Tuple[bool, str]:
        """
        Check if we can make a request given current budget and constraints.

        Args:
            cost: Credit cost of the request
            category: Budget category (discovery, analysis, validation, reserve)
            priority: Request priority
            expected_value: Expected benefit (0.0-1.0)

        Returns:
            Tuple of (allowed, reason)
        """
        # Check rate limit first
        if not self._check_rate_limit():
            return False, "Rate limit reached (50 req/s)"

        # Check budget
        if not self._check_budget(cost, category):
            return False, f"Insufficient budget in {category} category"

        # Priority-based filtering when budget is tight
        budget, spent = self._get_category_budget(category)
        remaining_ratio = (budget - spent) / budget if budget > 0 else 0

        # If budget is tight (< 20% remaining), only allow high-priority requests
        if remaining_ratio < 0.2 and priority != RequestPriority.CRITICAL:
            if self._conservative_mode:
                return False, f"Budget tight ({remaining_ratio*100:.1f}% remaining), only critical requests allowed"

        # Value-based filtering for low-priority requests
        if priority == RequestPriority.LOW and expected_value < 0.3:
            if self._enable_optimization:
                return False, f"Low value request ({expected_value:.2f}) below threshold"

        return True, "OK"

    def record_request(self, cost: int, category: str = "analysis",
                      success: bool = True, endpoint: str = "unknown",
                      wallet_address: Optional[str] = None):
        """
        Record a completed API request.

        Args:
            cost: Credit cost of the request
            category: Budget category
            success: Whether the request succeeded
            endpoint: API endpoint name
            wallet_address: Associated wallet address (if any)
        """
        self._check_daily_reset()

        # Update counters
        self._credits_used_today += cost
        self._credits_used_month += cost
        self._requests_today += 1

        # Update category-specific counters
        if category == "discovery":
            self._discovery_spent += cost
        elif category == "analysis":
            self._analysis_spent += cost
        elif category == "validation":
            self._validation_spent += cost
        elif category == "reserve":
            self._reserve_spent += cost

        # Update rate limiting
        now = time.time()
        self._request_times.append(now)

        # Record in history
        request = RequestCost(
            endpoint=endpoint,
            credit_cost=cost,
            priority=RequestPriority.MEDIUM,
            expected_value=0.5,
            wallet_address=wallet_address,
            timestamp=now,
            completed=True,
            success=success
        )

        self._request_history.append(request)

        # Trim history if needed
        if len(self._request_history) > self._max_history_size:
            self._request_history = self._request_history[-self._max_history_size:]

        # Save state periodically
        if self._requests_today % 10 == 0:
            self._save_state()

    def get_snapshot(self) -> CreditSnapshot:
        """Get current snapshot of credit usage."""
        self._check_daily_reset()

        now = time.time()
        credits_remaining = self._daily_budget - self._credits_used_today

        # Calculate current requests per second
        recent_requests = [t for t in self._request_times
                          if now - t < self._rate_limit_window]
        current_rps = len(recent_requests) / self._rate_limit_window

        # Project monthly usage based on daily trend
        days_elapsed = (now - self._month_start_time) / 86400
        if days_elapsed > 0:
            daily_average = self._credits_used_month / days_elapsed
            projected_monthly = daily_average * 30
        else:
            projected_monthly = self._credits_used_month

        # Determine budget status
        remaining_ratio = credits_remaining / self._daily_budget if self._daily_budget > 0 else 0
        if remaining_ratio > 0.5:
            status = "healthy"
        elif remaining_ratio > 0.2:
            status = "warning"
        else:
            status = "critical"

        return CreditSnapshot(
            timestamp=now,
            credits_used=self._credits_used_today,
            credits_remaining=int(credits_remaining),
            daily_usage=self._credits_used_today,
            requests_made=self._requests_today,
            requests_per_second=current_rps,
            projected_monthly=int(projected_monthly),
            budget_status=status
        )

    def get_optimization_suggestions(self) -> List[str]:
        """Get optimization suggestions based on current usage."""
        suggestions = []
        snapshot = self.get_snapshot()

        # Budget status suggestions
        if snapshot.budget_status == "critical":
            suggestions.append("URGENT: Credit budget critical - enable conservative mode immediately")
            suggestions.append("Consider reducing wallet discovery scope")
            suggestions.append("Skip non-critical enrichment operations")
        elif snapshot.budget_status == "warning":
            suggestions.append("Credit budget below 20% - prioritize high-value requests")
            suggestions.append("Reduce analysis depth for low-conviction wallets")

        # Rate limit suggestions
        if snapshot.requests_per_second > 40:
            suggestions.append(f"Approaching rate limit ({snapshot.requests_per_second:.1f}/50 req/s)")

        # Projected monthly usage
        if snapshot.projected_monthly > self._budget.MONTHLY_CREDITS:
            suggestions.append(f"WARNING: Projected to exceed monthly budget ({snapshot.projected_monthly:,.0f} > {self._budget.MONTHLY_CREDITS:,.0f})")

        # Growth optimization suggestions
        if self._growth_optimized:
            discovery_ratio = self._discovery_spent / max(1, self._credits_used_today)
            if discovery_ratio > 0.4:
                suggestions.append("Discovery using >40% of budget - consider pre-filtering candidates")

            analysis_ratio = self._analysis_spent / max(1, self._credits_used_today)
            if analysis_ratio < 0.3:
                suggestions.append("Analysis budget underutilized - increase wallet analysis depth")

        return suggestions

    def optimize_for_growth(self, wallet_wqs: Optional[float] = None) -> int:
        """
        Calculate optimized credit allocation for growth goal ($200 → $1000).

        Args:
            wallet_wqs: WQS score of wallet being analyzed

        Returns:
            Recommended credit multiplier (1.0 = normal, 2.0 = double budget)
        """
        if not self._growth_optimized or wallet_wqs is None:
            return 1.0

        # High-conviction wallets get more analysis budget
        if wallet_wqs >= 70:
            return int(self._budget.HIGH_CONVICTIO_BONUS)
        elif wallet_wqs >= 60:
            return 1
        elif wallet_wqs >= 50:
            return 0.5  # Reduce analysis for emerging wallets
        else:
            return 0.2  # Minimal analysis for low-conviction wallets

    def should_skip_operation(self, operation: str, wallet_wqs: Optional[float] = None) -> bool:
        """
        Determine if an operation should be skipped based on budget and wallet quality.

        Args:
            operation: Operation type (enrichment, metadata, validation)
            wallet_wqs: WQS score of wallet

        Returns:
            True if operation should be skipped
        """
        snapshot = self.get_snapshot()

        # Skip non-critical operations when budget is critical
        if snapshot.budget_status == "critical" and operation != "validation":
            return True

        # Skip enrichment for low-conviction wallets when budget is tight
        if operation == "enrichment" and wallet_wqs and wallet_wqs < 50:
            if snapshot.budget_status != "healthy":
                return True

        # Skip optional metadata when budget warning
        if operation == "metadata" and snapshot.budget_status == "warning":
            return True

        return False

    def print_status_report(self):
        """Print a comprehensive status report."""
        snapshot = self.get_snapshot()

        print("\n" + "="*70)
        print("HELIUS CREDIT TRACKER - STATUS REPORT")
        print("="*70)

        print(f"\nTime: {datetime.fromtimestamp(snapshot.timestamp).strftime('%Y-%m-%d %H:%M:%S')}")
        print(f"Status: {snapshot.budget_status.upper()}")

        print(f"\nDaily Usage:")
        print(f"  Credits used: {snapshot.credits_used:,.0f} / {self._daily_budget:,.0f}")
        print(f"  Credits remaining: {snapshot.credits_remaining:,.0f}")
        print(f"  Requests made: {snapshot.requests_made}")
        print(f"  Request rate: {snapshot.requests_per_second:.1f} / {self._budget.MAX_REQUESTS_PER_SECOND} req/s")

        print(f"\nCategory Breakdown:")
        print(f"  Discovery: {self._discovery_spent:,.0f} / {self._discovery_budget:,.0f} credits")
        print(f"  Analysis: {self._analysis_spent:,.0f} / {self._analysis_budget:,.0f} credits")
        print(f"  Validation: {self._validation_spent:,.0f} / {self._validation_budget:,.0f} credits")
        print(f"  Reserve: {self._reserve_spent:,.0f} / {self._reserve_budget:,.0f} credits")

        print(f"\nProjections:")
        print(f"  Projected monthly: {snapshot.projected_monthly:,.0f} / {self._budget.MONTHLY_CREDITS:,.0f}")

        # Optimization suggestions
        suggestions = self.get_optimization_suggestions()
        if suggestions:
            print(f"\nOptimization Suggestions:")
            for i, suggestion in enumerate(suggestions, 1):
                print(f"  {i}. {suggestion}")

        print("="*70 + "\n")

    def shutdown(self):
        """Cleanup and save final state."""
        self._save_state()
        logger.info("Helius Credit Tracker shut down")


# Global singleton instance
_credit_tracker: Optional[HeliusCreditTracker] = None
_tracker_lock = threading.Lock()


def get_credit_tracker() -> HeliusCreditTracker:
    """Get the global credit tracker singleton."""
    global _credit_tracker

    with _tracker_lock:
        if _credit_tracker is None:
            _credit_tracker = HeliusCreditTracker()

    return _credit_tracker


def reset_credit_tracker():
    """Reset the global credit tracker (mainly for testing)."""
    global _credit_tracker

    with _tracker_lock:
        if _credit_tracker:
            _credit_tracker.shutdown()
        _credit_tracker = None


# Convenience functions for common operations
def can_fetch_wallet_transactions() -> Tuple[bool, str]:
    """Check if we can fetch wallet transactions."""
    tracker = get_credit_tracker()
    return tracker.can_make_request(
        cost=CreditCost.GET_TRANSACTIONS.value * 10,  # Assume 10 pages
        category="discovery",
        priority=RequestPriority.HIGH,
        expected_value=0.8
    )


def can_analyze_wallet(wallet_wqs: Optional[float] = None) -> Tuple[bool, str]:
    """Check if we can analyze a wallet."""
    tracker = get_credit_tracker()
    multiplier = tracker.optimize_for_growth(wallet_wqs)
    return tracker.can_make_request(
        cost=int(CreditCost.SWAP_ANALYSIS.value * 50 * multiplier),  # Assume 50 swaps
        category="analysis",
        priority=RequestPriority.HIGH if wallet_wqs and wallet_wqs >= 60 else RequestPriority.MEDIUM,
        expected_value=0.7 if wallet_wqs and wallet_wqs >= 60 else 0.5
    )


def can_validate_backtest() -> Tuple[bool, str]:
    """Check if we can run backtest validation."""
    tracker = get_credit_tracker()
    return tracker.can_make_request(
        cost=CreditCost.POSITION_TRACK.value * 20,  # Assume 20 trades
        category="validation",
        priority=RequestPriority.CRITICAL,
        expected_value=0.9
    )


if __name__ == "__main__":
    # Test the credit tracker
    tracker = get_credit_tracker()
    tracker.print_status_report()

    # Test some operations
    print("\nTesting operations:")
    print(f"Can fetch wallet transactions: {can_fetch_wallet_transactions()}")
    print(f"Can analyze high-WQS wallet: {can_analyze_wallet(75)}")
    print(f"Can analyze low-WQS wallet: {can_analyze_wallet(40)}")
    print(f"Can validate backtest: {can_validate_backtest()}")

    tracker.shutdown()