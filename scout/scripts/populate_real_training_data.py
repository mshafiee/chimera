#!/usr/bin/env python3
"""
Populate database with real wallet data from Helius API.

Fetches real trading data from Solana blockchain:
- Discovers active wallets from recent DEX swaps
- Analyzes wallet performance metrics
- Stores real trading patterns in database

Usage:
    python -m scout.scripts.populate_real_training_data --api-key YOUR_KEY --wallets 500
"""

import asyncio
import argparse
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from scout.core.helius_client import HeliusClient
from scout.core.analyzer import WalletAnalyzer
from scout.core.roster_writer_db import WalletRecord, write_wallets_to_db

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)


async def populate_real_wallet_data(
    api_key: str,
    min_wallets: int = 500,
    min_trade_count: int = 5,
    hours_back: int = 168,  # 7 days
    output_db: str = "data/chimera.db"
):
    """
    Fetch and populate real wallet data from Helius.

    Args:
        api_key: Helius API key
        min_wallets: Minimum number of wallets to fetch
        min_trade_count: Minimum trades required per wallet
        hours_back: Hours to look back for discovering wallets
        output_db: Path to output database
    """
    logger.info("Starting real wallet data collection from Solana blockchain")
    logger.info(f"Target: {min_wallets} wallets with {min_trade_count}+ trades")

    # Initialize Helius client
    helius = HeliusClient(api_key=api_key)
    # Initialize WalletAnalyzer
    analyzer = WalletAnalyzer(helius_api_key=api_key)

    # Discover wallets from recent swaps
    logger.info(f"Discovering wallets from last {hours_back} hours...")
    try:
        discovered = await helius.discover_wallets_from_recent_swaps(
            min_trade_count=min_trade_count,
            max_wallets=min_wallets * 2,  # Discover more to filter later
            hours_back=hours_back
        )
    except Exception as e:
        logger.error(f"Failed to discover wallets: {e}")
        logger.error("Check your Helius API key and network connection")
        return 0
    finally:
        await helius.close()

    logger.info(f"Discovered {len(discovered)} unique wallet addresses")

    if len(discovered) == 0:
        logger.error("No wallets discovered. Try increasing hours_back parameter.")
        return 0

    # Initialize WalletAnalyzer
    analyzer = WalletAnalyzer(helius_api_key=api_key)

    wallet_records = []
    skipped = 0
    failed = 0

    logger.info("Analyzing wallets and calculating metrics...")

    for i, wallet_address in enumerate(discovered):
        try:
            # Get wallet metrics
            metrics = await analyzer.get_wallet_metrics(wallet_address)

            if not metrics:
                skipped += 1
                logger.debug(f"  [{wallet_address[:8]}] No metrics returned, skipping")
                continue

            # Filter by minimum trade count
            trade_count = metrics.trade_count_30d if metrics.trade_count_30d else 0
            if trade_count < min_trade_count:
                skipped += 1
                logger.debug(f"  [{wallet_address[:8]}] Only {trade_count} trades, skipping")
                continue

            # Calculate WQS score if not present
            # Import WQS calculator
            from scout.core.wqs import calculate_wqs
            wqs_result = calculate_wqs(metrics, strategy="SHIELD")
            wqs_score = wqs_result.get("score", 50.0)

            # Create wallet record
            record = WalletRecord(
                address=wallet_address,
                status="CANDIDATE",
                wqs_score=float(wqs_score) if wqs_score else 50.0,
                roi_7d=float(metrics.roi_7d) if metrics.roi_7d else 0.0,
                roi_30d=float(metrics.roi_30d) if metrics.roi_30d else 0.0,
                trade_count_30d=int(metrics.trade_count_30d) if metrics.trade_count_30d else 0,
                win_rate=float(metrics.win_rate) if metrics.win_rate else 0.5,
                max_drawdown_30d=float(metrics.max_drawdown_30d) if metrics.max_drawdown_30d else 0.0,
                avg_trade_size_sol=float(metrics.avg_trade_size_sol) if metrics.avg_trade_size_sol else 0.1,
                profit_factor=float(metrics.profit_factor) if metrics.profit_factor else 1.0,
                archetype=str(metrics.archetype) if metrics.archetype else None,
                avg_entry_delay_seconds=float(metrics.avg_entry_delay_seconds) if metrics.avg_entry_delay_seconds else 1.0,
                last_trade_at=str(metrics.last_trade_at) if metrics.last_trade_at else None,
            )

            wallet_records.append(record)

            # Progress update
            if (i + 1) % 25 == 0:
                logger.info(f"Processed {i + 1}/{len(discovered)} wallets, collected {len(wallet_records)} valid records")

        except Exception as e:
            failed += 1
            logger.debug(f"Failed to analyze {wallet_address[:8]}...: {e}")

        # Stop if we have enough wallets
        if len(wallet_records) >= min_wallets:
            logger.info(f"Reached target of {min_wallets} wallets")
            break

    # Cleanup analyzer
    await analyzer.shutdown()

    # Write to database
    if len(wallet_records) == 0:
        logger.error("No valid wallet records collected")
        return 0

    logger.info(f"Writing {len(wallet_records)} real wallet records to database...")
    try:
        success_count = write_wallets_to_db(wallet_records)

        if success_count > 0:
            logger.info(f"✓ Successfully populated {success_count}/{len(wallet_records)} real wallet records")
            logger.info(f"  Skipped: {skipped} (below minimum trade count)")
            logger.info(f"  Failed: {failed} (analysis errors)")

            # Show statistics
            wqs_scores = [r.wqs_score for r in wallet_records if r.wqs_score is not None]
            roi_values = [r.roi_30d for r in wallet_records if r.roi_30d is not None]
            trade_counts = [r.trade_count_30d for r in wallet_records if r.trade_count_30d is not None]

            if wqs_scores:
                logger.info("\nData Statistics:")
                logger.info(f"  WQS Score: {sum(wqs_scores)/len(wqs_scores):.1f} avg (min={min(wqs_scores):.1f}, max={max(wqs_scores):.1f})")
            if roi_values:
                logger.info(f"  ROI 30d: {sum(roi_values)/len(roi_values):.2f} avg (min={min(roi_values):.2f}, max={max(roi_values):.2f})")
            if trade_counts:
                logger.info(f"  Trade Count 30d: {sum(trade_counts)/len(trade_counts):.1f} avg (min={min(trade_counts)}, max={max(trade_counts)})")

            return success_count
        else:
            logger.error("Failed to write to database")
            return 0

    except Exception as e:
        logger.error(f"Database write failed: {e}")
        import traceback
        traceback.print_exc()
        return 0


def main():
    parser = argparse.ArgumentParser(
        description="Populate database with real wallet data from Solana blockchain via Helius API",
        epilog="""
Examples:
    # Collect 500 wallets with 5+ trades from last 7 days
    python -m scout.scripts.populate_real_training_data --api-key YOUR_KEY --wallets 500

    # Collect more wallets with stricter requirements
    python -m scout.scripts.populate_real_training_data --api-key YOUR_KEY --wallets 1000 --min-trades 10

    # Look back further in time (30 days)
    python -m scout.scripts.populate_real_training_data --api-key YOUR_KEY --hours-back 720
        """
    )

    parser.add_argument("--api-key", required=True, help="Helius API key (get free tier at https://www.helius.dev/)")
    parser.add_argument("--wallets", type=int, default=500, help="Number of wallets to collect (default: 500)")
    parser.add_argument("--min-trades", type=int, default=5, help="Minimum trade count per wallet (default: 5)")
    parser.add_argument("--hours-back", type=int, default=168, help="Hours to look back for discovery, default: 168 (7 days)")
    parser.add_argument("--output-db", default="data/chimera.db", help="Output database path (default: data/chimera.db)")
    parser.add_argument("--verbose", action="store_true", help="Enable verbose logging")

    args = parser.parse_args()

    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    try:
        count = asyncio.run(populate_real_wallet_data(
            api_key=args.api_key,
            min_wallets=args.wallets,
            min_trade_count=args.min_trades,
            hours_back=args.hours_back,
            output_db=args.output_db
        ))

        if count > 0:
            print(f"\n✓ Successfully collected {count} real wallet records")
            print(f"  Database: {args.output_db}")
            print(f"  Next: Run 'python -m scout.scripts.train_ml_models --db-path {args.output_db}' to train models")
            sys.exit(0)
        else:
            print("\n✗ Failed to collect wallet data")
            sys.exit(1)

    except KeyboardInterrupt:
        logger.info("Data collection interrupted by user")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Data collection failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
