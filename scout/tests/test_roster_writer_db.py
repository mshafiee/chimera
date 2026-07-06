"""Tests for roster_writer_db module - direct database writes."""

import pytest
import tempfile
import os
from pathlib import Path
from unittest.mock import Mock, patch, MagicMock

# Import the module to test
from scout.core.roster_writer_db import (
    WalletRecord,
    write_wallet_to_db,
    write_wallets_to_db,
    update_wallet_status,
    delete_wallet,
)


@pytest.fixture
def sample_wallet():
    """Create a sample wallet record for testing."""
    return WalletRecord(
        address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        status="ACTIVE",
        wqs_score=85.5,
        wqs_confidence=0.85,
        roi_7d=12.5,
        roi_30d=25.8,
        trade_count_30d=50,
        win_rate=0.65,
        max_drawdown_30d=0.15,
        avg_trade_size_sol=1.5,
        avg_win_sol=0.8,
        avg_loss_sol=0.5,
        profit_factor=1.6,
        realized_pnl_30d_sol=12.5,
        last_trade_at="2024-01-01T12:00:00Z",
        promoted_at="2024-01-01T10:00:00Z",
        ttl_expires_at="2024-02-01T10:00:00Z",
        notes="Test wallet",
        archetype="SWING",
        avg_entry_delay_seconds=0.5,
    )


class TestWriteWalletToDB:
    """Test write_wallet_to_db function."""

    @patch("scout.core.roster_writer_db.Connection")
    @patch("scout.core.roster_writer_db.execute_query")
    @patch("scout.core.roster_writer_db.execute_update")
    def test_write_wallet_success(self, mock_exec_update, mock_exec_query, mock_connection_class, sample_wallet):
        """Test successful wallet write."""
        # Mock connection and cursor
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 1
        
        # Configure the context manager
        mock_connection_class.return_value.__enter__.return_value = mock_conn
        mock_connection_class.return_value.__exit__.return_value = None
        
        # Mock execute functions
        mock_exec_update.return_value = 1
        
        result = write_wallet_to_db(sample_wallet)
        
        assert result is True
        mock_exec_update.assert_called_once()

    @patch("scout.core.roster_writer_db.Connection")
    def test_write_wallet_database_error(self, mock_connection_class, sample_wallet):
        """Test wallet write with database error."""
        # Mock connection to raise exception
        mock_connection_class.return_value.__enter__.side_effect = Exception("Database error")
        
        result = write_wallet_to_db(sample_wallet)
        
        assert result is False


class TestWriteWalletsToDB:
    """Test write_wallets_to_db function."""

    @patch("scout.core.roster_writer_db.write_wallet_to_db")
    def test_write_multiple_wallets_success(self, mock_write_wallet, sample_wallet):
        """Test writing multiple wallets successfully."""
        # Mock individual writes to succeed
        mock_write_wallet.return_value = True
        
        wallets = [sample_wallet, sample_wallet, sample_wallet]
        result = write_wallets_to_db(wallets)
        
        assert result == 3
        assert mock_write_wallet.call_count == 3

    @patch("scout.core.roster_writer_db.write_wallet_to_db")
    def test_write_multiple_wallets_partial_failure(self, mock_write_wallet, sample_wallet):
        """Test writing multiple wallets with some failures."""
        # Mock first 2 writes to succeed, last to fail
        mock_write_wallet.side_effect = [True, True, False]
        
        wallets = [sample_wallet, sample_wallet, sample_wallet]
        result = write_wallets_to_db(wallets)
        
        assert result == 2


class TestUpdateWalletStatus:
    """Test update_wallet_status function."""

    @patch("scout.core.roster_writer_db.Connection")
    @patch("scout.core.roster_writer_db.execute_update")
    def test_update_status_success(self, mock_exec_update, mock_connection_class):
        """Test successful status update."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 1
        
        mock_connection_class.return_value.__enter__.return_value = mock_conn
        mock_connection_class.return_value.__exit__.return_value = None
        
        mock_exec_update.return_value = 1
        
        result = update_wallet_status(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "CANDIDATE"
        )
        
        assert result is True
        mock_exec_update.assert_called_once()

    @patch("scout.core.roster_writer_db.Connection")
    def test_update_status_database_error(self, mock_connection_class):
        """Test status update with database error."""
        mock_connection_class.return_value.__enter__.side_effect = Exception("Database error")
        
        result = update_wallet_status(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "CANDIDATE"
        )
        
        assert result is False


class TestDeleteWallet:
    """Test delete_wallet function."""

    @patch("scout.core.roster_writer_db.Connection")
    @patch("scout.core.roster_writer_db.execute_update")
    def test_delete_wallet_success(self, mock_exec_update, mock_connection_class):
        """Test successful wallet deletion."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 1
        
        mock_connection_class.return_value.__enter__.return_value = mock_conn
        mock_connection_class.return_value.__exit__.return_value = None
        
        mock_exec_update.return_value = 1
        
        result = delete_wallet("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")
        
        assert result is True
        mock_exec_update.assert_called_once()