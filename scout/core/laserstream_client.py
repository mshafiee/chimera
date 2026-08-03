"""
LaserStream gRPC Client for Helius Business Plan

This module implements gRPC connectivity for ultra-low-latency data streaming
when scaling to Helius Business Plan ($499/month, 100M credits, 200 req/s).

LaserStream Benefits (Business Plan):
- Sub-millisecond latency data delivery
- Real-time transaction streaming
- DEX trade streaming
- Token price streaming
- Account activity streaming

Features:
- gRPC streaming for minimal latency
- Automatic reconnection with backoff
- Stream subscription management
- Message parsing and normalization
- Connection health monitoring
- Backpressure handling

LaserStream Endpoints:
- Transactions: Real-time transaction streaming
- DEX Trades: DEX trade streaming with parsed swap data
- Prices: Token price streaming
- Accounts: Account activity streaming
"""

import os
import time
import logging
import asyncio
import threading
from typing import Dict, List, Optional, Tuple, Any, Callable
from dataclasses import dataclass, field
from enum import Enum
from collections import deque
import grpc
from grpc import aio

logger = logging.getLogger(__name__)


class StreamType(Enum):
    """Types of LaserStream subscriptions."""

    TRANSACTIONS = "transactions"      # Transaction streaming
    DEX_TRADES = "dex_trades"         # DEX trade streaming
    PRICES = "prices"                 # Token price streaming
    ACCOUNTS = "accounts"             # Account activity streaming
    TOKENS = "tokens"                 # Token activity streaming


class MessageType(Enum):
    """Types of LaserStream messages."""

    TRANSACTION = "transaction"
    DEX_TRADE = "dex_trade"
    PRICE_UPDATE = "price_update"
    ACCOUNT_UPDATE = "account_update"
    TOKEN_UPDATE = "token_update"
    HEARTBEAT = "heartbeat"
    ERROR = "error"


@dataclass
class StreamMessage:
    """Parsed LaserStream message."""

    message_type: MessageType
    stream_type: StreamType
    data: Dict[str, Any]
    timestamp: float = field(default_factory=time.time)
    sequence_number: int = 0
    latency_ms: float = 0.0


@dataclass
class StreamSubscription:
    """Active stream subscription."""

    stream_type: StreamType
    filters: Dict[str, Any]
    callback: Callable[[StreamMessage], None]
    subscribed_at: float
    message_count: int = 0
    last_message_at: float = 0.0
    bytes_received: int = 0
    task: Optional[asyncio.Task] = None


@dataclass
class ConnectionStats:
    """LaserStream connection statistics."""

    connected_at: float
    messages_received: int = 0
    messages_per_second: float = 0.0
    bytes_received: int = 0
    reconnection_count: int = 0
    last_heartbeat_at: float = 0.0
    average_latency_ms: float = 0.0
    peak_latency_ms: float = 0.0

    @property
    def uptime_seconds(self) -> float:
        """Connection uptime in seconds."""
        return time.time() - self.connected_at


@dataclass
class LaserStreamConfig:
    """Configuration for LaserStream client."""

    # gRPC endpoint
    GRPC_ENDPOINT: str = "laserstream.helius-rpc.com:443"

    # Connection settings
    CONNECT_TIMEOUT: float = 10.0  # Connection timeout in seconds
    KEEPALIVE_TIMEOUT: float = 60.0  # Keepalive timeout
    KEEPALIVE_PERMIT_WITHOUT_CALLS: int = 5  # Keepalive probes

    # Stream settings
    MAX_RECEIVE_MESSAGE_LENGTH: int = 100 * 1024 * 1024  # 100MB
    INITIAL_METADATA: Tuple[Tuple[str, str], ...] = ()

    # Reconnection settings
    MAX_RECONNECT_ATTEMPTS: int = 10
    INITIAL_RECONNECT_DELAY: float = 1.0
    MAX_RECONNECT_DELAY: float = 60.0
    RECONNECT_MULTIPLIER: float = 1.5

    # Queue settings
    MESSAGE_QUEUE_SIZE: int = 10000
    BACKPRESSURE_THRESHOLD: int = 8000

    # Performance settings
    ENABLE_COMPRESSION: bool = True  # Enable gRPC compression
    MAX_CONCURRENT_STREAMS: int = 100


class LaserStreamClient:
    """
    LaserStream gRPC client for Helius Business Plan.

    Features:
    - Ultra-low-latency data streaming
    - Automatic reconnection
    - Stream subscription management
    - Message parsing and normalization
    - Connection health monitoring
    """

    def __init__(self, config: Optional[LaserStreamConfig] = None, api_key: Optional[str] = None):
        """
        Initialize the LaserStream client.

        Args:
            config: LaserStream configuration
            api_key: Helius API key (defaults to environment variable)
        """
        self._config = config or LaserStreamConfig()
        self._api_key = api_key or os.getenv("HELIUS_API_KEY")

        if not self._api_key:
            logger.warning("No Helius API key provided - LaserStream may not work properly")

        # Connection state
        self._channel: Optional[aio.Channel] = None
        self._connected = False
        self._should_reconnect = True
        self._reconnect_attempts = 0

        # Streams
        self._active_streams: Dict[str, grpc.aio.UnaryStreamCall] = {}
        self._subscriptions: Dict[str, StreamSubscription] = {}
        self._stream_tasks: list[asyncio.Task] = []

        # Reconnection guard (only one reconnect loop at a time)
        self._is_reconnecting = False

        # Message queue
        self._message_queue: deque[StreamMessage] = deque(maxlen=self._config.MESSAGE_QUEUE_SIZE)
        self._queue_lock = threading.Lock()

        # Statistics
        self._stats = ConnectionStats(connected_at=time.time())
        self._latency_samples: List[float] = []

        # Event loop
        self._event_loop: Optional[asyncio.AbstractEventLoop] = None

        logger.info("LaserStream gRPC Client initialized")

    async def connect(self) -> bool:
        """
        Establish gRPC connection.

        Returns:
            True if connection successful
        """
        endpoint = self._config.GRPC_ENDPOINT

        try:
            logger.info(f"Connecting to LaserStream gRPC: {endpoint}")

            # Create channel credentials
            if self._api_key:
                credentials = grpc.composite_channel_credentials(
                    grpc.ssl_channel_credentials(),
                    grpc.access_token_call_credentials(self._api_key)
                )
            else:
                credentials = grpc.ssl_channel_credentials()

            # Create channel with compression (passed to the constructor —
            # 'grpc.enable_compression' is not a valid channel option)
            compression = grpc.Compression.Gzip if self._config.ENABLE_COMPRESSION else grpc.Compression.NoCompression

            # Preserve stats across reconnects (reconnection_count etc.)
            if self._reconnect_attempts > 0 and hasattr(self, '_stats'):
                existing_stats = self._stats
            else:
                existing_stats = None

            self._channel = aio.secure_channel(
                endpoint,
                credentials,
                options=[
                    ('grpc.max_receive_message_length', self._config.MAX_RECEIVE_MESSAGE_LENGTH),
                    ('grpc.keepalive_timeout_ms', int(self._config.KEEPALIVE_TIMEOUT * 1000)),
                    ('grpc.keepalive_permit_without_calls', self._config.KEEPALIVE_PERMIT_WITHOUT_CALLS),
                    ('grpc.max_concurrent_streams', self._config.MAX_CONCURRENT_STREAMS),
                ],
                compression=compression,
            )

            # Test connection with simple call
            # Note: Actual implementation depends on Helius gRPC proto definitions
            await asyncio.wait_for(self._channel.channel_ready(), timeout=self._config.CONNECT_TIMEOUT)

            self._connected = True
            self._reconnect_attempts = 0
            if existing_stats is not None:
                # Keep accumulated counters; only refresh the uptime anchor
                existing_stats.connected_at = time.time()
                self._stats = existing_stats
            else:
                self._stats = ConnectionStats(connected_at=time.time())

            logger.info("LaserStream gRPC connected successfully")

            return True

        except asyncio.TimeoutError:
            logger.error(f"LaserStream connection timeout after {self._config.CONNECT_TIMEOUT}s")
            return False

        except Exception as e:
            logger.error(f"LaserStream connection failed: {e}")
            return False

    async def disconnect(self):
        """Disconnect from LaserStream."""
        self._should_reconnect = False
        self._connected = False

        # Close all active streams
        for stream_id, stream in self._active_streams.items():
            try:
                await stream.cancel()
            except Exception as e:
                logger.warning(f"Error closing stream {stream_id}: {e}")

        self._active_streams.clear()

        # Close channel
        if self._channel:
            try:
                await self._channel.close()
                logger.info("LaserStream disconnected")
            except Exception as e:
                logger.warning(f"Error closing LaserStream: {e}")

    async def subscribe_to_transactions(self, filters: Optional[Dict[str, Any]] = None,
                                     callback: Optional[Callable[[StreamMessage], None]] = None) -> Optional[str]:
        """
        Subscribe to transaction streaming.

        Args:
            filters: Optional filters (account, program, etc.)
            callback: Callback for transaction updates

        Returns:
            Stream ID or None if failed
        """
        if not self._connected or not self._channel:
            logger.warning("Cannot subscribe - not connected")
            return None

        stream_id = f"tx_{int(time.time() * 1000)}"

        try:
            # Create subscription record
            self._subscriptions[stream_id] = StreamSubscription(
                stream_type=StreamType.TRANSACTIONS,
                filters=filters or {},
                callback=callback or (lambda msg: None),
                subscribed_at=time.time()
            )

            # Start streaming
            task = asyncio.create_task(self._stream_transactions(stream_id, filters or {}))
            self._stream_tasks.append(task)
            self._subscriptions[stream_id].task = task

            logger.info(f"Subscribed to transaction streaming (ID: {stream_id})")
            return stream_id

        except Exception as e:
            logger.error(f"Transaction subscription failed: {e}")
            if stream_id in self._subscriptions:
                del self._subscriptions[stream_id]
            return None

    async def subscribe_to_dex_trades(self, token_mints: Optional[List[str]] = None,
                                     callback: Optional[Callable[[StreamMessage], None]] = None) -> Optional[str]:
        """
        Subscribe to DEX trade streaming.

        Args:
            token_mints: Optional list of token mint addresses to filter
            callback: Callback for trade updates

        Returns:
            Stream ID or None if failed
        """
        if not self._connected or not self._channel:
            logger.warning("Cannot subscribe - not connected")
            return None

        stream_id = f"dex_{int(time.time() * 1000)}"

        try:
            self._subscriptions[stream_id] = StreamSubscription(
                stream_type=StreamType.DEX_TRADES,
                filters={"token_mints": token_mints or []},
                callback=callback or (lambda msg: None),
                subscribed_at=time.time()
            )

            # Start streaming
            task = asyncio.create_task(self._stream_dex_trades(stream_id, token_mints or []))
            self._stream_tasks.append(task)
            self._subscriptions[stream_id].task = task

            logger.info(f"Subscribed to DEX trade streaming (ID: {stream_id})")
            return stream_id

        except Exception as e:
            logger.error(f"DEX trade subscription failed: {e}")
            if stream_id in self._subscriptions:
                del self._subscriptions[stream_id]
            return None

    async def subscribe_to_prices(self, token_mints: List[str],
                                 callback: Optional[Callable[[StreamMessage], None]] = None) -> Optional[str]:
        """
        Subscribe to token price streaming.

        Args:
            token_mints: List of token mint addresses
            callback: Callback for price updates

        Returns:
            Stream ID or None if failed
        """
        if not self._connected or not self._channel:
            logger.warning("Cannot subscribe - not connected")
            return None

        stream_id = f"price_{int(time.time() * 1000)}"

        try:
            self._subscriptions[stream_id] = StreamSubscription(
                stream_type=StreamType.PRICES,
                filters={"token_mints": token_mints},
                callback=callback or (lambda msg: None),
                subscribed_at=time.time()
            )

            # Start streaming
            task = asyncio.create_task(self._stream_prices(stream_id, token_mints))
            self._stream_tasks.append(task)
            self._subscriptions[stream_id].task = task

            logger.info(f"Subscribed to price streaming (ID: {stream_id})")
            return stream_id

        except Exception as e:
            logger.error(f"Price subscription failed: {e}")
            if stream_id in self._subscriptions:
                del self._subscriptions[stream_id]
            return None

    async def _stream_transactions(self, stream_id: str, filters: Dict[str, Any]):
        """Stream transaction updates.

        NOTE: The actual Helius LaserStream gRPC call requires the Helius
        proto stubs (not present in this repo). Until those exist, the loop
        keeps the subscription alive and delivers heartbeats so the queue /
        stats / backpressure plumbing is exercised and observable.
        """
        try:
            logger.debug(f"Started transaction stream {stream_id}")
            await self._stream_keepalive_loop(stream_id, StreamType.TRANSACTIONS)

        except Exception as e:
            logger.error(f"Transaction stream error: {e}")
            if self._should_reconnect:
                await self._reconnect()

    async def _stream_dex_trades(self, stream_id: str, token_mints: List[str]):
        """Stream DEX trade updates (placeholder until Helius proto stubs exist)."""
        try:
            logger.debug(f"Started DEX trades stream {stream_id}")
            await self._stream_keepalive_loop(stream_id, StreamType.DEX_TRADES)

        except Exception as e:
            logger.error(f"DEX trades stream error: {e}")
            if self._should_reconnect:
                await self._reconnect()

    async def _stream_prices(self, stream_id: str, token_mints: List[str]):
        """Stream price updates (placeholder until Helius proto stubs exist)."""
        try:
            logger.debug(f"Started price stream {stream_id}")
            await self._stream_keepalive_loop(stream_id, StreamType.PRICES)

        except Exception as e:
            logger.error(f"Price stream error: {e}")
            if self._should_reconnect:
                await self._reconnect()

    async def _stream_keepalive_loop(self, stream_id: str, stream_type: StreamType):
        """Keep a subscription alive while connected, delivering heartbeats.

        This is the shared placeholder body for the gRPC stream coroutines.
        The real implementation replaces the heartbeat loop with an actual
        ``channel.unary_stream(...)`` iteration once the Helius proto stubs
        are available.
        """
        subscription = self._subscriptions.get(stream_id)
        while self._connected and stream_id in self._subscriptions:
            await asyncio.sleep(30)  # Heartbeat interval
            if stream_id not in self._subscriptions:
                break
            heartbeat = StreamMessage(
                message_type=MessageType.HEARTBEAT,
                stream_type=stream_type,
                data={"stream_id": stream_id},
                timestamp=time.time(),
            )
            with self._queue_lock:
                self._message_queue.append(heartbeat)
            self._stats.last_heartbeat_at = time.time()
            if subscription is not None:
                subscription.message_count += 1
                subscription.last_message_at = time.time()
            # Keep the stats visible even with no data stream
            self._stats.messages_received += 1

    async def unsubscribe(self, stream_id: str) -> bool:
        """
        Unsubscribe from stream.

        Args:
            stream_id: Stream ID to unsubscribe

        Returns:
            True if successful
        """
        if stream_id not in self._subscriptions:
            logger.warning(f"Stream {stream_id} not found")
            return False

        try:
            # Cancel the stream task if running (the task is stored on the
            # subscription so unsubscribe actually stops it)
            subscription = self._subscriptions[stream_id]
            if subscription.task and not subscription.task.done():
                subscription.task.cancel()
                try:
                    await subscription.task
                except (asyncio.CancelledError, Exception):
                    pass

            # Cancel stream if active
            if stream_id in self._active_streams:
                await self._active_streams[stream_id].cancel()
                del self._active_streams[stream_id]

            # Remove subscription
            del self._subscriptions[stream_id]

            # Prune completed tasks so the registry doesn't grow unbounded
            self._stream_tasks = [t for t in self._stream_tasks if not t.done()]

            logger.info(f"Unsubscribed from stream {stream_id}")
            return True

        except Exception as e:
            logger.error(f"Unsubscribe failed: {e}")
            return False

    async def _reconnect(self):
        """Attempt to reconnect with exponential backoff."""
        if not self._should_reconnect:
            return

        # Single in-flight guard: stream tasks call this independently on
        # error — only one reconnect loop may run at a time
        if self._is_reconnecting:
            logger.debug("Reconnection already in progress, skipping")
            return
        self._is_reconnecting = True

        try:
            delay = self._config.INITIAL_RECONNECT_DELAY
            self._reconnect_attempts += 1
            self._stats.reconnection_count += 1

            logger.info(f"LaserStream reconnecting... Attempt {self._reconnect_attempts}")

            # Exponential backoff
            if self._reconnect_attempts > 1:
                delay = min(
                    delay * (self._config.RECONNECT_MULTIPLIER ** (self._reconnect_attempts - 1)),
                    self._config.MAX_RECONNECT_DELAY
                )

            await asyncio.sleep(delay)

            # Try to reconnect
            if await self.connect():
                logger.info("LaserStream reconnection successful")
                # Resubscribe to previous streams
                await self._resubscribe_all()
            elif self._reconnect_attempts < self._config.MAX_RECONNECT_ATTEMPTS:
                await self._reconnect()
            else:
                logger.error("Max LaserStream reconnection attempts reached")
        finally:
            self._is_reconnecting = False

    async def _resubscribe_all(self):
        """Resubscribe to all previous streams after reconnection."""
        subscriptions = list(self._subscriptions.items())
        for stream_id, sub in subscriptions:
            try:
                if sub.stream_type == StreamType.TRANSACTIONS:
                    task = asyncio.create_task(self._stream_transactions(stream_id, sub.filters))
                elif sub.stream_type == StreamType.DEX_TRADES:
                    task = asyncio.create_task(self._stream_dex_trades(stream_id, sub.filters.get("token_mints", [])))
                elif sub.stream_type == StreamType.PRICES:
                    task = asyncio.create_task(self._stream_prices(stream_id, sub.filters.get("token_mints", [])))
                else:
                    continue
                self._stream_tasks.append(task)
                sub.task = task
                logger.info(f"Resubscribed to {sub.stream_type.value} stream {stream_id}")
            except Exception as e:
                logger.warning(f"Failed to resubscribe to {stream_id}: {e}")
        logger.info(f"Resubscribed to {len(subscriptions)} streams")

    def _update_latency(self, latency_ms: float):
        """Update latency statistics."""
        self._latency_samples.append(latency_ms)

        # Keep only recent samples (last 100)
        if len(self._latency_samples) > 100:
            self._latency_samples.pop(0)

        # Calculate average
        if self._latency_samples:
            self._stats.average_latency_ms = sum(self._latency_samples) / len(self._latency_samples)
            self._stats.peak_latency_ms = max(self._latency_samples)

    def get_message(self, timeout: float = 1.0) -> Optional[StreamMessage]:
        """
        Get next message from queue (blocking).

        Args:
            timeout: Maximum time to wait for message

        Returns:
            Stream message or None if timeout
        """
        start_time = time.time()

        while time.time() - start_time < timeout:
            with self._queue_lock:
                if self._message_queue:
                    return self._message_queue.popleft()

            time.sleep(0.01)

        return None

    def get_stats(self) -> ConnectionStats:
        """Get connection statistics."""
        # Calculate messages per second
        uptime = self._stats.uptime_seconds
        if uptime > 0:
            self._stats.messages_per_second = self._stats.messages_received / uptime

        return self._stats

    def get_active_subscriptions(self) -> Dict[str, StreamSubscription]:
        """Get all active subscriptions."""
        return self._subscriptions.copy()

    def print_status_report(self):
        """Print comprehensive status report."""
        stats = self.get_stats()

        print("\n" + "="*70)
        print("LASERSTREAM gRPC CLIENT - STATUS")
        print("="*70)

        print("\nConnection Status:")
        print(f"  Connected: {self._connected}")
        print(f"  Uptime: {stats.uptime_seconds:.0f} seconds")
        print(f"  Reconnections: {stats.reconnection_count}")

        print("\nMessage Statistics:")
        print(f"  Received: {stats.messages_received:,}")
        print(f"  Rate: {stats.messages_per_second:.1f} msg/s")
        print(f"  Bytes received: {stats.bytes_received:,}")

        print("\nLatency:")
        print(f"  Average: {stats.average_latency_ms:.2f} ms")
        print(f"  Peak: {stats.peak_latency_ms:.2f} ms")

        print(f"\nActive Subscriptions: {len(self._subscriptions)}")
        for stream_id, sub in self._subscriptions.items():
            print(f"  [{stream_id}] {sub.stream_type.value}: {sub.message_count} messages")

        print("="*70 + "\n")

    async def shutdown(self):
        """Cleanup and shutdown."""
        # Cancel any running stream tasks
        for task in self._stream_tasks:
            if not task.done():
                task.cancel()
        if self._stream_tasks:
            await asyncio.gather(*self._stream_tasks, return_exceptions=True)
        self._stream_tasks.clear()

        await self.disconnect()

        # Clear subscriptions
        self._subscriptions.clear()

        # Clear message queue
        with self._queue_lock:
            self._message_queue.clear()

        logger.info("LaserStream client shut down")


# Global singleton instance
_client: Optional[LaserStreamClient] = None
_client_lock = threading.Lock()


def get_laserstream_client(api_key: Optional[str] = None) -> LaserStreamClient:
    """Get the global LaserStream client singleton."""
    global _client

    with _client_lock:
        if _client is None:
            _client = LaserStreamClient(api_key=api_key)

    return _client


def reset_laserstream_client():
    """Reset the global LaserStream client (mainly for testing).

    NOTE: use ``await shutdown_laserstream_client()`` for proper async
    cleanup; this sync variant only drops the reference.
    """
    global _client

    with _client_lock:
        _client = None


async def shutdown_laserstream_client():
    """Shut the global client down cleanly (closes channel, cancels streams)."""
    global _client

    with _client_lock:
        client = _client
        _client = None

    if client is not None:
        try:
            await client.shutdown()
        except Exception as e:
            logger.warning(f"LaserStream shutdown error: {e}")


if __name__ == "__main__":
    # Test the LaserStream client
    async def test_laserstream():
        client = get_laserstream_client()

        # Try to connect
        if await client.connect():
            print("LaserStream gRPC connected successfully")

            # Print status
            client.print_status_report()

            # Disconnect
            await client.disconnect()
        else:
            print("LaserStream gRPC connection failed")

        await client.shutdown()

    # Run test
    asyncio.run(test_laserstream())
