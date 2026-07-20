"""
Direct database writer for Scout roster output.

Writes analyzed wallets directly to the shared database (PostgreSQL in production,
SQLite locally) using ON CONFLICT upserts. Replaces the atomic file-based
RosterWriter approach.

Pattern:
1. Analyze wallets and collect metrics
2. Upsert directly to wallets table using ON CONFLICT DO UPDATE
3. No intermediate files or HTTP merge API calls
"""

import logging
from decimal import Decimal
from dataclasses import dataclass
from typing import List, Optional
from .db import execute_update
from .utils import utcnow

logger = logging.getLogger(__name__)


@dataclass
class WalletRecord:
    """Wallet data for direct database insertion."""
    address: str
    status: str  # 'ACTIVE', 'CANDIDATE', 'REJECTED'
    wqs_score: Optional[float] = None
    wqs_confidence: Optional[float] = None
    roi_7d: Optional[float] = None
    roi_30d: Optional[float] = None
    trade_count_30d: Optional[int] = None
    win_rate: Optional[float] = None
    max_drawdown_30d: Optional[float] = None
    avg_trade_size_sol: Optional[Decimal] = None
    avg_win_sol: Optional[Decimal] = None
    avg_loss_sol: Optional[Decimal] = None
    profit_factor: Optional[float] = None
    realized_pnl_30d_sol: Optional[Decimal] = None
    last_trade_at: Optional[str] = None
    promoted_at: Optional[str] = None
    ttl_expires_at: Optional[str] = None
    notes: Optional[str] = None
    archetype: Optional[str] = None
    avg_entry_delay_seconds: Optional[float] = None


def write_wallet_to_db(wallet: WalletRecord) -> bool:
    """
    Write a single wallet record to the database using upsert.
    
    Args:
        wallet: WalletRecord to write
        
    Returns:
        True if successful, False otherwise
    """
    try:
        # PostgreSQL upsert query
        query = """
            INSERT INTO wallets (
                address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate,
                max_drawdown_30d, avg_trade_size_sol, avg_win_sol, avg_loss_sol,
                profit_factor, realized_pnl_30d_sol, last_trade_at,
                promoted_at, ttl_expires_at, notes, archetype,
                avg_entry_delay_seconds, last_arb_check_at
            ) VALUES (
                %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s
            )
            ON CONFLICT (address) DO UPDATE SET
                status = EXCLUDED.status,
                wqs_score = EXCLUDED.wqs_score,
                wqs_confidence = EXCLUDED.wqs_confidence,
                roi_7d = EXCLUDED.roi_7d,
                roi_30d = EXCLUDED.roi_30d,
                trade_count_30d = EXCLUDED.trade_count_30d,
                win_rate = EXCLUDED.win_rate,
                max_drawdown_30d = EXCLUDED.max_drawdown_30d,
                avg_trade_size_sol = EXCLUDED.avg_trade_size_sol,
                avg_win_sol = EXCLUDED.avg_win_sol,
                avg_loss_sol = EXCLUDED.avg_loss_sol,
                profit_factor = EXCLUDED.profit_factor,
                realized_pnl_30d_sol = EXCLUDED.realized_pnl_30d_sol,
                last_trade_at = EXCLUDED.last_trade_at,
                promoted_at = COALESCE(EXCLUDED.promoted_at, wallets.promoted_at),
                ttl_expires_at = EXCLUDED.ttl_expires_at,
                notes = EXCLUDED.notes,
                archetype = EXCLUDED.archetype,
                avg_entry_delay_seconds = EXCLUDED.avg_entry_delay_seconds,
                last_arb_check_at = CASE
                    WHEN EXCLUDED.archetype = 'ARBITRAGE' OR EXCLUDED.archetype != COALESCE(wallets.archetype, '')
                    THEN CURRENT_TIMESTAMP
                    ELSE COALESCE(wallets.last_arb_check_at, EXCLUDED.last_arb_check_at)
                END,
                updated_at = CURRENT_TIMESTAMP
            WHERE wallets.address = EXCLUDED.address
        """

        params = (
            wallet.address,
            wallet.status,
            wallet.wqs_score,
            wallet.wqs_confidence,
            wallet.roi_7d,
            wallet.roi_30d,
            wallet.trade_count_30d,
            wallet.win_rate,
            wallet.max_drawdown_30d,
            wallet.avg_trade_size_sol,
            wallet.avg_win_sol,
            wallet.avg_loss_sol,
            wallet.profit_factor,
            wallet.realized_pnl_30d_sol,
            wallet.last_trade_at,
            wallet.promoted_at,
            wallet.ttl_expires_at,
            wallet.notes,
            wallet.archetype,
            wallet.avg_entry_delay_seconds,
            utcnow().isoformat() if wallet.archetype == 'ARBITRAGE' else None,
        )

        execute_update(query, params)
        logger.debug(f"Wrote wallet {wallet.address} to database")
        return True

    except Exception as e:
        logger.error(f"Failed to write wallet {wallet.address} to database: {e}")
        return False


def write_wallets_to_db(wallets: List[WalletRecord]) -> int:
    """
    Write multiple wallet records to the database using batch upserts.
    
    Args:
        wallets: List of WalletRecord to write
        
    Returns:
        Number of successfully written wallets
    """
    success_count = 0
    
    for wallet in wallets:
        if write_wallet_to_db(wallet):
            success_count += 1
    
    logger.info(f"Wrote {success_count}/{len(wallets)} wallets to database")
    return success_count


def update_wallet_status(address: str, status: str) -> bool:
    """
    Update wallet status in the database.
    
    Args:
        address: Wallet address
        status: New status ('ACTIVE', 'CANDIDATE', 'REJECTED')
        
    Returns:
        True if successful, False otherwise
    """
    try:
        query = """
            UPDATE wallets
            SET status = %s, updated_at = CURRENT_TIMESTAMP
            WHERE address = %s
        """
        execute_update(query, (status, address))
        logger.debug(f"Updated wallet {address} status to {status}")
        return True

    except Exception as e:
        logger.error(f"Failed to update wallet {address} status: {e}")
        return False


def delete_wallet(address: str) -> bool:
    """
    Delete a wallet from the database.
    
    Args:
        address: Wallet address to delete
        
    Returns:
        True if successful, False otherwise
    """
    try:
        query = "DELETE FROM wallets WHERE address = %s"
        execute_update(query, (address,))
        logger.debug(f"Deleted wallet {address} from database")
        return True

    except Exception as e:
        logger.error(f"Failed to delete wallet {address}: {e}")
        return False


def get_wallet(address: str) -> Optional[dict]:
    """
    Get a wallet record from the database.
    
    Args:
        address: Wallet address
        
    Returns:
        Wallet dict or None if not found
    """
    try:
        from .db import execute_and_fetchone
        
        query = "SELECT * FROM wallets WHERE address = %s"
        wallet = execute_and_fetchone(query, (address,))
        return wallet
        
    except Exception as e:
        logger.error(f"Failed to get wallet {address}: {e}")
        return None


def get_wallets_by_status(status: str) -> List[dict]:
    """
    Get all wallets with a specific status.
    
    Args:
        status: Status filter ('ACTIVE', 'CANDIDATE', 'REJECTED')
        
    Returns:
        List of wallet dicts
    """
    try:
        from .db import execute_and_fetchall
        
        query = "SELECT * FROM wallets WHERE status = %s ORDER BY wqs_score DESC"
        wallets = execute_and_fetchall(query, (status,))
        return wallets
        
    except Exception as e:
        logger.error(f"Failed to get wallets with status {status}: {e}")
        return []