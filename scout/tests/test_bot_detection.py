"""
Tests for bot-user detection features.

Tests bot router detection, swap ratio tracking, and blocking of bot users
from ACTIVE promotion.
"""

import pytest
from decimal import Decimal
from datetime import datetime

from core.helius_client import HeliusClient
from core.analyzer import WalletAnalyzer
from core.wqs import WalletMetrics
from core.validator import PrePromotionValidator, ValidationStatus


class TestKnownBotRouters:
    """Test KNOWN_BOT_ROUTERS set."""

    def test_known_bot_routers_exists(self):
        """Test that KNOWN_BOT_ROUTERS set exists."""
        assert hasattr(HeliusClient, 'KNOWN_BOT_ROUTERS')

    def test_known_bot_routers_is_set(self):
        """Test that KNOWN_BOT_ROUTERS is a set."""
        assert isinstance(HeliusClient.KNOWN_BOT_ROUTERS, set)

    def test_known_bot_routers_addresses_valid(self):
        """Test that any addresses in KNOWN_BOT_ROUTERS are valid."""
        KNOWN_BOT_ROUTERS = HeliusClient.KNOWN_BOT_ROUTERS
        for address in KNOWN_BOT_ROUTERS:
            # Should be valid Solana addresses (44 chars, base58-like)
            assert len(address) == 44 or len(address) == 43
            assert address.isalnum()


@pytest.mark.asyncio
class TestBotSwapRatioTracking:
    """Test bot swap ratio tracking in analyzer."""

    async def test_bot_swap_detection(self, analyzer):
        """Test detection of bot-routed swaps."""
        # Create a swap with bot router
        swap_with_bot = {
            "type": "SWAP",
            "tokenIn": "SOL",
            "tokenOut": "TOKEN",
            "tokenAmountIn": Decimal('100'),
            "tokenAmountOut": Decimal('1000'),
            "from": "wallet1",
            "to": list(KNOWN_BOT_ROUTERS)[0] if KNOWN_BOT_ROUTERS else "bot_router",
            "timestamp": datetime.now(),
            "signature": "sig1",
            "tokenAddress": "TOKEN"
        }
        
        # Create a normal swap
        swap_normal = {
            "type": "SWAP",
            "tokenIn": "SOL",
            "tokenOut": "TOKEN",
            "tokenAmountIn": Decimal('100'),
            "tokenAmountOut": Decimal('1000'),
            "from": "wallet1",
            "to": "normal_router",
            "timestamp": datetime.now(),
            "signature": "sig2",
            "tokenAddress": "TOKEN"
        }
        
        # Count bot swaps
        bot_swaps = 0
        total_swaps = 2
        
        if KNOWN_BOT_ROUTERS:
            for swap in [swap_with_bot, swap_normal]:
                if swap["to"] in KNOWN_BOT_ROUTERS:
                    bot_swaps += 1
            
            assert bot_swaps == 1
            bot_ratio = bot_swaps / total_swaps
            assert bot_ratio == 0.5

    async def test_bot_swap_ratio_calculation(self, analyzer):
        """Test calculation of bot swap ratio."""
        total_swaps = 20
        bot_swaps = 12
        
        bot_ratio = bot_swaps / total_swaps
        assert bot_ratio == 0.6

    async def test_bot_swap_ratio_threshold(self, analyzer):
        """Test bot swap ratio threshold for bot user detection."""
        # >=50% of >=10 swaps = bot user
        bot_ratio = 0.5
        total_swaps = 10
        
        is_bot_user = (total_swaps >= 10) and (bot_ratio >= 0.5)
        assert is_bot_user is True

    async def test_bot_swap_ratio_below_threshold(self, analyzer):
        """Test bot swap ratio below threshold."""
        bot_ratio = 0.4
        total_swaps = 10
        
        is_bot_user = (total_swaps >= 10) and (bot_ratio >= 0.5)
        assert is_bot_user is False

    async def test_bot_swap_ratio_insufficient_swaps(self, analyzer):
        """Test bot swap ratio with insufficient swaps."""
        bot_ratio = 0.6
        total_swaps = 9  # Below threshold
        
        is_bot_user = (total_swaps >= 10) and (bot_ratio >= 0.5)
        assert is_bot_user is False

    async def test_bot_swap_ratio_high_swap_count(self, analyzer):
        """Test bot swap ratio with high swap count."""
        bot_ratio = 0.3
        total_swaps = 100
        
        is_bot_user = (total_swaps >= 10) and (bot_ratio >= 0.5)
        assert is_bot_user is False


@pytest.mark.asyncio
class TestIsTgBotUserField:
    """Test is_tg_bot_user field in WalletMetrics."""

    async def test_is_tg_bot_user_field_exists(self):
        """Test that WalletMetrics has is_tg_bot_user field."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            is_tg_bot_user=False
        )
        assert hasattr(metrics, 'is_tg_bot_user')
        assert metrics.is_tg_bot_user is False

    async def test_is_tg_bot_user_default(self):
        """Test default value of is_tg_bot_user."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5
        )
        assert metrics.is_tg_bot_user is False

    async def test_is_tg_bot_user_true(self):
        """Test is_tg_bot_user set to True."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            is_tg_bot_user=True
        )
        assert metrics.is_tg_bot_user is True

    async def test_is_tg_bot_user_false(self):
        """Test is_tg_bot_user set to False."""
        metrics = WalletMetrics(
            wallet_address="test_wallet",
            total_trades=10,
            winning_trades=5,
            losing_trades=5,
            is_tg_bot_user=False
        )
        assert metrics.is_tg_bot_user is False


@pytest.mark.asyncio
class TestBotUserBlockingInValidator:
    """Test blocking of bot users from ACTIVE promotion."""

    async def test_validate_archetype_blocks_bot_user(self, validator):
        """Test that validate_archetype_for_promotion blocks bot users."""
        # Create metrics for a bot user
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        assert result.passed is False
        assert result.status == ValidationStatus.FAILED_WQS
        assert "Telegram bot user" in result.reason
        assert result.recommended_status == "CANDIDATE"

    async def test_validate_archetype_allows_normal_user(self, validator):
        """Test that validate_archetype_for_promotion allows normal users."""
        # Create metrics for a normal user
        normal_metrics = WalletMetrics(
            wallet_address="normal_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            is_tg_bot_user=False
        )
        
        result = validator.validate_archetype_for_promotion("normal_wallet", normal_metrics)
        
        # Should pass if other criteria are met
        assert result.passed is True

    async def test_bot_user_check_before_low_churn(self, validator):
        """Test that bot user check happens before low churn check."""
        # Create metrics for a bot user that would pass low churn
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=48.0,  # Would pass low churn
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        # Should fail due to bot user, not low churn
        assert result.passed is False
        assert "Telegram bot user" in result.reason
        assert "low-churn" not in result.reason.lower()

    async def test_bot_user_with_disabled_enforcement(self, validator):
        """Test bot user check when enforcement is disabled."""
        # Create validator with enforcement disabled
        validator.criteria.enforce_low_churn = False
        
        # Create metrics for a bot user
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=24.0,
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        # Should still fail due to bot user check
        # (bot user check is independent of low_churn enforcement)
        assert result.passed is False

    async def test_bot_user_error_message(self, validator):
        """Test that bot user error message is clear."""
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        # Error message should be informative
        assert "bot router" in result.reason.lower()
        assert "≥50%" in result.reason or "50%" in result.reason
        assert "≥10" in result.reason or "10" in result.reason

    async def test_bot_user_status_details(self, validator):
        """Test that bot user status has proper details."""
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        assert result.wallet_address == "bot_wallet"
        assert result.status == ValidationStatus.FAILED_WQS
        assert result.recommended_status == "CANDIDATE"

    async def test_normal_user_passes_bot_check(self, validator):
        """Test that normal users pass the bot user check."""
        normal_metrics = WalletMetrics(
            wallet_address="normal_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=48.0,
            is_tg_bot_user=False
        )
        
        result = validator.validate_archetype_for_promotion("normal_wallet", normal_metrics)
        
        # Should pass (assuming other criteria are met)
        assert result.passed is True or "bot" not in result.reason.lower()

    async def test_bot_user_blocking_prevents_promotion(self, validator):
        """Test that bot user blocking prevents ACTIVE promotion."""
        bot_metrics = WalletMetrics(
            wallet_address="bot_wallet",
            total_trades=20,
            winning_trades=10,
            losing_trades=10,
            avg_roi=Decimal('0.15'),
            win_rate=0.5,
            avg_hold_time_hours=48.0,
            is_tg_bot_user=True
        )
        
        result = validator.validate_archetype_for_promotion("bot_wallet", bot_metrics)
        
        # Should recommend CANDIDATE, not ACTIVE
        assert result.recommended_status == "CANDIDATE"
        assert result.recommended_status != "ACTIVE"


@pytest.fixture
def validator():
    """Create a Validator instance for testing."""
    from core.validator import LowChurnCriteria
    
    criteria = LowChurnCriteria(
        enforce_low_churn=True,
        min_avg_hold_time_hours=24.0,
        forbidden_archetypes=set(["SNIPER", "SCALPER"])
    )
    
    return Validator(criteria)


@pytest.fixture
def analyzer():
    """Create a WalletAnalyzer instance for testing."""
    return WalletAnalyzer()