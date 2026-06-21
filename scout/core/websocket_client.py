"""
Enhanced WebSocket Client for Helius Business Plan

This module implements WebSocket connectivity for real-time data streaming
when scaling to Helius Business Plan ($499/month, 100M credits, 200 req/s).

Enhanced WebSockets Benefits (Business Plan):
- Real-time transaction monitoring
- Instant signal detection
- Token price streaming
- Wallet activity notifications
- Subscription-based data feeds

Features:
- Automatic reconnection with exponential backoff
- Subscription management for multiple data types
- Message parsing and normalization
- Connection health monitoring
- Backpressure handling
"""

import os
import time
import json
import logging
import asyncio
import threading
from typing import Dict, Optional, Any, Callable
from dataclasses import dataclass, field
from enum import Enum
from collections import deque
import websockets
from websockets.exceptions import ConnectionClosed

logger = logging.getLogger(__name__)


class SubscriptionType(Enum):
    """Types of WebSocket subscriptions."""

    ACCOUNT_SUBSCRIBE = "accountSubscribe"           # Account changes
    ACCOUNT_UNSUBSCRIBE = "accountUnsubscribe"
    LOGS_SUBSCRIBE = "logsSubscribe"                 # Log subscriptions
    LOGS_UNSUBSCRIBE = "logsUnsubscribe"
    PROGRAM_SUBSCRIBE = "programSubscribe"           # Program account changes
    PROGRAM_UNSUBSCRIBE = "programUnsubscribe"
    SLOT_SUBSCRIBE = "slotSubscribe"                 # Slot notifications
    SLOT_UNSUBSCRIBE = "slotUnsubscribe"
    ROOT_SUBSCRIBE = "rootSubscribe"                 # Chain updates
    ROOT_UNSUBSCRIBE = "rootUnsubscribe"
    TRANSACTION_SUBSCRIBE = "transactionSubscribe"   # Transaction updates
    TRANSACTION_UNSUBSCRIBE = "transactionUnsubscribe"


class MessageType(Enum):
    """Types of WebSocket messages."""

    ACCOUNT = "account"
    LOG = "log"
    PROGRAM = "program"
    SLOT = "slot"
    ROOT = "root"
    TRANSACTION = "transaction"
    PING = "ping"
    PONG = "pong"
    ERROR = "error"


@dataclass
class WebSocketMessage:
    """Parsed WebSocket message."""

    message_type: MessageType
    subscription_id: int
    data: Dict[str, Any]
    timestamp: float = field(default_factory=time.time)
    raw_message: str = ""


@dataclass
class Subscription:
    """Active subscription."""

    subscription_type: SubscriptionType
    filters: Dict[str, Any]
    callback: Optional[Callable[[WebSocketMessage], None]]
    subscribed_at: float
    message_count: int = 0
    last_message_at: float = 0.0


@dataclass
class ConnectionStats:
    """WebSocket connection statistics."""

    connected_at: float
    messages_received: int = 0
    messages_sent: int = 0
    bytes_received: int = 0
    bytes_sent: int = 0
    reconnection_count: int = 0
    last_ping_at: float = 0.0
    last_pong_at: float = 0.0
    latency_ms: float = 0.0

    @property
    def uptime_seconds(self) -> float:
        """Connection uptime in seconds."""
        return time.time() - self.connected_at

    @property
    def messages_per_second(self) -> float:
        """Messages received per second."""
        if self.uptime_seconds > 0:
            return self.messages_received / self.uptime_seconds
        return 0.0


@dataclass
class WebSocketConfig:
    """Configuration for WebSocket client."""

    # Connection settings
    WS_ENDPOINT: str = "wss://rpc.shyft.to"  # Helius WebSocket endpoint
    CONNECT_TIMEOUT: float = 10.0  # Connection timeout in seconds
    PING_INTERVAL: float = 30.0  # Ping interval in seconds
    PING_TIMEOUT: float = 10.0  # Ping timeout in seconds

    # Reconnection settings
    MAX_RECONNECT_ATTEMPTS: int = 10
    INITIAL_RECONNECT_DELAY: float = 1.0  # Initial reconnection delay
    MAX_RECONNECT_DELAY: float = 60.0  # Maximum reconnection delay
    RECONNECT_MULTIPLIER: float = 1.5  # Exponential backoff multiplier

    # Queue settings
    MESSAGE_QUEUE_SIZE: int = 10000  # Maximum queued messages
    BACKPRESSURE_THRESHOLD: int = 8000  # Start dropping at this queue size

    # Subscription settings
    MAX_SUBSCRIPTIONS: int = 100  # Maximum active subscriptions
    SUBSCRIPTION_TIMEOUT: float = 5.0  # Subscription confirmation timeout


class HeliusWebSocketClient:
    """
    Enhanced WebSocket client for Helius Business Plan.

    Features:
    - Real-time data streaming
    - Automatic reconnection
    - Subscription management
    - Message parsing and normalization
    - Connection health monitoring
    """

    def __init__(self, config: Optional[WebSocketConfig] = None, api_key: Optional[str] = None):
        """
        Initialize the WebSocket client.

        Args:
            config: WebSocket configuration
            api_key: Helius API key (defaults to environment variable)
        """
        self._config = config or WebSocketConfig()
        self._api_key = api_key or os.getenv("HELIUS_API_KEY")

        if not self._api_key:
            logger.warning("No Helius API key provided - WebSocket may not work properly")

        # Connection state
        self._websocket: Optional[websockets.WebSocketClientProtocol] = None
        self._connected = False
        self._should_reconnect = True
        self._reconnect_attempts = 0

        # Subscriptions
        self._subscriptions: Dict[int, Subscription] = {}
        self._subscription_counter = 0
        self._pending_subscriptions: deque = deque()

        # Message queue
        self._message_queue: deque[WebSocketMessage] = deque(maxlen=self._config.MESSAGE_QUEUE_SIZE)
        self._queue_lock = threading.Lock()

        # Statistics
        self._stats = ConnectionStats(connected_at=time.time())

        # Event loop for async operations
        self._event_loop: Optional[asyncio.AbstractEventLoop] = None
        self._loop_thread: Optional[threading.Thread] = None

        logger.info("Helius WebSocket Client initialized")

    async def connect(self) -> bool:
        """
        Establish WebSocket connection.

        Returns:
            True if connection successful
        """
        endpoint = self._config.WS_ENDPOINT

        try:
            logger.info(f"Connecting to WebSocket: {endpoint}")

            # Add API key to endpoint if provided
            if self._api_key:
                endpoint = f"{endpoint}?api-key={self._api_key}"

            self._websocket = await websockets.connect(
                endpoint,
                close_timeout=self._config.CONNECT_TIMEOUT,
                ping_interval=self._config.PING_INTERVAL,
                ping_timeout=self._config.PING_TIMEOUT,
            )

            self._connected = True
            self._reconnect_attempts = 0
            self._stats = ConnectionStats(connected_at=time.time())

            logger.info("WebSocket connected successfully")

            # Start message listener
            asyncio.create_task(self._message_listener())

            # Resubscribe to previous subscriptions
            await self._resubscribe_all()

            return True

        except Exception as e:
            logger.error(f"WebSocket connection failed: {e}")
            return False

    async def disconnect(self):
        """Disconnect from WebSocket."""
        self._should_reconnect = False
        self._connected = False

        if self._websocket:
            try:
                await self._websocket.close()
                logger.info("WebSocket disconnected")
            except Exception as e:
                logger.warning(f"Error closing WebSocket: {e}")

    async def _message_listener(self):
        """Listen for incoming WebSocket messages."""
        if not self._websocket:
            return

        try:
            async for message in self._websocket:
                await self._handle_message(message)

        except ConnectionClosed:
            logger.warning("WebSocket connection closed")
            if self._should_reconnect:
                await self._reconnect()

        except Exception as e:
            logger.error(f"WebSocket message listener error: {e}")
            if self._should_reconnect:
                await self._reconnect()

    async def _handle_message(self, raw_message: str):
        """Handle incoming WebSocket message."""
        try:
            # Update statistics
            self._stats.messages_received += 1
            self._stats.bytes_received += len(raw_message)

            # Parse message
            data = json.loads(raw_message)

            # Determine message type
            if "result" in data and "subscription" in data:
                # Subscription confirmation
                subscription_id = data["subscription"]
                logger.debug(f"Subscription confirmed: {subscription_id}")

            elif "params" in data and "result" in data["params"]:
                # Regular data message
                subscription_id = data["params"].get("subscription", 0)
                result = data["params"]["result"]

                # Update subscription stats
                if subscription_id in self._subscriptions:
                    sub = self._subscriptions[subscription_id]
                    sub.message_count += 1
                    sub.last_message_at = time.time()

                    # Call callback if provided
                    if sub.callback:
                        try:
                            message = WebSocketMessage(
                                message_type=MessageType.TRANSACTION,  # Default
                                subscription_id=subscription_id,
                                data=result,
                                raw_message=raw_message
                            )
                            sub.callback(message)
                        except Exception as e:
                            logger.error(f"Callback error: {e}")

            elif "method" in data:
                # RPC method call
                method = data["method"]
                if method == "slotNotification":
                    pass  # Handle slot updates
                elif method == "accountNotification":
                    pass  # Handle account updates

            # Add to queue
            with self._queue_lock:
                if len(self._message_queue) >= self._message_queue.maxlen:
                    # Drop oldest message if queue is full (backpressure)
                    self._message_queue.popleft()

                self._message_queue.append(WebSocketMessage(
                    message_type=MessageType.TRANSACTION,
                    subscription_id=0,
                    data=data,
                    raw_message=raw_message
                ))

        except json.JSONDecodeError as e:
            logger.warning(f"Failed to parse WebSocket message: {e}")

        except Exception as e:
            logger.error(f"Error handling WebSocket message: {e}")

    async def subscribe(self, subscription_type: SubscriptionType,
                       filters: Dict[str, Any],
                       callback: Optional[Callable[[WebSocketMessage], None]] = None) -> Optional[int]:
        """
        Subscribe to WebSocket data feed.

        Args:
            subscription_type: Type of subscription
            filters: Subscription filters (account, program, etc.)
            callback: Optional callback for incoming messages

        Returns:
            Subscription ID or None if failed
        """
        if not self._connected or not self._websocket:
            logger.warning("Cannot subscribe - not connected")
            return None

        # Check subscription limit
        if len(self._subscriptions) >= self._config.MAX_SUBSCRIPTIONS:
            logger.warning(f"Maximum subscriptions reached ({self._config.MAX_SUBSCRIPTIONS})")
            return None

        # Create subscription
        self._subscription_counter += 1
        subscription_id = self._subscription_counter

        subscription = Subscription(
            subscription_type=subscription_type,
            filters=filters,
            callback=callback,
            subscribed_at=time.time()
        )

        self._subscriptions[subscription_id] = subscription

        # Send subscription request
        try:
            request = self._build_subscription_request(subscription_type, subscription_id, filters)
            await self._websocket.send(json.dumps(request))

            self._stats.messages_sent += 1
            self._stats.bytes_sent += len(json.dumps(request))

            logger.info(f"Subscribed to {subscription_type.value} (ID: {subscription_id})")
            return subscription_id

        except Exception as e:
            logger.error(f"Subscription failed: {e}")
            del self._subscriptions[subscription_id]
            return None

    def _build_subscription_request(self, subscription_type: SubscriptionType,
                                    subscription_id: int, filters: Dict[str, Any]) -> Dict[str, Any]:
        """Build subscription request JSON."""
        return {
            "jsonrpc": "2.0",
            "id": subscription_id,
            "method": subscription_type.value,
            "params": filters
        }

    async def unsubscribe(self, subscription_id: int) -> bool:
        """
        Unsubscribe from data feed.

        Args:
            subscription_id: Subscription ID to unsubscribe

        Returns:
            True if successful
        """
        if subscription_id not in self._subscriptions:
            logger.warning(f"Subscription {subscription_id} not found")
            return False

        subscription = self._subscriptions[subscription_id]

        try:
            # Build unsubscribe request
            unsubscribe_method = subscription.subscription_type.value.replace("Subscribe", "Unsubscribe")

            request = {
                "jsonrpc": "2.0",
                "id": subscription_id,
                "method": unsubscribe_method,
                "params": subscription.filters
            }

            await self._websocket.send(json.dumps(request))
            self._stats.messages_sent += 1

            # Remove subscription
            del self._subscriptions[subscription_id]

            logger.info(f"Unsubscribed from {subscription.subscription_type.value} (ID: {subscription_id})")
            return True

        except Exception as e:
            logger.error(f"Unsubscribe failed: {e}")
            return False

    async def _resubscribe_all(self):
        """Resubscribe to all previous subscriptions after reconnection."""
        for subscription_id, subscription in list(self._subscriptions.items()):
            try:
                await self.subscribe(subscription.subscription_type, subscription.filters, subscription.callback)
            except Exception as e:
                logger.error(f"Failed to resubscribe: {e}")

    async def _reconnect(self):
        """Attempt to reconnect with exponential backoff."""
        if not self._should_reconnect:
            return

        delay = self._config.INITIAL_RECONNECT_DELAY
        self._reconnect_attempts += 1
        self._stats.reconnection_count += 1

        logger.info(f"Reconnecting... Attempt {self._reconnect_attempts}")

        # Exponential backoff
        if self._reconnect_attempts > 1:
            delay = min(
                delay * (self._config.RECONNECT_MULTIPLIER ** (self._reconnect_attempts - 1)),
                self._config.MAX_RECONNECT_DELAY
            )

        await asyncio.sleep(delay)

        # Try to reconnect
        if await self.connect():
            logger.info("Reconnection successful")
        elif self._reconnect_attempts < self._config.MAX_RECONNECT_ATTEMPTS:
            await self._reconnect()
        else:
            logger.error(f"Max reconnection attempts reached ({self._config.MAX_RECONNECT_ATTEMPTS})")

    def get_message(self, timeout: float = 1.0) -> Optional[WebSocketMessage]:
        """
        Get next message from queue (blocking).

        Args:
            timeout: Maximum time to wait for message

        Returns:
            WebSocket message or None if timeout
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
        # Update latency based on ping/pong
        if self._stats.last_ping_at > 0 and self._stats.last_pong_at > 0:
            self._stats.latency_ms = (self._stats.last_pong_at - self._stats.last_ping_at) * 1000

        return self._stats

    def get_active_subscriptions(self) -> Dict[int, Subscription]:
        """Get all active subscriptions."""
        return self._subscriptions.copy()

    def print_status_report(self):
        """Print comprehensive status report."""
        stats = self.get_stats()

        print("\n" + "="*70)
        print("HELIUS WEBSOCKET CLIENT - STATUS")
        print("="*70)

        print("\nConnection Status:")
        print(f"  Connected: {self._connected}")
        print(f"  Uptime: {stats.uptime_seconds:.0f} seconds")
        print(f"  Reconnections: {stats.reconnection_count}")

        print("\nMessage Statistics:")
        print(f"  Received: {stats.messages_received:,} ({stats.messages_per_second:.1f} msg/s)")
        print(f"  Sent: {stats.messages_sent:,}")
        print(f"  Bytes received: {stats.bytes_received:,}")
        print(f"  Bytes sent: {stats.bytes_sent:,}")

        print("\nLatency:")
        print(f"  Current: {stats.latency_ms:.1f} ms")

        print(f"\nActive Subscriptions: {len(self._subscriptions)}")
        for sub_id, sub in self._subscriptions.items():
            print(f"  [{sub_id}] {sub.subscription_type.value}: {sub.message_count} messages")

        print("="*70 + "\n")

    async def shutdown(self):
        """Cleanup and shutdown."""
        await self.disconnect()

        # Clear subscriptions
        self._subscriptions.clear()

        # Clear message queue
        with self._queue_lock:
            self._message_queue.clear()

        logger.info("WebSocket client shut down")

    async def monitor_wallet_activity(self, wallet_address: str,
                                     callback: Callable[[WebSocketMessage], None]) -> Optional[int]:
        """
        Subscribe to wallet activity updates.

        Args:
            wallet_address: Wallet address to monitor
            callback: Callback for transaction updates

        Returns:
            Subscription ID or None if failed
        """
        return await self.subscribe(
            SubscriptionType.ACCOUNT_SUBSCRIBE,
            {"account": wallet_address, "encoding": "jsonParsed"},
            callback
        )

    async def monitor_program_logs(self, program_id: str,
                                  callback: Callable[[WebSocketMessage], None]) -> Optional[int]:
        """
        Subscribe to program log updates.

        Args:
            program_id: Program ID to monitor
            callback: Callback for log updates

        Returns:
            Subscription ID or None if failed
        """
        return await self.subscribe(
            SubscriptionType.LOGS_SUBSCRIBE,
            {"mentions": [program_id]},
            callback
        )

    async def monitor_token_transfers(self, token_mint: str,
                                    callback: Callable[[WebSocketMessage], None]) -> Optional[int]:
        """
        Subscribe to token transfer events.

        Args:
            token_mint: Token mint address
            callback: Callback for transfer events

        Returns:
            Subscription ID or None if failed
        """
        return await self.subscribe(
            SubscriptionType.ACCOUNT_SUBSCRIBE,
            {"account": token_mint, "encoding": "jsonParsed"},
            callback
        )


# Global singleton instance
_client: Optional[HeliusWebSocketClient] = None
_client_lock = threading.Lock()


def get_websocket_client(api_key: Optional[str] = None) -> HeliusWebSocketClient:
    """Get the global WebSocket client singleton."""
    global _client

    with _client_lock:
        if _client is None:
            _client = HeliusWebSocketClient(api_key=api_key)

    return _client


def reset_websocket_client():
    """Reset the global WebSocket client (mainly for testing)."""
    global _client

    with _client_lock:
        if _client:
            # Note: This should be called from async context
            _client = None


if __name__ == "__main__":
    # Test the WebSocket client
    async def test_websocket():
        client = get_websocket_client()

        # Try to connect
        if await client.connect():
            print("WebSocket connected successfully")

            # Print status
            client.print_status_report()

            # Disconnect
            await client.disconnect()
        else:
            print("WebSocket connection failed")

        await client.shutdown()

    # Run test
    asyncio.run(test_websocket())
