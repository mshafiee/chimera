"""
Utility functions for the Scout intelligence layer.

This module provides common utilities used across the Scout codebase,
including timezone-aware datetime handling.
"""

from datetime import datetime, timezone


def utcnow() -> datetime:
    """
    Get current UTC time as timezone-aware datetime.

    This is the recommended way to get the current time in Scout.
    It ensures all datetime objects are timezone-aware, preventing
    issues with naive/aware datetime comparisons.

    Returns:
        datetime: Current UTC time with timezone info set.

    Example:
        >>> from scout.core.utils import utcnow
        >>> now = utcnow()
        >>> now.tzinfo is not None
        True
    """
    return datetime.now(timezone.utc)


def parse_utc_timestamp(ts: str) -> datetime:
    """
    Parse ISO timestamp and ensure it is timezone-aware.

    If the timestamp lacks timezone information, UTC is assumed.
    This provides safe parsing for both aware and naive ISO strings.

    Args:
        ts: ISO format timestamp string (e.g., "2025-06-18T12:00:00" or
            "2025-06-18T12:00:00+00:00")

    Returns:
        datetime: Timezone-aware datetime object.

    Raises:
        ValueError: If the timestamp cannot be parsed.

    Example:
        >>> from scout.core.utils import parse_utc_timestamp
        >>> dt = parse_utc_timestamp("2025-06-18T12:00:00")
        >>> dt.tzinfo is not None
        True
    """
    dt = datetime.fromisoformat(ts)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt
