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
_KNOWN_SCAM_ADDRESSES: Set[str] = {
    # These are examples — replace with real community-maintained addresses
    # from sources like RugCheck, Dune dashboards, or blockchain forensics.
}

# Funders of known scam tokens (PumpFun rug factories, etc.)
_KNOWN_SCAM_FUNDERS: Set[str] = set()


def _load_custom_denylist() -> None:
    """Load additional addresses from a local denylist file."""
    path = os.getenv("SCOUT_DENYLIST_PATH", "config/denylist.txt")
    if not os.path.exists(path):
        return
    try:
        with open(path) as f:
            for line in f:
                addr = line.strip().split("#")[0].strip()
                if addr and len(addr) >= 32:
                    _KNOWN_SCAM_ADDRESSES.add(addr)
    except Exception as exc:
        logger.warning("Failed to load denylist from %s: %s", path, exc)


_load_custom_denylist()


def is_known_scam_address(address: Optional[str]) -> bool:
    """Return True if the address is in the known scam denylist."""
    if not address:
        return False
    return address in _KNOWN_SCAM_ADDRESSES or address in _KNOWN_SCAM_FUNDERS



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
        matches = counterparties & (_KNOWN_SCAM_ADDRESSES | _KNOWN_SCAM_FUNDERS)
        if matches:
            logger.warning(
                "Wallet %s has %d counterparties on the scam denylist",
                wallet_address[:8], len(matches),
            )
            return False

    return True
