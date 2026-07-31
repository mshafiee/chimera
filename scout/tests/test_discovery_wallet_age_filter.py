"""Tests for wallet age filtering during discovery."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import datetime, timedelta, timezone


@pytest.mark.asyncio
async def test_filter_by_wallet_age_removes_young_wallets():
    """Wallets younger than min_age_days should be filtered out."""
    from scout.core.helius_client import HeliusClient

    client = MagicMock(spec=HeliusClient)

    now = datetime.now(timezone.utc)
    # Wallet created 3 days ago — should be filtered if min_age=7
    young_wallet = "YoungWal11111111111111111111111111111111111"
    # Wallet created 30 days ago — should pass
    old_wallet = "OldWall222222222222222222222222222222222222"

    creation_times = {
        young_wallet: (now - timedelta(days=3)).timestamp(),
        old_wallet: (now - timedelta(days=30)).timestamp(),
    }

    client._get_wallet_creation_timestamps_batch = AsyncMock(
        return_value=creation_times
    )

    result = await HeliusClient._filter_by_wallet_age(
        client, [young_wallet, old_wallet], min_age_days=7
    )

    assert young_wallet not in result
    assert old_wallet in result


@pytest.mark.asyncio
async def test_filter_by_wallet_age_disabled_when_zero():
    """When min_age_days=0, all wallets pass (filter disabled)."""
    from scout.core.helius_client import HeliusClient

    client = MagicMock(spec=HeliusClient)
    wallets = [
        "WallA1111111111111111111111111111111111111",
        "WallB2222222222222222222222222222222222222",
    ]

    result = await HeliusClient._filter_by_wallet_age(client, wallets, min_age_days=0)

    assert len(result) == len(wallets)


@pytest.mark.asyncio
async def test_filter_by_wallet_age_handles_missing_creation_time():
    """Wallets with unknown creation time should pass (fail-open)."""
    from scout.core.helius_client import HeliusClient

    client = MagicMock(spec=HeliusClient)

    unknown_wallet = "Unknown111111111111111111111111111111111111"
    old_wallet = "OldWall222222222222222222222222222222222222"
    now = datetime.now(timezone.utc)

    client._get_wallet_creation_timestamps_batch = AsyncMock(
        return_value={
            # unknown_wallet missing — no creation time
            old_wallet: (now - timedelta(days=30)).timestamp(),
        }
    )

    result = await HeliusClient._filter_by_wallet_age(
        client, [unknown_wallet, old_wallet], min_age_days=7
    )

    # Unknown wallet passes (fail-open to avoid blocking new legitimate wallets)
    assert unknown_wallet in result
    assert old_wallet in result
