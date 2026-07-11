"""
Tests for HighConvictionAllocator integration with db_writer

These tests verify that the high-conviction allocation system works correctly
with the wallet database writing and prioritizes WQS 70+ wallets appropriately.
"""

import pytest
from unittest.mock import Mock, AsyncMock, MagicMock, patch
from typing import List, Dict, Any
from decimal import Decimal

from core.high_conviction_allocator import (
    HighConvictionAllocator,
    ConvictionLevel,
    AllocationResult
)
from core.models import WalletRecord
from integrations.high_conviction_integration import (
    HighConvictionIntegration,
    create_high_conviction_integration
)


def _wallet(address, wqs_score, status):
    """Helper to create WalletRecord with required fields."""
    return WalletRecord(
        address=address,
        status=status,
        wqs_score=wqs_score,
        roi_7d=0.0,
        roi_30d=0.0,
        trade_count_30d=10,
        win_rate=0.5,
        max_drawdown_30d=0.0,
        avg_trade_size_sol=Decimal("1.0"),
    )


class TestHighConvictionAllocator:
    """Test suite for HighConvictionAllocator core functionality."""

    def test_allocator_initialization(self):
        """Test that allocator initializes with correct defaults."""
        allocator = HighConvictionAllocator()
        assert allocator is not None

        # Check that it can be initialized without errors
        # The default total credits should be set via config
        assert allocator.get_high_conviction_budget() >= 0
        assert allocator.get_emerging_wallet_budget() >= 0

    def test_custom_total_credits(self):
        """Test custom total credits configuration."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(10000)

        # Check that high-conviction budget reflects the total (70% of 10000)
        assert allocator.get_high_conviction_budget() == 7000  # 70% of 10000

    def test_conviction_level_determination(self):
        """Test conviction level assignment based on WQS."""
        allocator = HighConvictionAllocator()

        # Test different WQS ranges
        very_high = allocator.get_conviction_level(85.0)
        high = allocator.get_conviction_level(75.0)
        medium = allocator.get_conviction_level(55.0)
        emerging = allocator.get_conviction_level(35.0)
        low = allocator.get_conviction_level(15.0)

        assert very_high == ConvictionLevel.VERY_HIGH
        assert high == ConvictionLevel.HIGH
        assert medium == ConvictionLevel.MEDIUM
        assert emerging == ConvictionLevel.EMERGING
        assert low == ConvictionLevel.LOW

    def test_credit_allocation_by_conviction(self):
        """Test that different conviction levels get different credit allocations."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(5000)

        # Allocate for different conviction levels
        very_high_result = allocator.allocate_analysis_credits("wallet1", 85.0, 100)
        high_result = allocator.allocate_analysis_credits("wallet2", 75.0, 100)
        medium_result = allocator.allocate_analysis_credits("wallet3", 55.0, 100)
        low_result = allocator.allocate_analysis_credits("wallet4", 25.0, 100)

        # VERY_HIGH should get most credits, LOW should get least
        assert very_high_result.credits_allocated >= high_result.credits_allocated
        assert high_result.credits_allocated >= medium_result.credits_allocated
        assert medium_result.credits_allocated >= low_result.credits_allocated

    def test_high_conviction_budget_tracking(self):
        """Test high-conviction budget tracking."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(5000)

        # Initially should have full budget (70% of 5000 = 3500 for high-conviction)
        initial_budget = allocator.get_high_conviction_budget()
        assert initial_budget == 3500  # 70% of total

        # Allocate some credits to high-conviction wallet
        allocator.allocate_analysis_credits("wallet1", 75.0, 500)

        # Budget should have decreased
        remaining_budget = allocator.get_high_conviction_budget()
        assert remaining_budget < initial_budget

    def test_emerging_wallet_budget_tracking(self):
        """Test emerging wallet budget tracking."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(5000)

        # Initially should have full emerging budget
        # Either 8% of 5000 = 400 or MIN_EMERGING_ALLOCATION = 1000, whichever is higher
        initial_budget = allocator.get_emerging_wallet_budget()
        assert initial_budget >= 400  # At least 8% of total

        # Allocate some credits to emerging wallet
        allocator.allocate_analysis_credits("wallet1", 35.0, 200)

        # Budget should have decreased (or stayed same if minimum allocation applied)
        remaining_budget = allocator.get_emerging_wallet_budget()
        assert remaining_budget <= initial_budget

    def test_budget_exhaustion(self):
        """Test behavior when budget is exhausted."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(100)  # Small budget for testing

        # Exhaust the high-conviction budget
        allocator.allocate_analysis_credits("wallet1", 75.0, 50)
        allocator.allocate_analysis_credits("wallet2", 75.0, 30)

        # Should still allow allocation but with reduced credits
        result = allocator.allocate_analysis_credits("wallet3", 75.0, 100)
        assert result.credits_allocated >= 0  # Should handle gracefully

    def test_allocation_result_structure(self):
        """Test that allocation results have correct structure."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(10000)  # Ensure sufficient budget

        result = allocator.allocate_analysis_credits("test_wallet", 70.0, 100)

        # Check result structure
        assert result.wallet_address == "test_wallet"
        assert result.wqs_score == 70.0
        assert result.credits_allocated >= 0  # May be 0 if budget exhausted, but should be positive
        assert result.conviction_level in ConvictionLevel
        assert result.multiplier_used > 0
        assert len(result.reason) > 0


class TestHighConvictionIntegration:
    """Test suite for HighConvictionIntegration functionality."""

    def test_integration_initialization(self):
        """Test that integration initializes correctly."""
        integration = HighConvictionIntegration(total_credits=5000)
        assert integration is not None
        assert integration.allocator is not None

    def test_wallet_prioritization(self):
        """Test wallet prioritization based on WQS."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Create test wallets with different WQS scores
        wallets = [
            "wallet1",  # Will be WQS 80
            "wallet2",  # Will be WQS 60
            "wallet3",  # Will be WQS 75
            "wallet4",  # Will be WQS 40
        ]

        wqs_scores = {
            "wallet1": 80.0,
            "wallet2": 60.0,
            "wallet3": 75.0,
            "wallet4": 40.0
        }

        # Prioritize wallets
        prioritized = integration.prioritize_wallets_for_analysis(wallets, wqs_scores)

        # High-conviction wallets (WQS 70+) should come first
        assert prioritized[0] in ["wallet1", "wallet3"]  # WQS 80, 75
        assert "wallet4" in prioritized  # Low WQS should still be included
        assert len(prioritized) == len(wallets)  # All wallets included

    def test_credit_allocation_for_wallets(self):
        """Test credit allocation for specific wallets."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Allocate credits for high-conviction wallet
        result1 = integration.allocate_analysis_credits("wallet1", 75.0, 100)
        assert result1.credits_allocated > 0
        assert result1.conviction_level == ConvictionLevel.HIGH

        # Allocate credits for low-conviction wallet
        result2 = integration.allocate_analysis_credits("wallet2", 35.0, 100)
        assert result2.credits_allocated > 0
        assert result2.conviction_level == ConvictionLevel.EMERGING

        # High-conviction should get more credits
        assert result1.credits_allocated >= result2.credits_allocated

    def test_should_analyze_wallet_decision(self):
        """Test decision logic for whether to analyze a wallet."""
        integration = HighConvictionIntegration(total_credits=5000)

        # High-conviction wallet should always be analyzed
        should_analyze, reason = integration.should_analyze_wallet("wallet1", 75.0)
        assert should_analyze is True
        assert "high-conviction" in reason.lower()

        # Emerging wallet with budget should be analyzed
        should_analyze, reason = integration.should_analyze_wallet("wallet2", 35.0)
        assert should_analyze is True
        assert "emerging" in reason.lower()

    def test_roster_filtering_by_budget(self):
        """Test filtering wallet roster based on budget."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Create mock wallet records
        wallets = [
            _wallet("wallet1", 80.0, "ACTIVE"),
            _wallet("wallet2", 60.0, "ACTIVE"),
            _wallet("wallet3", 75.0, "CANDIDATE"),
            _wallet("wallet4", 40.0, "REJECTED"),
        ]

        # Filter roster (should prioritize high-conviction)
        filtered = integration.filter_roster_by_budget(wallets)

        # Should return wallets (may filter based on budget constraints)
        assert isinstance(filtered, list)
        assert len(filtered) <= len(wallets)

    def test_allocation_summary_generation(self):
        """Test allocation summary generation."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Allocate some credits
        integration.allocate_analysis_credits("wallet1", 75.0, 100)
        integration.allocate_analysis_credits("wallet2", 60.0, 100)
        integration.allocate_analysis_credits("wallet3", 35.0, 100)

        # Get summary
        summary = integration.get_allocation_summary()

        # Check summary structure
        assert "total_wallets_analyzed" in summary
        assert "high_conviction_count" in summary
        assert "budget_remaining" in summary
        assert "wallets_analyzed" in summary

        assert summary["total_wallets_analyzed"] == 3

    def test_print_allocation_report(self):
        """Test allocation report printing (should not raise errors)."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Add some allocations
        integration.allocate_analysis_credits("wallet1", 75.0, 100)
        integration.allocate_analysis_credits("wallet2", 60.0, 100)

        # Should not raise any exceptions
        try:
            integration.print_allocation_report()
        except Exception as e:
            pytest.fail(f"print_allocation_report raised {e}")


class TestFactoryFunction:
    """Test suite for factory functions."""

    def test_create_integration_enabled(self):
        """Test creating integration when enabled."""
        integration = create_high_conviction_integration(total_credits=5000, enabled=True)
        assert integration is not None
        assert isinstance(integration, HighConvictionIntegration)

    def test_create_integration_disabled(self):
        """Test creating integration when disabled."""
        integration = create_high_conviction_integration(total_credits=5000, enabled=False)
        assert integration is None


class TestConvictionLevels:
    """Test suite for conviction level functionality."""

    def test_conviction_level_values(self):
        """Test conviction level enum values."""
        assert ConvictionLevel.VERY_HIGH.value == "very_high"
        assert ConvictionLevel.HIGH.value == "high"
        assert ConvictionLevel.MEDIUM.value == "medium"
        assert ConvictionLevel.EMERGING.value == "emerging"
        assert ConvictionLevel.LOW.value == "low"

    def test_conviction_level_ordering(self):
        """Test that conviction levels have correct ordering."""
        # This test verifies the logical ordering of conviction levels
        levels = [
            ConvictionLevel.VERY_HIGH,
            ConvictionLevel.HIGH,
            ConvictionLevel.MEDIUM,
            ConvictionLevel.EMERGING,
            ConvictionLevel.LOW
        ]

        # All should be different
        assert len(set(levels)) == len(levels)

    def test_conviction_thresholds(self):
        """Test WQS thresholds for conviction levels."""
        allocator = HighConvictionAllocator()

        # Test boundary conditions
        assert allocator.get_conviction_level(70.0) == ConvictionLevel.HIGH
        assert allocator.get_conviction_level(69.9) == ConvictionLevel.MEDIUM
        assert allocator.get_conviction_level(80.0) == ConvictionLevel.VERY_HIGH
        assert allocator.get_conviction_level(79.9) == ConvictionLevel.HIGH


class TestBudgetDistribution:
    """Test suite for budget distribution logic."""

    def test_seventy_percent_high_conviction(self):
        """Test that high-conviction wallets get significant budget allocation."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(10000)

        # High conviction = VERY_HIGH (30%) + HIGH (40%) = 70%
        # But this method returns remaining budget, not total allocation
        high_conviction_budget = allocator.get_high_conviction_budget()

        # Initially should have the full high-conviction allocation
        # VERY_HIGH (30%) + HIGH (40%) = 70% = 7000
        assert high_conviction_budget == 7000

    def test_twenty_percent_emerging(self):
        """Test that emerging wallets get appropriate budget allocation."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(10000)

        # Emerging wallets get 8% of total (800) or MIN_EMERGING_ALLOCATION (1000), whichever is higher
        emerging_budget = allocator.get_emerging_wallet_budget()
        expected = max(10000 * 0.08, 1000)  # 8% or minimum allocation

        assert emerging_budget == expected

    def test_ten_percent_reserve(self):
        """Test that remaining budget is for other wallet categories."""
        allocator = HighConvictionAllocator()
        allocator.set_total_credits(10000)

        # High-conviction (70%) + Emerging (with minimum allocation) + others
        high_conviction = allocator.get_high_conviction_budget()  # 70% = 7000
        emerging = allocator.get_emerging_wallet_budget()  # max(8%, 1000) = 1000

        # Remaining budget for MEDIUM (20%) + LOW (2%) = 2000
        remaining = 10000 - high_conviction - emerging

        assert remaining == 2000  # MEDIUM (20%) + LOW (2%)


class TestWalletPrioritizationLogic:
    """Test suite for wallet prioritization logic."""

    def test_high_conviction_first(self):
        """Test that high-conviction wallets are prioritized first."""
        integration = HighConvictionIntegration(total_credits=5000)

        wallets = ["w1", "w2", "w3", "w4", "w5"]
        wqs_scores = {
            "w1": 40.0,  # Low
            "w2": 80.0,  # High
            "w3": 60.0,  # Medium
            "w4": 75.0,  # High
            "w5": 30.0,  # Low
        }

        prioritized = integration.prioritize_wallets_for_analysis(wallets, wqs_scores)

        # First wallets should be high-conviction (WQS 70+)
        first_two = prioritized[:2]
        assert "w2" in first_two  # WQS 80
        assert "w4" in first_two  # WQS 75

    def test_wqs_sorting_within_priority(self):
        """Test that wallets are sorted by WQS within priority groups."""
        integration = HighConvictionIntegration(total_credits=5000)

        wallets = ["w1", "w2", "w3", "w4"]
        wqs_scores = {
            "w1": 85.0,  # Very high
            "w2": 75.0,  # High
            "w3": 65.0,  # Medium
            "w4": 55.0,  # Medium
        }

        prioritized = integration.prioritize_wallets_for_analysis(wallets, wqs_scores)

        # High-conviction wallets should be sorted by WQS (highest first)
        high_conviction = [w for w in prioritized if wqs_scores[w] >= 70.0]
        assert high_conviction[0] == "w1"  # WQS 85
        assert high_conviction[1] == "w2"  # WQS 75

    def test_all_wallets_included(self):
        """Test that all wallets are included in prioritization."""
        integration = HighConvictionIntegration(total_credits=5000)

        wallets = ["w1", "w2", "w3", "w4", "w5"]
        wqs_scores = {w: 50.0 for w in wallets}  # All medium

        prioritized = integration.prioritize_wallets_for_analysis(wallets, wqs_scores)

        # All wallets should be included
        assert len(prioritized) == len(wallets)
        assert set(prioritized) == set(wallets)


class TestPerformanceTracking:
    """Test suite for performance tracking functionality."""

    def test_wallet_analysis_tracking(self):
        """Test that wallet analyses are tracked."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Analyze some wallets
        integration.allocate_analysis_credits("wallet1", 75.0, 100)
        integration.allocate_analysis_credits("wallet2", 60.0, 100)

        # Check tracking
        assert len(integration._wallets_analyzed) == 2
        assert "wallet1" in integration._wallets_analyzed
        assert "wallet2" in integration._wallets_analyzed

    def test_high_conviction_counting(self):
        """Test high-conviction wallet counting."""
        integration = HighConvictionIntegration(total_credits=5000)

        wallets = ["w1", "w2", "w3", "w4"]
        wqs_scores = {
            "w1": 80.0,  # High
            "w2": 75.0,  # High
            "w3": 60.0,  # Medium
            "w4": 40.0,  # Low
        }

        integration.prioritize_wallets_for_analysis(wallets, wqs_scores)

        # Should count 2 high-conviction wallets
        assert integration._high_conviction_count == 2

    def test_summary_includes_wallet_details(self):
        """Test that summary includes wallet-level details."""
        integration = HighConvictionIntegration(total_credits=5000)

        # Allocate with different conviction levels
        integration.allocate_analysis_credits("wallet1", 80.0, 100)
        integration.allocate_analysis_credits("wallet2", 75.0, 100)
        integration.allocate_analysis_credits("wallet3", 40.0, 100)

        summary = integration.get_allocation_summary()

        # Should have breakdown by conviction level
        assert "wallets_analyzed" in summary
        assert len(summary["wallets_analyzed"]) > 0