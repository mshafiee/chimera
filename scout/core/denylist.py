"""
Known scam/rug wallet cluster denylist for correlation checks.

Wallets that have transacted with known scam addresses are downgraded as
they are likely part of the same ring.
"""

import os
import logging
from typing import Optional, Set

logger = logging.getLogger(__name__)

# Community-reported scam / rug wallet clusters
# Categorized by type: rug_pull, wash_trading, sandwich_bot, phishing
# These are examples — replace with real community-maintained addresses
# from sources like RugCheck, Dune dashboards, or blockchain forensics.
_KNOWN_SCAM_ADDRESSES: Set[str] = set()

# Funders of known scam tokens (PumpFun rug factories, etc.)
_KNOWN_SCAM_FUNDERS: Set[str] = set()


def _load_custom_denylist() -> None:
    """Load additional addresses from a local denylist file.

    Plain lines are wallet addresses; a ``FUNDERS:`` (or ``[FUNDERS]``)
    section header switches the section to funder addresses.
    """
    path = os.getenv("SCOUT_DENYLIST_PATH", "config/denylist.txt")
    if not os.path.exists(path):
        return
    try:
        section = "addresses"
        with open(path) as f:
            for line in f:
                stripped = line.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                if stripped.upper() in ("FUNDERS:", "[FUNDERS]"):
                    section = "funders"
                    continue
                if stripped.upper() in ("ADDRESSES:", "[ADDRESSES]"):
                    section = "addresses"
                    continue
                addr = stripped.split("#")[0].strip()
                if addr and len(addr) >= 32:
                    if section == "funders":
                        _KNOWN_SCAM_FUNDERS.add(addr)
                    else:
                        _KNOWN_SCAM_ADDRESSES.add(addr)
    except Exception as exc:
        logger.warning("Failed to load denylist from %s: %s", path, exc)


_load_custom_denylist()

# Precomputed combined set for O(1) membership tests on the hot path
_ALL_SCAM: frozenset = frozenset(_KNOWN_SCAM_ADDRESSES | _KNOWN_SCAM_FUNDERS)


def is_known_scam_address(address: Optional[str]) -> bool:
    """Return True if the address is in the known scam denylist."""
    if not address:
        return False
    return address in _ALL_SCAM



async def check_wallet_correlation(
    wallet_address: str,
    funder: Optional[str] = None,
    counterparties: Optional[Set[str]] = None,
) -> bool:
    """
    Check if a wallet is correlated with known scam clusters.

    Returns True if the wallet appears to be clean (no correlation found).
    Returns False if the wallet or its funder/counterparties are on the denylist.
    """
    if is_known_scam_address(wallet_address):
        logger.warning("Wallet %s is on the scam denylist", wallet_address[:8])
        return False

    if funder and is_known_scam_address(funder):
        logger.warning(
            "Wallet %s was funded by known scam address %s",
            wallet_address[:8], funder[:8],
        )
        return False

    if counterparties:
        matches = counterparties & _ALL_SCAM
        if matches:
            logger.warning(
                "Wallet %s has %d counterparties on the scam denylist",
                wallet_address[:8], len(matches),
            )
            return False

    return True
