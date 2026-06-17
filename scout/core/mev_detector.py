"""
Enhanced MEV Protection Detection

Provides utilities for detecting MEV protection usage beyond basic Jito detection:
- Multiple MEV protection services (FlashTrade, BloXroute, etc.)
- Bundle transaction detection
- Heuristic detection for wallets avoiding sandwich-able trades

Usage:
    detector = MEVDetector()
    result = detector.detect_mev_protection(transactions, wallet_address)
"""

import logging
from typing import Dict, List, Optional, Any
from dataclasses import dataclass

logger = logging.getLogger(__name__)


@dataclass
class MEVDetectionResult:
    """Result of MEV protection detection."""
    uses_mev_protection: bool
    protection_service: Optional[str] = None  # "Jito", "Bundle", "Heuristic", etc.
    uses_bundles: bool = False
    uses_limit_orders: bool = False
    complex_ratio: Optional[float] = None  # Ratio of complex swaps
    confidence: float = 0.5  # Confidence in detection


class MEVDetector:
    """
    Enhanced MEV protection detection.

    Detects various forms of MEV protection:
    - Jito tips (transfers to known tip accounts)
    - Bundle transactions
    - MEV protection programs (FlashTrade, BloXroute, etc.)
    - Heuristic detection (complex swap patterns)
    """

    # Jito tip accounts (known routers)
    JITO_TIP_ACCOUNTS = {
        "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU4",
        "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
        "Cw8CFyM9FkoMi7K918YFiz4gBC9MDiSrqwR775XZdTJ5",
        "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt13UZMCSj",
        "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
        "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
        "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
        "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
    }

    # Additional MEV protection service program IDs
    MEV_PROTECTION_PROGRAMS = {
        # FlashTrade
        "FTdRFkEoSvT1gM1YPRdLhcXZcjZzMxfHxVUofVYc",
        # BloXroute MEV protection
        "HjXfpQkvR6G9UNCuPrYpgLESMfD5RHZAZsu4wBTNYhj",
    }

    # Jupiter limit order program
    JUPITER_LIMIT_PROGRAM = "j1o2qRpjcyUwEvwtcfhEQefh773ZgjxcVRry7LDqg5X"

    def detect_mev_protection(
        self,
        transactions: List[Dict[str, Any]],
        wallet_address: Optional[str] = None
    ) -> MEVDetectionResult:
        """
        Detect MEV protection usage from transaction data.

        Args:
            transactions: List of transaction dictionaries
            wallet_address: Optional wallet address for wallet-relative checks

        Returns:
            MEVDetectionResult with detection details
        """
        uses_mev_protection = False
        uses_bundles = False
        uses_limit_orders = False
        protection_service = None
        complex_ratio = None
        confidence = 0.5

        swap_txs = [tx for tx in transactions if tx.get("type") == "SWAP"]
        total_txs = len(transactions)

        # Detect for each transaction
        for tx in transactions:
            # Detect limit orders
            if not uses_limit_orders:
                if tx.get("source") == "JUPITER_LIMIT":
                    uses_limit_orders = True
                else:
                    for ix in tx.get("instructions", []):
                        if ix.get("programId") == self.JUPITER_LIMIT_PROGRAM:
                            uses_limit_orders = True
                            break

            # Detect Jito tips
            if not uses_mev_protection:
                for nt in tx.get("nativeTransfers", []):
                    if nt.get("toUserAccount") in self.JITO_TIP_ACCOUNTS:
                        uses_mev_protection = True
                        protection_service = "Jito"
                        confidence = 0.9  # High confidence for direct tip detection
                        break

            # Detect bundle transactions
            if not uses_bundles:
                if tx.get("type") == "BUNDLE" or "bundle" in tx.get("description", "").lower():
                    uses_bundles = True
                    if not uses_mev_protection:
                        uses_mev_protection = True
                        if protection_service is None:
                            protection_service = "Bundle"
                            confidence = 0.85  # High confidence for bundle type

                # Check for MEV protection programs
                if not uses_mev_protection:
                    for ix in tx.get("instructions", []):
                        prog_id = ix.get("programId", "")
                        if prog_id in self.MEV_PROTECTION_PROGRAMS:
                            uses_mev_protection = True
                            uses_bundles = True
                            if protection_service is None:
                                protection_service = prog_id[:8] + "..."
                                confidence = 0.8  # Medium-high confidence for program ID
                            break

        # Heuristic: wallets that consistently avoid sandwich-able trades
        # If wallet has many trades but low MEV risk score, they likely use protection
        if swap_txs and total_txs > 10:
            # Calculate ratio of swaps that are "complex" (likely protected)
            complex_swaps = sum(1 for tx in swap_txs if len(tx.get("tokenTransfers", [])) > 2)
            complex_ratio = complex_swaps / len(swap_txs) if swap_txs else 0.0

            # High ratio of complex swaps suggests MEV protection usage
            if complex_ratio > 0.6 and not uses_mev_protection:
                uses_mev_protection = True
                if protection_service is None:
                    protection_service = "Heuristic"
                    confidence = 0.6  # Medium confidence for heuristic

        return MEVDetectionResult(
            uses_mev_protection=uses_mev_protection,
            protection_service=protection_service,
            uses_bundles=uses_bundles,
            uses_limit_orders=uses_limit_orders,
            complex_ratio=complex_ratio,
            confidence=confidence,
        )

    def get_mev_protection_summary(self, result: MEVDetectionResult) -> str:
        """
        Get a human-readable summary of MEV detection results.

        Args:
            result: MEVDetectionResult from detect_mev_protection

        Returns:
            Human-readable summary string
        """
        if not result.uses_mev_protection:
            return "No MEV protection detected"

        parts = []
        if result.protection_service:
            parts.append(f"service={result.protection_service}")
        if result.uses_bundles:
            parts.append("bundles")
        if result.uses_limit_orders:
            parts.append("limit_orders")

        summary = "MEV protection: " + ", ".join(parts)
        summary += f" (confidence: {result.confidence:.2f})"

        return summary


def detect_mev_protection(
    transactions: List[Dict[str, Any]],
    wallet_address: Optional[str] = None
) -> MEVDetectionResult:
    """
    Convenience function for MEV protection detection.

    Args:
        transactions: List of transaction dictionaries
        wallet_address: Optional wallet address

    Returns:
        MEVDetectionResult with detection details
    """
    detector = MEVDetector()
    return detector.detect_mev_protection(transactions, wallet_address)
