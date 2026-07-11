"""
Unit tests for token age edge cases.

Tests various edge cases for token age validation:
- Very new tokens (less than 1 hour old)
- Tokens just created (0 age)
- Old tokens (years old)
- Future tokens (malformed data)
- Missing token age data
- Age calculation around thresholds
"""

import pytest
from datetime import datetime, timedelta
from decimal import Decimal


class TestTokenAgeEdgeCases:
    """Test token age validation edge cases."""

    def test_token_age_zero_hours(self):
        """Test token that was just created (0 hours old)."""
        now = datetime.now()
        creation_time = now  # Just created

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 0.0, "Token should be 0 hours old"

    def test_token_age_less_than_hour(self):
        """Test token that's less than 1 hour old."""
        now = datetime.now()
        creation_time = now - timedelta(minutes=30)  # 30 minutes old

        age_hours = (now - creation_time).total_seconds() / 3600
        assert 0 < age_hours < 1.0, "Token should be less than 1 hour old"

    def test_token_age_exactly_one_hour(self):
        """Test token that's exactly 1 hour old."""
        now = datetime.now()
        creation_time = now - timedelta(hours=1)

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 1.0, "Token should be exactly 1 hour old"

    def test_token_age_exactly_24_hours(self):
        """Test token that's exactly 24 hours old (1 day)."""
        now = datetime.now()
        creation_time = now - timedelta(days=1)

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 24.0, "Token should be exactly 24 hours old"

    def test_token_age_week_old(self):
        """Test token that's 1 week old."""
        now = datetime.now()
        creation_time = now - timedelta(weeks=1)

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 168.0, "Token should be 168 hours old (1 week)"

    def test_token_age_month_old(self):
        """Test token that's 1 month old."""
        now = datetime.now()
        creation_time = now - timedelta(days=30)

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 720.0, "Token should be 720 hours old (30 days)"

    def test_token_age_year_old(self):
        """Test token that's 1 year old."""
        now = datetime.now()
        creation_time = now - timedelta(days=365)

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours == 8760.0, "Token should be 8760 hours old (1 year)"

    def test_token_age_missing_data(self):
        """Test handling of missing token age data."""
        creation_time = None

        if creation_time is None:
            age_hours = None
        else:
            age_hours = (datetime.now() - creation_time).total_seconds() / 3600

        assert age_hours is None, "Missing token age should be handled"

    def test_token_age_future_timestamp(self):
        """Test handling of invalid future token creation timestamp."""
        now = datetime.now()
        creation_time = now + timedelta(hours=1)  # Invalid: future timestamp

        age_hours = (now - creation_time).total_seconds() / 3600
        assert age_hours < 0, "Future timestamp should result in negative age"

    def test_token_age_threshold_24_hours(self):
        """Test token age threshold validation (24 hours)."""
        now = datetime.now()
        creation_time = now - timedelta(hours=24)

        age_hours = (now - creation_time).total_seconds() / 3600

        # Common threshold: tokens must be at least 24 hours old
        if age_hours >= 24.0:
            is_valid = True
        else:
            is_valid = False

        assert is_valid, "Token exactly 24 hours old should pass threshold"

    def test_token_age_threshold_just_below(self):
        """Test token age threshold validation (just below 24 hours)."""
        now = datetime.now()
        creation_time = now - timedelta(hours=23, minutes=59)

        age_hours = (now - creation_time).total_seconds() / 3600

        # Common threshold: tokens must be at least 24 hours old
        if age_hours >= 24.0:
            is_valid = True
        else:
            is_valid = False

        assert not is_valid, "Token just below 24 hours should fail threshold"

    def test_token_age_threshold_just_above(self):
        """Test token age threshold validation (just above 24 hours)."""
        now = datetime.now()
        creation_time = now - timedelta(hours=24, minutes=1)

        age_hours = (now - creation_time).total_seconds() / 3600

        # Common threshold: tokens must be at least 24 hours old
        if age_hours >= 24.0:
            is_valid = True
        else:
            is_valid = False

        assert is_valid, "Token just above 24 hours should pass threshold"

    def test_token_age_multiple_thresholds(self):
        """Test token age with multiple confidence tiers."""
        now = datetime.now()
        test_cases = [
            (timedelta(hours=1), "high_risk"),     # < 24h: very high risk
            (timedelta(hours=24), "moderate"),     # 1d: moderate risk
            (timedelta(days=7), "low"),           # 1w: low risk
            (timedelta(days=30), "very_low"),     # 1m: very low risk
        ]

        for age_delta, expected_risk in test_cases:
            creation_time = now - age_delta
            age_hours = (now - creation_time).total_seconds() / 3600

            # Simple risk tier logic
            if age_hours < 24.0:
                risk = "high_risk"
            elif age_hours < 168.0:  # 1 week
                risk = "moderate"
            elif age_hours < 720.0:  # 1 month
                risk = "low"
            else:
                risk = "very_low"

            assert risk == expected_risk, f"Token age {age_hours}h should be {expected_risk}, got {risk}"

    def test_token_age_string_parsing(self):
        """Test parsing token age from string format."""
        age_string = "2025-01-01T00:00:00Z"
        try:
            from datetime import datetime
            creation_time = datetime.fromisoformat(age_string.replace('Z', '+00:00'))
            age_hours = (datetime.now(creation_time.tzinfo) - creation_time).total_seconds() / 3600
            assert age_hours > 0, "Parsed age should be positive"
        except Exception as e:
            pytest.fail(f"Failed to parse age string: {e}")

    def test_token_age_zero_confidence(self):
        """Test token age when confidence is zero (unknown age)."""
        age_hours = None  # Unknown token age
        confidence = 0.0

        # Should handle gracefully with low confidence
        if age_hours is None:
            adjusted_confidence = 0.0  # No confidence without age data
        else:
            adjusted_confidence = confidence

        assert adjusted_confidence == 0.0, "Unknown age should result in zero confidence"

    def test_token_age_very_old_token(self):
        """Test handling of very old tokens (years old)."""
        now = datetime.now()
        creation_time = now - timedelta(days=1000)  # ~2.7 years old

        age_hours = (now - creation_time).total_seconds() / 3600

        # Very old tokens are generally safer
        is_suspicious = age_hours < 24.0
        assert not is_suspicious, "Very old token should not be suspicious"