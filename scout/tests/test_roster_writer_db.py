"""Tests for roster_writer_db module - direct database writes."""

import pytest
from unittest.mock import patch

# Import the module to test
from core.roster_writer_db import (
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

    @patch("core.roster_writer_db.execute_update")
    def test_write_wallet_success(self, mock_exec_update, sample_wallet):
        """Test successful wallet write."""
        mock_exec_update.return_value = 1

        result = write_wallet_to_db(sample_wallet)

        assert result is True
        assert mock_exec_update.call_count >= 1
        # First call must be the wallet upsert with the wallet's own data
        insert_query, insert_params = mock_exec_update.call_args_list[0][0]
        assert "INSERT INTO wallets" in insert_query
        assert insert_params[0] == sample_wallet.address
        assert insert_params[1] == "ACTIVE"
        assert insert_params[2] == 85.5

    @patch("core.roster_writer_db.execute_update")
    def test_write_wallet_database_error(self, mock_exec_update, sample_wallet):
        """Test wallet write with database error."""
        mock_exec_update.side_effect = Exception("Database error")

        result = write_wallet_to_db(sample_wallet)

        assert result is False


class TestWriteWalletsToDB:
    """Test write_wallets_to_db function."""

    @patch("core.roster_writer_db.write_wallet_to_db")
    def test_write_multiple_wallets_success(self, mock_write_wallet, sample_wallet):
        """Test writing multiple wallets successfully."""
        # Mock individual writes to succeed
        mock_write_wallet.return_value = True

        wallets = [sample_wallet, sample_wallet, sample_wallet]
        result = write_wallets_to_db(wallets)

        assert result == 3
        assert mock_write_wallet.call_count == 3

    @patch("core.roster_writer_db.write_wallet_to_db")
    def test_write_multiple_wallets_partial_failure(self, mock_write_wallet, sample_wallet):
        """Test writing multiple wallets with some failures."""
        # Mock first 2 writes to succeed, last to fail
        mock_write_wallet.side_effect = [True, True, False]

        wallets = [sample_wallet, sample_wallet, sample_wallet]
        result = write_wallets_to_db(wallets)

        assert result == 2

    @patch("core.roster_writer_db.write_wallet_to_db")
    def test_write_empty_list(self, mock_write_wallet):
        """Test writing an empty list returns 0 without calling the writer."""
        result = write_wallets_to_db([])

        assert result == 0
        mock_write_wallet.assert_not_called()


class TestUpdateWalletStatus:
    """Test update_wallet_status function."""

    @patch("core.roster_writer_db.execute_update")
    def test_update_status_success(self, mock_exec_update):
        """Test successful status update."""
        mock_exec_update.return_value = 1

        result = update_wallet_status(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "CANDIDATE"
        )

        assert result is True
        mock_exec_update.assert_called_once()
        query, params = mock_exec_update.call_args[0]
        assert "UPDATE wallets" in query
        assert params[0] == "CANDIDATE"

    @patch("core.roster_writer_db.execute_update")
    def test_update_status_database_error(self, mock_exec_update):
        """Test status update with database error."""
        mock_exec_update.side_effect = Exception("Database error")

        result = update_wallet_status(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            "CANDIDATE"
        )

        assert result is False


class TestDeleteWallet:
    """Test delete_wallet function."""

    @patch("core.roster_writer_db.execute_update")
    def test_delete_wallet_success(self, mock_exec_update):
        """Test successful wallet deletion."""
        mock_exec_update.return_value = 1

        result = delete_wallet("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")

        assert result is True
        mock_exec_update.assert_called_once()
        query, params = mock_exec_update.call_args[0]
        assert "DELETE FROM wallets" in query
        assert params == ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",)

    @patch("core.roster_writer_db.execute_update")
    def test_delete_wallet_zero_rows_still_success(self, mock_exec_update):
        """Test deleting a nonexistent wallet still returns True (no exception)."""
        mock_exec_update.return_value = 0

        result = delete_wallet("nonexistent_wallet_address")

        assert result is True
