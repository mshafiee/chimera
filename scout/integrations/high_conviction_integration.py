"""
High-Conviction Allocator Integration for Scout

This module integrates the HighConvictionAllocator into the Scout pipeline to:
1. Prioritize WQS 70+ wallets for analysis (70% of budget)
2. Allocate analysis credits based on conviction levels
3. Track conviction-level performance for optimization
4. Filter roster based on high-conviction budgets

The allocator ensures that Scout focuses its limited Helius API quota on
high-WQS wallets that are most likely to be profitable.
"""

import logging
from typing import List, Dict, Any, Optional
from core.high_conviction_allocator import HighConvictionAllocator, ConvictionLevel, AllocationResult
from core.models import WalletRecord

logger = logging.getLogger(__name__)


class HighConvictionIntegration:
    """
    Integration layer for high-conviction allocation in Scout pipeline.

    Features:
    - Prioritizes WQS 70+ wallets for analysis
    - Allocates analysis budget based on conviction levels
    - Filters roster based on available conviction budgets
    - Tracks performance by conviction level
    """

    def __init__(self, total_credits: int = 5000):
        """
        Initialize high-conviction integration.

        Args:
            total_credits: Total analysis credits available for the run
        """
        self.allocator = HighConvictionAllocator()
        self.allocator.set_total_credits(total_credits)

        self._wallets_analyzed: Dict[str, AllocationResult] = {}
        self._total_wallets = 0
        self._high_conviction_count = 0

        logger.info("HighConvictionIntegration initialized with %s credits", total_credits)

    def prioritize_wallets_for_analysis(
        self, wallet_addresses: List[str], wqs_scores: Dict[str, float]
    ) -> List[str]:
        """
        Prioritize wallets for analysis based on WQS scores.

        High-conviction wallets (WQS 70+) get prioritized to the front of the queue.

        Args:
            wallet_addresses: List of wallet addresses to prioritize
            wqs_scores: Dict mapping wallet_address -> WQS score

        Returns:
            Prioritized list of wallet addresses
        """
        if not wallet_addresses or not wqs_scores:
            return wallet_addresses

        # Separate into high-conviction (WQS 70+) and others
        high_conviction = []
        others = []

        for addr in wallet_addresses:
            wqs = wqs_scores.get(addr, 0)
            if wqs >= 70.0:
                high_conviction.append((addr, wqs))
            else:
                others.append((addr, wqs))

        # Sort each group by WQS descending
        high_conviction.sort(key=lambda x: x[1], reverse=True)
        others.sort(key=lambda x: x[1], reverse=True)

        # Combine: high-conviction first (70% of budget), then others (30%)
        high_conviction_addrs = [addr for addr, _ in high_conviction]
        other_addrs = [addr for addr, _ in others]

        prioritized = high_conviction_addrs + other_addrs

        self._high_conviction_count = len(high_conviction)
        self._total_wallets = len(wallet_addresses)

        logger.info(
            "Prioritized %d high-conviction wallets (%d/%d)",
            len(high_conviction), self._high_conviction_count, self._total_wallets,
        )

        return prioritized

    def allocate_analysis_credits(
        self, wallet_address: str, wqs_score: float, base_credits: int = 100
    ) -> AllocationResult:
        """
        Allocate analysis credits for a wallet based on WQS.

        Args:
            wallet_address: Wallet to allocate credits for
            wqs_score: Current WQS score
            base_credits: Base credit amount to allocate

        Returns:
            AllocationResult with allocated credits
        """
        result = self.allocator.allocate_analysis_credits(
            wallet_address, wqs_score, base_credits
        )

        self._wallets_analyzed[wallet_address] = result

        logger.debug(
            "Allocated %s credits to %s (WQS: %.1f, Level: %s)",
            result.credits_allocated, wallet_address[:8], wqs_score, result.conviction_level.value,
        )

        return result

    def should_analyze_wallet(self, wallet_address: str, wqs_score: float) -> tuple[bool, str]:
        """
        Determine if we should analyze a wallet based on remaining budget.

        Args:
            wallet_address: Wallet to check
            wqs_score: Current WQS score

        Returns:
            Tuple of (should_analyze: bool, reason: str)
        """
        # Gate each conviction level on its OWN pool's remaining credits so an
        # exhausted pool actually blocks further approvals.
        level = self.allocator.get_conviction_level(wqs_score)
        summary = self.allocator.get_allocation_summary()
        by_level = summary['allocations_by_level']
        level_remaining = by_level.get(level.value, {}).get('remaining', 0)

        if level in [ConvictionLevel.VERY_HIGH, ConvictionLevel.HIGH]:
            high_budget = summary['high_conviction_budget']
            if high_budget > 0:
                return True, f"High-conviction wallet ({level.value}) with {high_budget} credits remaining"

        # Check the wallet's own conviction-level pool (emerging/medium/low)
        if level_remaining > 0:
            return True, f"{level.value} wallet with {level_remaining} credits remaining in its pool"

        return False, f"Insufficient budget for {level.value} wallet"

    def filter_roster_by_budget(
        self, wallets: List[WalletRecord]
    ) -> List[WalletRecord]:
        """
        Filter wallet roster based on available high-conviction budget.

        If budget is limited, prioritize WQS 70+ wallets for the ACTIVE roster.

        Args:
            wallets: Complete list of wallet records

        Returns:
            Filtered list of wallet records fitting within budget
        """
        if not wallets:
            return wallets

        # Get remaining high-conviction budget
        high_budget = self.allocator.get_high_conviction_budget()

        # If we have plenty of budget, return all wallets
        if high_budget >= len(wallets) * 100:
            logger.info("Sufficient budget for all %d wallets", len(wallets))
            return wallets

        # Budget-limited: fit the roster to what the budget can actually cover
        affordable = max(0, high_budget // 100)
        if affordable == 0:
            logger.warning("High-conviction budget exhausted - returning empty roster")
            return []

        # Prioritize high-conviction wallets, then others
        high_conviction = [w for w in wallets if w.wqs_score and w.wqs_score >= 70.0]
        others = [w for w in wallets if not (w.wqs_score and w.wqs_score >= 70.0)]

        # Allocate 70% of the affordable slots to high-conviction, 30% to others
        high_slots = min(len(high_conviction), max(1, int(affordable * 0.70)))
        other_slots = affordable - high_slots

        filtered = high_conviction[:high_slots] + others[:other_slots]

        logger.info(
            "Budget-limited roster: %d/%d wallets (%d high-conviction)",
            len(filtered), len(wallets), len(high_conviction[:high_slots]),
        )

        return filtered

    def get_allocation_summary(self) -> Dict[str, Any]:
        """Get summary of allocations and performance."""
        summary = {
            "total_wallets_analyzed": len(self._wallets_analyzed),
            "high_conviction_count": self._high_conviction_count,
            "total_wallets": self._total_wallets,
            "wallets_analyzed": {},
            "budget_remaining": {
                "high_conviction": self.allocator.get_high_conviction_budget(),
                "emerging": self.allocator.get_emerging_wallet_budget(),
            },
        }

        # Summarize by conviction level
        by_level = {}
        for addr, result in self._wallets_analyzed.items():
            level = result.conviction_level.value
            if level not in by_level:
                by_level[level] = {
                    "count": 0,
                    "credits_total": 0,
                    "wqs_scores": [],
                }

            by_level[level]["count"] += 1
            by_level[level]["credits_total"] += result.credits_allocated
            by_level[level]["wqs_scores"].append(result.wqs_score)

        summary["wallets_analyzed"] = by_level

        # Calculate average WQS by level
        for level_data in by_level.values():
            if level_data["wqs_scores"]:
                level_data["avg_wqs"] = sum(level_data["wqs_scores"]) / len(level_data["wqs_scores"])
            else:
                level_data["avg_wqs"] = 0.0

        return summary

    def print_allocation_report(self) -> None:
        """Print allocation summary to console."""
        summary = self.get_allocation_summary()

        print("\n" + "=" * 70)
        print("HIGH-CONVICTION ALLOCATION REPORT")
        print("=" * 70)

        print(f"Wallets Analyzed: {summary['total_wallets_analyzed']}")
        print(f"High-Conviction (WQS 70+): {summary['high_conviction_count']}")

        print("\nBudget Remaining:")
        print(f"  High-Conviction: {summary['budget_remaining']['high_conviction']:,} credits")
        print(f"  Emerging: {summary['budget_remaining']['emerging']:,} credits")

        print("\nAnalysis by Conviction Level:")
        for level, data in summary["wallets_analyzed"].items():
            print(f"  {level.upper()}:")
            print(f"    Wallets: {data['count']}")
            print(f"    Credits Used: {data['credits_total']:,}")
            print(f"    Avg WQS: {data['avg_wqs']:.1f}")

        print("=" * 70)


def create_high_conviction_integration(
    total_credits: int = 5000, enabled: bool = True
) -> Optional[HighConvictionIntegration]:
    """
    Factory function to create high-conviction integration.

    Args:
        total_credits: Total analysis credits available
        enabled: Whether to enable the integration

    Returns:
        HighConvictionIntegration instance or None if disabled
    """
    if not enabled:
        logger.info("High-conviction integration disabled")
        return None

    return HighConvictionIntegration(total_credits=total_credits)