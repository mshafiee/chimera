"""
Tests for bot-user detection features.

Tests bot router detection, swap ratio tracking, and blocking of bot users
from ACTIVE promotion.
"""

import pytest

from core.helius_client import HeliusClient
from core.analyzer import WalletAnalyzer
from core.wqs import WalletMetrics
from core.validator import PrePromotionValidator, PromotionCriteria, ValidationStatus


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
        for address in HeliusClient.KNOWN_BOT_ROUTERS:
            # Should be valid Solana addresses (44 chars, base58-like)
            assert len(address) == 44 or len(address) == 43
            assert address.isalnum()


@pytest.mark.asyncio
class TestIsTgBotUserField:
    """Test is_tg_bot_user field in WalletMetrics."""

    def _metrics(self, is_bot=None):
        kwargs = dict(
            address="test_wallet",
            trade_count_30d=10,
            win_rate=0.5,
        )
        if is_bot is not None:
            kwargs["is_tg_bot_user"] = is_bot
        return WalletMetrics(**kwargs)

    async def test_is_tg_bot_user_field_exists(self):
        """Test that WalletMetrics has is_tg_bot_user field."""
        metrics = self._metrics(is_bot=False)
        assert hasattr(metrics, 'is_tg_bot_user')
        assert metrics.is_tg_bot_user is False

    async def test_is_tg_bot_user_default(self):
        """Test default value of is_tg_bot_user."""
        metrics = self._metrics()
        assert metrics.is_tg_bot_user is False

    async def test_is_tg_bot_user_true(self):
        """Test is_tg_bot_user set to True."""
        metrics = self._metrics(is_bot=True)
        assert metrics.is_tg_bot_user is True

    async def test_is_tg_bot_user_false(self):
        """Test is_tg_bot_user set to False."""
        metrics = self._metrics(is_bot=False)
        assert metrics.is_tg_bot_user is False


@pytest.mark.asyncio
class TestBotUserBlockingInValidator:
    """Test blocking of bot users from ACTIVE promotion."""

    def _metrics(self, is_bot=True, avg_hold_hours=24.0):
        return WalletMetrics(
            address="bot_wallet" if is_bot else "normal_wallet",
            trade_count_30d=20,
            win_rate=0.5,
            avg_hold_time_hours=avg_hold_hours,
            is_tg_bot_user=is_bot,
        )

    async def test_validate_archetype_blocks_bot_user(self, validator):
        """Test that validate_archetype_for_promotion blocks bot users."""
        result = validator.validate_archetype_for_promotion("bot_wallet", self._metrics(is_bot=True))

        assert result.passed is False
        assert result.status == ValidationStatus.FAILED_WQS
        assert "Telegram bot user" in result.reason
        assert result.recommended_status == "CANDIDATE"

    async def test_validate_archetype_allows_normal_user(self, validator):
        """Test that validate_archetype_for_promotion allows normal users."""
        result = validator.validate_archetype_for_promotion("normal_wallet", self._metrics(is_bot=False))

        assert result.passed is True

    async def test_bot_user_check_before_low_churn(self, validator):
        """Test that bot user check happens before low churn check."""
        # Would pass low churn, but is a bot user
        result = validator.validate_archetype_for_promotion(
            "bot_wallet", self._metrics(is_bot=True, avg_hold_hours=48.0)
        )

        assert result.passed is False
        assert "Telegram bot user" in result.reason
        assert "low-churn" not in result.reason.lower()

    async def test_bot_user_with_disabled_enforcement(self, validator):
        """Test bot user check when enforcement is disabled."""
        # Disable low-churn enforcement
        validator.criteria.enforce_low_churn = False

        result = validator.validate_archetype_for_promotion("bot_wallet", self._metrics(is_bot=True))

        # Should still fail due to bot user check
        # (bot user check is independent of low_churn enforcement)
        assert result.passed is False

    async def test_bot_user_error_message(self, validator):
        """Test that bot user error message is clear."""
        result = validator.validate_archetype_for_promotion("bot_wallet", self._metrics(is_bot=True))

        # Error message should be informative
        assert "bot router" in result.reason.lower()
        assert "≥50%" in result.reason or "50%" in result.reason
        assert "≥10" in result.reason or "10" in result.reason

    async def test_bot_user_status_details(self, validator):
        """Test that bot user status has proper details."""
        result = validator.validate_archetype_for_promotion("bot_wallet", self._metrics(is_bot=True))

        assert result.wallet_address == "bot_wallet"
        assert result.status == ValidationStatus.FAILED_WQS
        assert result.recommended_status == "CANDIDATE"

    async def test_normal_user_passes_bot_check(self, validator):
        """Test that normal users pass the bot user check."""
        result = validator.validate_archetype_for_promotion("normal_wallet", self._metrics(is_bot=False))

        assert result.passed is True
        assert "bot" not in result.reason.lower()

    async def test_bot_user_blocking_prevents_promotion(self, validator):
        """Test that bot user blocking prevents ACTIVE promotion."""
        result = validator.validate_archetype_for_promotion(
            "bot_wallet", self._metrics(is_bot=True, avg_hold_hours=48.0)
        )

        # Should recommend CANDIDATE, not ACTIVE
        assert result.recommended_status == "CANDIDATE"
        assert result.recommended_status != "ACTIVE"


@pytest.fixture
def validator():
    """Create a PrePromotionValidator instance for testing."""
    criteria = PromotionCriteria(
        enforce_low_churn=True,
        min_avg_hold_time_hours=24.0,
        forbidden_archetypes=set(["SNIPER", "SCALPER"])
    )

    return PrePromotionValidator(promotion_criteria=criteria)


@pytest.fixture
def analyzer():
    """Create a WalletAnalyzer instance for testing."""
    return WalletAnalyzer()
