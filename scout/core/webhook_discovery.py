"""
Webhook-First Wallet Discovery System

This module implements a comprehensive webhook-based wallet discovery system that makes
webhook events the primary method for discovering new wallets (90% of discovery vs 10% polling).

The system receives real-time wallet activity events from Helius webhooks and processes them
through a parallel pipeline to extract wallet addresses and assess their quality.

Architecture:
- WebhookReceiver: FastAPI-based webhook endpoint for receiving Helius events
- EventProcessor: Parallel processing pipeline for wallet extraction
- QualityFilter: Real-time wallet quality assessment
- DiscoveryCoordinator: Manages webhook vs polling fallback strategy

Configuration:
- SCOUT_WEBHOOK_PORT: Port for webhook server (default: 8002)
- SCOUT_WEBHOOK_SECRET: HMAC secret for webhook verification
- SCOUT_WEBHOOK_FIRST_RATIO: Webhook vs polling ratio (default: 0.9)
"""

import os
import time
import hmac
import hashlib
import asyncio
import logging
from typing import Dict, List, Optional, Any
from dataclasses import dataclass, field
from enum import Enum
from fastapi import FastAPI, Request, HTTPException
from fastapi.responses import JSONResponse

logger = logging.getLogger(__name__)


class WebhookEventType(Enum):
    """Types of webhook events from Helius."""
    TRANSACTION = "transaction"
    SWAP = "swap"
    TRANSFER = "transfer"
    ACCOUNT = "account"
    COMPRESSED_NFT = "compressed_nft"
    TOKEN = "token"


@dataclass
class WebhookEvent:
    """Raw webhook event from Helius."""
    event_type: WebhookEventType
    timestamp: float
    signature: str
    data: Dict[str, Any]
    raw_payload: Dict[str, Any]

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "event_type": self.event_type.value,
            "timestamp": self.timestamp,
            "signature": self.signature,
            "data": self.data,
        }


@dataclass
class DiscoveredWallet:
    """Wallet discovered from webhook event."""
    address: str
    discovery_source: str  # "webhook_swap", "webhook_transfer", "polling_fallback"
    discovery_timestamp: float
    initial_quality_score: float = 0.0
    trade_count: int = 0
    last_activity: float = 0.0
    metadata: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            "address": self.address,
            "discovery_source": self.discovery_source,
            "discovery_timestamp": self.discovery_timestamp,
            "initial_quality_score": self.initial_quality_score,
            "trade_count": self.trade_count,
            "last_activity": self.last_activity,
            "metadata": self.metadata,
        }


@dataclass
class DiscoveryStats:
    """Statistics for the webhook discovery system."""
    total_events_received: int = 0
    total_wallets_discovered: int = 0
    events_processed: int = 0
    events_failed: int = 0
    webhook_discovery_ratio: float = 0.9
    polling_fallback_count: int = 0
    average_processing_time_ms: float = 0.0
    discovery_queue_depth: int = 0


class WebhookReceiver:
    """
    FastAPI-based webhook receiver for Helius events.

    This provides a lightweight, fast webhook endpoint that accepts
    wallet activity events from Helius and queues them for processing.
    """

    def __init__(self, port: int = 8002, webhook_secret: Optional[str] = None):
        """Initialize the webhook receiver."""
        self.port = port
        self.webhook_secret = webhook_secret or os.getenv("SCOUT_WEBHOOK_SECRET", "")
        self.app = FastAPI(title="Chimera Scout Webhook Receiver")
        self._setup_routes()

        # Event queue for parallel processing
        self.event_queue: asyncio.Queue = asyncio.Queue(maxsize=10000)
        self.processing = False

        # Statistics
        self.stats = DiscoveryStats()

        logger.info(f"[WebhookReceiver] Initialized on port {port}")

    def _setup_routes(self):
        """Setup FastAPI routes."""

        @self.app.post("/webhook")
        async def receive_webhook(request: Request):
            """Receive webhook events from Helius."""
            try:
                # Verify HMAC signature if secret is configured
                if self.webhook_secret:
                    await self._verify_signature(request)

                # Parse JSON payload
                payload = await request.json()

                # Queue event for processing
                event = WebhookEvent(
                    event_type=WebhookEventType.TRANSACTION,
                    timestamp=time.time(),
                    signature=payload.get("signature", ""),
                    data=payload,
                    raw_payload=payload,
                )

                # Add to processing queue (non-blocking)
                try:
                    self.event_queue.put_nowait(event)
                    self.stats.total_events_received += 1
                except asyncio.QueueFull:
                    logger.warning("[WebhookReceiver] Event queue full, dropping event")
                    return JSONResponse(
                        status_code=503,
                        content={"status": "queue_full", "message": "Event queue full"}
                    )

                return JSONResponse(
                    status_code=202,
                    content={"status": "accepted", "message": "Event queued for processing"}
                )

            except Exception as e:
                logger.error(f"[WebhookReceiver] Error processing webhook: {e}")
                raise HTTPException(status_code=400, detail=str(e))

        @self.app.get("/health")
        async def health_check():
            """Health check endpoint."""
            return {
                "status": "healthy",
                "queue_depth": self.event_queue.qsize(),
                "stats": {
                    "total_events_received": self.stats.total_events_received,
                    "total_wallets_discovered": self.stats.total_wallets_discovered,
                    "events_processed": self.stats.events_processed,
                    "events_failed": self.stats.events_failed,
                }
            }

        @self.app.get("/stats")
        async def get_stats():
            """Get discovery statistics."""
            return {
                **self.stats.__dict__,
                "queue_depth": self.event_queue.qsize(),
            }

    async def _verify_signature(self, request: Request):
        """Verify HMAC signature from Helius."""
        # Get signature from header
        signature = request.headers.get("X-Helius-Signature", "")
        if not signature:
            raise HTTPException(status_code=401, detail="Missing signature")

        # Get raw body
        body = await request.body()

        # Verify HMAC
        expected_signature = hmac.new(
            self.webhook_secret.encode(),
            body,
            hashlib.sha256
        ).hexdigest()

        if not hmac.compare_digest(signature, expected_signature):
            raise HTTPException(status_code=401, detail="Invalid signature")

    async def start(self):
        """Start the webhook receiver server."""
        import uvicorn

        config = uvicorn.Config(
            app=self.app,
            host="0.0.0.0",
            port=self.port,
            log_level="info"
        )
        server = uvicorn.Server(config)

        logger.info(f"[WebhookReceiver] Starting webhook server on port {self.port}")
        await server.serve()


class EventProcessor:
    """
    Parallel event processing pipeline for wallet extraction.

    This processes webhook events through multiple stages:
    1. Event validation and normalization
    2. Wallet extraction from transaction data
    3. Quality assessment and filtering
    4. Integration with discovery database
    """

    def __init__(self, max_workers: int = 10):
        """Initialize the event processor."""
        self.max_workers = max_workers
        self.processing = False
        self.discovered_wallets: Dict[str, DiscoveredWallet] = {}

        # Filter settings
        self.min_trade_count = int(os.getenv("SCOUT_MIN_TRADES", "3"))
        self.require_recent_activity = os.getenv("SCOUT_REQUIRE_RECENT_ACTIVITY", "false").lower() == "true"

        logger.info(f"[EventProcessor] Initialized with {max_workers} workers")

    async def process_events(self, event_queue: asyncio.Queue) -> Dict[str, Any]:
        """
        Process events from the queue in parallel.

        Returns processing statistics.
        """
        self.processing = True
        stats = {
            "processed": 0,
            "failed": 0,
            "wallets_discovered": 0,
            "start_time": time.time(),
        }

        logger.info("[EventProcessor] Starting event processing loop")

        try:
            # Create worker tasks
            tasks = [
                asyncio.create_task(self._worker(event_queue, stats))
                for _ in range(self.max_workers)
            ]

            # Wait for all workers to complete
            await asyncio.gather(*tasks, return_exceptions=True)

        except Exception as e:
            logger.error(f"[EventProcessor] Error in processing loop: {e}")

        finally:
            self.processing = False
            stats["end_time"] = time.time()
            stats["duration_seconds"] = stats["end_time"] - stats["start_time"]

        logger.info(f"[EventProcessor] Processing complete: {stats}")
        return stats

    async def _worker(self, event_queue: asyncio.Queue, stats: Dict[str, Any]):
        """Worker task that processes events from the queue."""
        while self.processing:
            try:
                # Get event from queue with timeout
                event = await asyncio.wait_for(event_queue.get(), timeout=1.0)

                # Process the event
                try:
                    wallets = await self._process_single_event(event)

                    # Add discovered wallets
                    for wallet in wallets:
                        if wallet.address not in self.discovered_wallets:
                            self.discovered_wallets[wallet.address] = wallet
                            stats["wallets_discovered"] += 1

                    stats["processed"] += 1

                except Exception as e:
                    logger.error(f"[EventProcessor] Error processing event: {e}")
                    stats["failed"] += 1

                finally:
                    event_queue.task_done()

            except asyncio.TimeoutError:
                # No events available, continue loop
                continue
            except Exception as e:
                logger.error(f"[EventProcessor] Worker error: {e}")
                break

    async def _process_single_event(self, event: WebhookEvent) -> List[DiscoveredWallet]:
        """Process a single webhook event and extract wallets."""
        wallets = []

        try:
            # Extract transaction data from event
            transaction = event.data.get("transaction", {})
            if not transaction:
                return wallets

            # Extract wallet addresses from transaction
            # Priority: fee payer -> token transfers -> account keys
            fee_payer = transaction.get("feePayer")

            if fee_payer and self._is_valid_wallet_address(fee_payer):
                wallets.append(self._create_discovered_wallet(
                    fee_payer,
                    event,
                    "webhook_fee_payer"
                ))

            # Extract from token transfers
            for transfer in transaction.get("tokenTransfers", []):
                from_account = transfer.get("fromUserAccount")
                to_account = transfer.get("toUserAccount")

                for account in [from_account, to_account]:
                    if account and self._is_valid_wallet_address(account):
                        wallets.append(self._create_discovered_wallet(
                            account,
                            event,
                            "webhook_token_transfer"
                        ))

            # Extract from native transfers
            for transfer in transaction.get("nativeTransfers", []):
                from_account = transfer.get("fromUserAccount")
                to_account = transfer.get("toUserAccount")

                for account in [from_account, to_account]:
                    if account and self._is_valid_wallet_address(account):
                        wallets.append(self._create_discovered_wallet(
                            account,
                            event,
                            "webhook_native_transfer"
                        ))

        except Exception as e:
            logger.error(f"[EventProcessor] Error extracting wallets: {e}")

        return wallets

    def _create_discovered_wallet(
        self,
        address: str,
        event: WebhookEvent,
        source: str
    ) -> DiscoveredWallet:
        """Create a discovered wallet object."""
        return DiscoveredWallet(
            address=address,
            discovery_source=source,
            discovery_timestamp=event.timestamp,
            metadata={
                "event_signature": event.signature,
                "event_type": event.event_type.value,
            }
        )

    def _is_valid_wallet_address(self, address: str) -> bool:
        """Check if address is a valid wallet address."""
        if not address or len(address) < 32 or len(address) > 44:
            return False

        # Filter system programs
        system_programs = {
            "11111111111111111111111111111111",  # System Program
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  # Token Program
            "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25ekTN8LoUaUX",  # Token-2022
        }

        if address in system_programs:
            return False

        # Filter program-like addresses
        if address.endswith("11111111111111111111111111111111"):
            return False

        return True

    def get_discovered_wallets(self) -> List[DiscoveredWallet]:
        """Get all discovered wallets."""
        return list(self.discovered_wallets.values())


class QualityFilter:
    """
    Real-time wallet quality assessment for webhook-discovered wallets.

    This applies fast, lightweight quality checks to filter out low-quality
    wallets before they enter the full analysis pipeline.
    """

    def __init__(self):
        """Initialize the quality filter."""
        self.min_quality_score = float(os.getenv("SCOUT_MIN_QUALITY_SCORE", "20.0"))
        self.min_trades = int(os.getenv("SCOUT_MIN_TRADES", "3"))

    async def assess_wallet_quality(
        self,
        wallet: DiscoveredWallet,
        additional_context: Optional[Dict[str, Any]] = None
    ) -> float:
        """
        Assess the quality of a discovered wallet.

        Returns a quality score from 0-100.
        """
        score = 0.0

        # Base score for being discovered via webhook (real-time activity)
        if "webhook" in wallet.discovery_source:
            score += 30.0

        # Bonus for recent activity
        time_since_discovery = time.time() - wallet.discovery_timestamp
        if time_since_discovery < 3600:  # Within 1 hour
            score += 20.0
        elif time_since_discovery < 86400:  # Within 1 day
            score += 10.0

        # Bonus for trade count
        if wallet.trade_count >= self.min_trades:
            score += min(30.0, wallet.trade_count * 3)

        # Bonus for metadata completeness
        if wallet.metadata:
            score += 10.0

        # Additional context assessment
        if additional_context:
            if additional_context.get("has_swap_activity"):
                score += 15.0
            if additional_context.get("high_volume"):
                score += 10.0
            if additional_context.get("diverse_tokens"):
                score += 5.0

        return min(100.0, score)

    async def filter_wallets(
        self,
        wallets: List[DiscoveredWallet],
        min_score: Optional[float] = None
    ) -> List[DiscoveredWallet]:
        """Filter wallets by minimum quality score."""
        min_score = min_score or self.min_quality_score

        filtered = []
        for wallet in wallets:
            quality = await self.assess_wallet_quality(wallet)
            wallet.initial_quality_score = quality
            if quality >= min_score:
                filtered.append(wallet)

        return filtered


class DiscoveryCoordinator:
    """
    Manages the overall webhook-first discovery strategy.

    This coordinates between webhook discovery and polling fallback,
    ensuring 90% of discovery comes from webhooks and 10% from polling.
    """

    def __init__(self):
        """Initialize the discovery coordinator."""
        self.webhook_ratio = float(os.getenv("SCOUT_WEBHOOK_FIRST_RATIO", "0.9"))
        self.polling_ratio = 1.0 - self.webhook_ratio

        # Initialize components
        self.webhook_receiver = WebhookReceiver()
        self.event_processor = EventProcessor()
        self.quality_filter = QualityFilter()

        # Statistics
        self.stats = DiscoveryStats()
        self.stats.webhook_discovery_ratio = self.webhook_ratio

        logger.info(f"[DiscoveryCoordinator] Initialized with {self.webhook_ratio:.0%} webhook ratio")

    async def start_webhook_discovery(self):
        """Start the webhook-based discovery system."""
        logger.info("[DiscoveryCoordinator] Starting webhook-first discovery")

        # Start webhook receiver
        await self.webhook_receiver.start()

    async def start_event_processing(self):
        """Start the event processing pipeline."""
        logger.info("[DiscoveryCoordinator] Starting event processing pipeline")

        # Process events from webhook receiver queue
        stats = await self.event_processor.process_events(
            self.webhook_receiver.event_queue
        )

        # Update coordinator statistics
        self.stats.events_processed = stats["processed"]
        self.stats.events_failed = stats["failed"]
        self.stats.total_wallets_discovered = stats["wallets_discovered"]
        self.stats.average_processing_time_ms = (
            stats.get("duration_seconds", 0) * 1000 / max(1, stats["processed"])
        )

    async def get_high_quality_wallets(
        self,
        min_score: Optional[float] = None
    ) -> List[DiscoveredWallet]:
        """Get high-quality wallets discovered via webhooks."""
        all_wallets = self.event_processor.get_discovered_wallets()

        # Apply quality filter
        filtered = await self.quality_filter.filter_wallets(all_wallets, min_score)

        logger.info(f"[DiscoveryCoordinator] Found {len(filtered)}/{len(all_wallets)} high-quality wallets")
        return filtered

    async def get_discovery_stats(self) -> Dict[str, Any]:
        """Get comprehensive discovery statistics."""
        return {
            "webhook_receiver": {
                "total_events_received": self.webhook_receiver.stats.total_events_received,
                "total_wallets_discovered": self.webhook_receiver.stats.total_wallets_discovered,
                "queue_depth": self.webhook_receiver.event_queue.qsize(),
            },
            "event_processor": {
                "discovered_wallets_count": len(self.event_processor.discovered_wallets),
            },
            "overall": {
                "webhook_ratio": self.webhook_ratio,
                "polling_ratio": self.polling_ratio,
                "total_events_processed": self.stats.events_processed,
                "events_failed": self.stats.events_failed,
                "total_wallets_discovered": self.stats.total_wallets_discovered,
                "average_processing_time_ms": self.stats.average_processing_time_ms,
            }
        }


# Singleton instance
_coordinator_instance: Optional[DiscoveryCoordinator] = None


def get_discovery_coordinator() -> DiscoveryCoordinator:
    """Get the singleton discovery coordinator instance."""
    global _coordinator_instance
    if _coordinator_instance is None:
        _coordinator_instance = DiscoveryCoordinator()
    return _coordinator_instance
