"""
Utility functions for Decimal conversions at boundaries.

This module provides helper functions for converting between Decimal and float
at appropriate boundaries (database, JSON, API responses), following the same
pattern as the Rust codebase.
"""

from decimal import Decimal
from typing import Optional, Union


def float_to_decimal(value: Optional[Union[float, str, int]]) -> Decimal:
    """
    Safely convert float to Decimal, handling None and edge cases.
    
    This is used when reading from external sources (database, JSON, API).
    
    Args:
        value: Float, string, int, or None to convert
        
    Returns:
        Decimal value, or Decimal('0') if value is None or invalid
    """
    if value is None:
        return Decimal('0')
    
    if isinstance(value, Decimal):
        return value
    
    if isinstance(value, str):
        try:
            return Decimal(value)
        except (ValueError, TypeError):
            return Decimal('0')
    
    if isinstance(value, (int, float)):
        try:
            # Use string conversion to avoid floating point precision issues
            return Decimal(str(value))
        except (ValueError, TypeError):
            return Decimal('0')
    
    return Decimal('0')


def decimal_to_float(value: Optional[Decimal]) -> float:
    """
    Convert Decimal to float for JSON serialization or database storage.
    
    This should only be used at boundaries (API responses, database writes).
    Internal calculations should always use Decimal.
    
    Args:
        value: Decimal value to convert
        
    Returns:
        Float value, or 0.0 if value is None
    """
    if value is None:
        return 0.0
    
    try:
        return float(value)
    except (ValueError, TypeError):
        return 0.0


def safe_decimal_divide(numerator: Decimal, denominator: Decimal, default: Decimal = Decimal('0')) -> Decimal:
    """
    Safely divide two Decimals, returning default if denominator is zero.
    
    Args:
        numerator: Decimal numerator
        denominator: Decimal denominator
        default: Value to return if denominator is zero
        
    Returns:
        Decimal result of division, or default if denominator is zero
    """
    if denominator == Decimal('0'):
        return default
    return numerator / denominator
