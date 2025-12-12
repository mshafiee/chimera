"""Tests for wallet validation and backtesting"""

import pytest
from datetime import datetime, timedelta
from unittest.mock import Mock, MagicMock
from scout.core.validator import PrePromotionValidator, ValidationStatus, PromotionCriteria
from scout.core.wqs import WalletMetrics
from scout.core.models import HistoricalTrade, TradeAction, SimulatedResult, BacktestConfig
from scout.core.liquidity import LiquidityProvider


def test_validator_rejects_low_wqs():
    """Test that validator rejects wallets with WQS below threshold."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(min_wqs_score=60.0)
    )
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=10.0,  # Low ROI
        trade_count_30d=5,
        win_rate=0.5,
    )
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        [],
        strategy="SHIELD"
    )
    
    assert not result.passed
    assert result.status == ValidationStatus.FAILED_WQS
    assert "wqs score" in result.reason.lower()


def test_validator_rejects_insufficient_trades():
    """Test that validator rejects wallets with insufficient trades."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=10,
        )
    )
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,  # Enough for WQS
        win_rate=0.7,
    )
    
    # Only 5 trades (below min_trades=10)
    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
        )
        for i in range(5)
    ]
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )
    
    assert not result.passed
    assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES


def test_validator_rejects_insufficient_closes():
    """Test that validator rejects wallets with insufficient realized closes."""
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_closes_required=10,  # Need 10 SELLs with PnL
        )
    )
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
    )
    
    # Create trades with only 5 SELLs (below min_closes_required=10)
    trades = []
    for i in range(15):
        is_sell = i % 3 == 2  # Every 3rd trade is a SELL
        trades.append(HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if is_sell else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if is_sell else None,  # Only SELLs have PnL
        ))
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )
    
    assert not result.passed
    assert result.status == ValidationStatus.FAILED_INSUFFICIENT_TRADES
    assert "closes" in result.reason.lower()


def test_validator_rejects_negative_simulated_pnl():
    """Test that validator rejects wallets with negative simulated PnL."""
    # Mock backtester to return negative PnL
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=10,
        simulated_trades=10,
        rejected_trades=0,
        original_pnl_sol=5.0,
        simulated_pnl_sol=-1.0,  # Negative!
        pnl_difference_sol=6.0,
        total_slippage_cost_sol=2.0,
        total_fee_cost_sol=1.0,
        passed=False,
        failure_reason="Negative simulated PnL",
    )
    
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_closes_required=5,
        )
    )
    validator.simulator = mock_simulator
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
    )
    
    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(10)
    ]
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )
    
    assert not result.passed
    assert result.status == ValidationStatus.FAILED_NEGATIVE_PNL


def test_validator_rejects_high_rejection_rate():
    """Test that validator rejects wallets with too many rejected trades."""
    # Mock backtester to return high rejection rate
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=10,
        simulated_trades=4,  # Only 4 out of 10 executed
        rejected_trades=6,  # 60% rejected
        original_pnl_sol=5.0,
        simulated_pnl_sol=2.0,
        pnl_difference_sol=3.0,
        total_slippage_cost_sol=1.0,
        total_fee_cost_sol=0.5,
        passed=False,
        failure_reason="Too many trades rejected",
    )
    
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_closes_required=5,
            max_rejection_rate=0.5,  # Max 50% rejection
        )
    )
    validator.simulator = mock_simulator
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
    )
    
    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(10)
    ]
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )
    
    assert not result.passed
    assert result.status == ValidationStatus.FAILED_LIQUIDITY  # High rejection usually means liquidity issues


def test_validator_passes_good_wallet():
    """Test that validator accepts wallets that pass all checks."""
    # Mock backtester to return positive result
    mock_simulator = Mock()
    mock_simulator.simulate_wallet.return_value = SimulatedResult(
        wallet_address="test_wallet",
        total_trades=15,
        simulated_trades=15,
        rejected_trades=0,
        original_pnl_sol=10.0,
        simulated_pnl_sol=8.0,  # Positive, acceptable reduction
        pnl_difference_sol=2.0,
        total_slippage_cost_sol=1.0,
        total_fee_cost_sol=1.0,
        passed=True,
        failure_reason=None,
    )
    
    validator = PrePromotionValidator(
        promotion_criteria=PromotionCriteria(
            min_wqs_score=30.0,
            min_trades=5,
            min_closes_required=5,
        )
    )
    validator.simulator = mock_simulator
    
    metrics = WalletMetrics(
        address="test_wallet",
        roi_30d=50.0,
        trade_count_30d=20,
        win_rate=0.7,
    )
    
    trades = [
        HistoricalTrade(
            token_address="token1",
            token_symbol="TOKEN1",
            action=TradeAction.SELL if i % 2 == 1 else TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.001,
            timestamp=datetime.utcnow() - timedelta(days=i),
            tx_signature=f"tx{i}",
            pnl_sol=0.1 if i % 2 == 1 else None,
        )
        for i in range(15)
    ]
    
    result = validator.validate_for_promotion(
        "test_wallet",
        metrics,
        trades,
        strategy="SHIELD"
    )
    
    assert result.passed
    assert result.status == ValidationStatus.PASSED
    assert result.recommended_status == "ACTIVE"
