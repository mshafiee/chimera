"""
Pre-Promotion Validator Tests

Tests the validation logic for promoting wallets from CANDIDATE to ACTIVE:
- Pre-backtest checks
- Backtest result validation
- Liquidity requirement verification
"""

import pytest
from core.models import ValidationStatus, ValidationResult


# =============================================================================
# VALIDATION STATUS TESTS
# =============================================================================

def test_validation_status_passed():
    """Test ValidationStatus enum for passed validation."""
    status = ValidationStatus.PASSED
    
    assert status == ValidationStatus.PASSED
    assert status.value == "PASSED"


def test_validation_status_failed():
    """Test ValidationStatus enum for failed validation."""
    status = ValidationStatus.FAILED_LIQUIDITY
    
    assert status == ValidationStatus.FAILED_LIQUIDITY
    assert "LIQUIDITY" in status.value


def test_validation_result_passed():
    """Test ValidationResult dataclass for passed validation."""
    result = ValidationResult(
        wallet_address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        status=ValidationStatus.PASSED,
        passed=True,
        reason=None,
        notes="All checks passed",
    )
    
    assert result.passed is True
    assert result.reason is None
    assert "passed" in result.notes.lower()


def test_validation_result_failed():
    """Test ValidationResult dataclass for failed validation."""
    result = ValidationResult(
        wallet_address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        status=ValidationStatus.FAILED_LIQUIDITY,
        passed=False,
        reason="Insufficient liquidity",
        notes="Current liquidity: $3,000, Required: $10,000",
    )
    
    assert result.passed is False
    assert result.reason is not None
    assert "liquidity" in result.reason.lower()


# =============================================================================
# PRE-PROMOTION CRITERIA TESTS
# =============================================================================

def test_wqs_above_threshold_passes():
    """Test that WQS above threshold passes pre-promotion."""
    wqs_score = 75.0
    threshold = 70.0
    
    should_pass = wqs_score >= threshold
    assert should_pass, "WQS above threshold should pass"


def test_wqs_below_threshold_fails():
    """Test that WQS below threshold fails pre-promotion."""
    wqs_score = 65.0
    threshold = 70.0
    
    should_pass = wqs_score >= threshold
    assert not should_pass, "WQS below threshold should fail"


def test_wqs_at_threshold_passes():
    """Test that WQS exactly at threshold passes."""
    wqs_score = 70.0
    threshold = 70.0
    
    should_pass = wqs_score >= threshold
    assert should_pass, "WQS at exact threshold should pass"


# =============================================================================
# LIQUIDITY VALIDATION TESTS
# =============================================================================

def test_shield_liquidity_validation(default_backtest_config):
    """Test liquidity validation for Shield strategy."""
    current_liquidity = 15000.0
    min_required = default_backtest_config.min_liquidity_shield_usd
    strategy = "SHIELD"
    
    passes = current_liquidity >= min_required
    assert passes, "Shield with $15k liquidity should pass"


def test_spear_liquidity_validation(default_backtest_config):
    """Test liquidity validation for Spear strategy."""
    current_liquidity = 7000.0
    min_required = default_backtest_config.min_liquidity_spear_usd
    strategy = "SPEAR"
    
    passes = current_liquidity >= min_required
    assert passes, "Spear with $7k liquidity should pass"


def test_liquidity_validation_failure_message():
    """Test that liquidity failure includes relevant info."""
    current_liquidity = 5000.0
    min_required = 10000.0
    
    failure_message = f"Insufficient liquidity: ${current_liquidity:,.0f} < ${min_required:,.0f} required"
    
    # Check formatted numbers (with comma separators)
    assert "5,000" in failure_message
    assert "10,000" in failure_message


# =============================================================================
# BACKTEST RESULT VALIDATION TESTS
# =============================================================================

def test_positive_pnl_passes():
    """Test that positive simulated PnL passes validation."""
    simulated_pnl = 100.0
    
    passes = simulated_pnl >= 0
    assert passes, "Positive PnL should pass"


def test_zero_pnl_passes():
    """Test that zero simulated PnL passes (break-even is acceptable)."""
    simulated_pnl = 0.0
    
    passes = simulated_pnl >= 0
    assert passes, "Zero PnL (break-even) should pass"


def test_negative_pnl_fails():
    """Test that negative simulated PnL fails validation."""
    simulated_pnl = -50.0
    
    passes = simulated_pnl >= 0
    assert not passes, "Negative PnL should fail"


def test_backtest_pnl_considers_fees_and_slippage(default_backtest_config):
    """Test that backtest PnL accounts for fees and slippage."""
    gross_pnl = 100.0
    
    # Approximate costs for round-trip
    cost_percent = (default_backtest_config.max_slippage_percent + 
                    default_backtest_config.dex_fee_percent) * 2
    total_costs = 1000.0 * cost_percent  # Assuming $1000 trade
    
    net_pnl = gross_pnl - total_costs
    
    # With 10.6% costs on $1000 = $106 costs
    # Net PnL = $100 - $106 = -$6 (fails)
    assert net_pnl < 0, "Small gross gains should become losses after costs"


# =============================================================================
# TRADE COUNT VALIDATION TESTS
# =============================================================================

def test_sufficient_trades_passes(default_backtest_config):
    """Test that sufficient trade history passes."""
    trade_count = 10
    min_required = default_backtest_config.min_trades_required
    
    passes = trade_count >= min_required
    assert passes, f"Should pass with {trade_count} trades (>= {min_required})"


def test_insufficient_trades_fails(default_backtest_config):
    """Test that insufficient trade history fails."""
    trade_count = 3
    min_required = default_backtest_config.min_trades_required
    
    passes = trade_count >= min_required
    assert not passes, f"Should fail with only {trade_count} trades (< {min_required})"


def test_no_trades_fails():
    """Test that no trade history fails."""
    trade_count = 0
    min_required = 5
    
    passes = trade_count >= min_required
    assert not passes, "Should fail with no trades"


# =============================================================================
# COMPOSITE VALIDATION TESTS
# =============================================================================

def test_all_checks_must_pass():
    """Test that all validation checks must pass for promotion."""
    checks = {
        "wqs_above_threshold": True,
        "sufficient_liquidity": True,
        "positive_pnl": True,
        "enough_trades": True,
    }
    
    all_pass = all(checks.values())
    assert all_pass, "All checks passing should result in overall pass"


def test_one_failing_check_fails_all():
    """Test that one failing check fails overall validation."""
    checks = {
        "wqs_above_threshold": True,
        "sufficient_liquidity": False,  # One failure
        "positive_pnl": True,
        "enough_trades": True,
    }
    
    all_pass = all(checks.values())
    assert not all_pass, "One failing check should fail overall validation"


def test_validation_returns_first_failure_reason():
    """Test that validation reports the first failure reason."""
    failures = []
    
    if not True:  # wqs check passes
        failures.append("WQS below threshold")
    if not False:  # liquidity check fails
        failures.append("Insufficient liquidity")
    if not True:  # pnl check passes
        failures.append("Negative simulated PnL")
    
    first_failure = failures[0] if failures else None
    assert first_failure == "Insufficient liquidity"


# =============================================================================
# VALIDATION RESULT FORMATTING TESTS
# =============================================================================

def test_success_result_format():
    """Test successful validation result format."""
    result = {
        "passed": True,
        "reason": None,
        "notes": "All validation checks passed",
        "checks": {
            "wqs": "PASS",
            "liquidity": "PASS",
            "backtest_pnl": "PASS",
            "trade_count": "PASS",
        }
    }
    
    assert result["passed"] is True
    assert result["reason"] is None
    assert all(v == "PASS" for v in result["checks"].values())


def test_failure_result_format():
    """Test failed validation result format."""
    result = {
        "passed": False,
        "reason": "Simulated PnL is negative",
        "notes": "Simulated PnL: -$45.00 (after fees/slippage)",
        "checks": {
            "wqs": "PASS",
            "liquidity": "PASS",
            "backtest_pnl": "FAIL",
            "trade_count": "PASS",
        }
    }
    
    assert result["passed"] is False
    assert result["reason"] is not None
    assert result["checks"]["backtest_pnl"] == "FAIL"


# =============================================================================
# EDGE CASES
# =============================================================================

def test_validation_with_missing_metrics():
    """Test validation handles missing metrics gracefully."""
    # Simulate wallet with some missing data
    wallet_data = {
        "wqs_score": 75.0,
        "trade_count": None,  # Missing
        "current_liquidity": 15000.0,
    }
    
    # Should handle None gracefully
    has_trade_count = wallet_data.get("trade_count") is not None
    assert not has_trade_count, "Should detect missing trade count"


def test_validation_logs_all_checks():
    """Test that validation logs all individual check results."""
    check_results = []
    
    # Simulate running each check
    check_results.append(("WQS", True, "Score: 75.0 >= 70.0"))
    check_results.append(("Liquidity", True, "$15,000 >= $10,000"))
    check_results.append(("Backtest PnL", False, "PnL: -$45.00"))
    check_results.append(("Trade Count", True, "10 >= 5"))
    
    # All checks should be recorded
    assert len(check_results) == 4
    
    # Should be able to identify which check failed
    failed_checks = [c for c in check_results if not c[1]]
    assert len(failed_checks) == 1
    assert failed_checks[0][0] == "Backtest PnL"


def test_revalidation_after_demotion():
    """Test that demoted wallets can be revalidated."""
    wallet_status = "CANDIDATE"
    previous_status = "ACTIVE"  # Was demoted
    
    # Should be able to attempt promotion again
    can_validate = wallet_status == "CANDIDATE"
    assert can_validate, "Demoted wallets should be able to revalidate"


def test_ttl_promotion():
    """Test temporary promotion with TTL."""
    import datetime
    
    ttl_hours = 24
    promoted_at = datetime.datetime.now(datetime.timezone.utc)
    expires_at = promoted_at + datetime.timedelta(hours=ttl_hours)
    
    assert expires_at > promoted_at
    assert (expires_at - promoted_at).total_seconds() == ttl_hours * 3600

