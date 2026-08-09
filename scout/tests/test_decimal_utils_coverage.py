"""
Coverage tests for core/decimal_utils.py.
"""

import decimal

from decimal import Decimal

from core.decimal_utils import (
    decimal_to_float,
    float_to_decimal,
    safe_decimal_divide,
)


class _BadInt(int):
    """int subclass whose str() raises — exercises the except path."""

    def __str__(self):
        raise ValueError("boom")


def test_float_to_decimal_none():
    assert float_to_decimal(None) == Decimal('0')


def test_float_to_decimal_passthrough_decimal():
    assert float_to_decimal(Decimal('1.5')) == Decimal('1.5')


def test_float_to_decimal_valid_str():
    assert float_to_decimal("3.14") == Decimal('3.14')


def test_float_to_decimal_bad_str():
    assert float_to_decimal("not-a-number") == Decimal('0')


def test_float_to_decimal_float_uses_string_roundtrip():
    assert float_to_decimal(0.1) == Decimal('0.1')


def test_float_to_decimal_int():
    assert float_to_decimal(42) == Decimal('42')


def test_float_to_decimal_bad_numeric_raises_in_str():
    # str() of the int subclass raises -> Decimal construction fails
    assert float_to_decimal(_BadInt(1)) == Decimal('0')


def test_float_to_decimal_unknown_type():
    assert float_to_decimal([1, 2]) == Decimal('0')


def test_decimal_to_float_none():
    assert decimal_to_float(None) == 0.0


def test_decimal_to_float_valid():
    assert decimal_to_float(Decimal('2.5')) == 2.5


def test_decimal_to_float_invalid_returns_zero():
    # float(Decimal('sNaN')) raises ValueError
    assert decimal_to_float(Decimal('sNaN')) == 0.0


def test_safe_decimal_divide_zero_denominator():
    assert safe_decimal_divide(Decimal('10'), Decimal('0')) == Decimal('0')


def test_safe_decimal_divide_zero_denominator_custom_default():
    assert safe_decimal_divide(Decimal('10'), Decimal('0'), Decimal('-1')) == Decimal('-1')


def test_safe_decimal_divide_normal():
    assert safe_decimal_divide(Decimal('10'), Decimal('4')) == Decimal('2.5')


def test_decimal_import_path_available():
    # The module imports `decimal` (stdlib) and Decimal — both reachable
    assert decimal.InvalidOperation is not None
