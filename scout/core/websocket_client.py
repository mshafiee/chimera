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

        # Subscriptions: local id -> Subscription. Server-issued subscription
        # ids are mapped via _server_to_local so notifications resolve to the
        # right local entry (Solana does NOT echo the client's request id in
        # notifications — it sends its own subscription id).
        self._subscriptions: Dict[int, Subscription] = {}
        self._subscription_counter = 0
        self._pending_subs: Dict[int, int] = {}  # request id -> local sub id
        self._server_to_local: Dict[int, int] = {}  # server sub id -> local id
        self._state_lock = threading.Lock()

        # Message queue
        self._message_queue: deque[WebSocketMessage] = deque(maxlen=self._config.MESSAGE_QUEUE_SIZE)
        self._queue_lock = threading.Lock()

        # Statistics
        self._stats = ConnectionStats(connected_at=time.time())

        # Message listener task (retained so disconnect() can cancel it)
        self._listener_task: Optional[asyncio.Task] = None

        logger.info("Helius WebSocket Client initialized")

    async def connect(self) -> bool:
        """
        Establish WebSocket connection.

        Returns:
            True if connection successful
        """
        endpoint = self._config.WS_ENDPOINT

        try:
            # Log only the redacted endpoint — the api-key query parameter
            # must never appear in logs (error messages from websockets.connect
            # include the full URI)
            log_endpoint = endpoint
            if self._api_key:
                log_endpoint = f"{endpoint}?api-key=REDACTED"
            logger.info(f"Connecting to WebSocket: {log_endpoint}")

            # Add API key to endpoint if provided
            if self._api_key:
                endpoint = f"{endpoint}?api-key={self._api_key}"

            # Preserve accumulated stats across reconnects
            existing_stats = self._stats

            self._websocket = await websockets.connect(
                endpoint,
                close_timeout=self._config.CONNECT_TIMEOUT,
                ping_interval=self._config.PING_INTERVAL,
                ping_timeout=self._config.PING_TIMEOUT,
            )

            self._connected = True
            self._reconnect_attempts = 0
            # Keep counters (reconnection_count, message totals); refresh the
            # uptime anchor only
            existing_stats.connected_at = time.time()
            self._stats = existing_stats

            logger.info("WebSocket connected successfully")

            # Start message listener (single retained task)
            self._listener_task = asyncio.create_task(self._message_listener())

            # Resubscribe to previous subscriptions
            await self._resubscribe_all()

            return True

        except Exception as e:
            logger.error("WebSocket connection failed: %s", e)
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
            self._websocket = None

        # Cancel the retained listener task
        if self._listener_task and not self._listener_task.done():
            self._listener_task.cancel()
            try:
                await self._listener_task
            except (asyncio.CancelledError, Exception):
                pass
            self._listener_task = None

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

        except asyncio.CancelledError:
            pass

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

            message_type = MessageType.TRANSACTION
            subscription_id = 0
            message_data: Dict[str, Any] = data

            # Subscription confirmation: {"result": <server_sub_id>, "id": <request_id>}
            if "id" in data and "result" in data and "method" not in data:
                request_id = data.get("id")
                server_id = data.get("result")
                local_id = self._pending_subs.pop(request_id, None)
                if local_id is not None and isinstance(server_id, int):
                    with self._state_lock:
                        self._server_to_local[server_id] = local_id
                    logger.debug(
                        "Subscription confirmed: local=%s server=%s",
                        local_id, server_id,
                    )
                # Confirmation messages are not data — skip queueing
                return

            # Method notifications (must be checked BEFORE the generic
            # params.result branch, which also matches them)
            method = data.get("method")
            if method:
                if method == "slotNotification":
                    message_type = MessageType.SLOT
                    params = data.get("params", {})
                    message_data = params.get("result", params)
                elif method == "accountNotification":
                    message_type = MessageType.ACCOUNT
                    params = data.get("params", {})
                    subscription_id = params.get("subscription", 0)
                    message_data = params.get("result", params)
                elif method == "logsNotification":
                    message_type = MessageType.LOG
                    params = data.get("params", {})
                    subscription_id = params.get("subscription", 0)
                    message_data = params.get("result", params)
                elif method == "programNotification":
                    message_type = MessageType.PROGRAM
                    params = data.get("params", {})
                    subscription_id = params.get("subscription", 0)
                    message_data = params.get("result", params)
                elif method == "transactionNotification":
                    message_type = MessageType.TRANSACTION
                    params = data.get("params", {})
                    subscription_id = params.get("subscription", 0)
                    message_data = params.get("result", params)
                elif method == "rootNotification":
                    message_type = MessageType.ROOT
                    params = data.get("params", {})
                    message_data = params.get("result", params)
            elif "params" in data and "result" in data["params"]:
                # Regular data message
                subscription_id = data["params"].get("subscription", 0)
                result = data["params"]["result"]
                message_data = result

            # Resolve the LOCAL subscription for callbacks/stats
            local_id = None
            with self._state_lock:
                local_id = self._server_to_local.get(subscription_id, subscription_id)
            if local_id in self._subscriptions:
                sub = self._subscriptions[local_id]
                sub.message_count += 1
                sub.last_message_at = time.time()

                # Invoke callback without blocking the listener (offloaded)
                if sub.callback:
                    try:
                        message = WebSocketMessage(
                            message_type=message_type,
                            subscription_id=local_id,
                            data=message_data,
                            raw_message=raw_message
                        )
                        asyncio.create_task(asyncio.to_thread(sub.callback, message))
                    except Exception as e:
                        logger.error(f"Callback error: {e}")

            # Add to queue with the correct type and subscription id
            with self._queue_lock:
                if len(self._message_queue) >= self._message_queue.maxlen:
                    # Drop oldest message if queue is full (backpressure)
                    self._message_queue.popleft()

                self._message_queue.append(WebSocketMessage(
                    message_type=message_type,
                    subscription_id=local_id if local_id is not None else 0,
                    data=message_data,
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
            Local subscription ID or None if failed
        """
        if not self._connected or not self._websocket:
            logger.warning("Cannot subscribe - not connected")
            return None

        # Check subscription limit
        with self._state_lock:
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

            # Track the pending request id -> local id so the server's
            # subscription confirmation can be mapped back
            with self._state_lock:
                self._pending_subs[subscription_id] = subscription_id

            self._stats.messages_sent += 1
            self._stats.bytes_sent += len(json.dumps(request))

            logger.info(f"Subscribed to {subscription_type.value} (ID: {subscription_id})")
            return subscription_id

        except Exception as e:
            logger.error(f"Subscription failed: {e}")
            with self._state_lock:
                self._subscriptions.pop(subscription_id, None)
            return None

    def _build_subscription_request(self, subscription_type: SubscriptionType,
                                    subscription_id: int, filters: Dict[str, Any]) -> Dict[str, Any]:
        """Build subscription request JSON.

        Solana/Helius RPC subscription methods take POSITIONAL params, not an
        object — e.g. accountSubscribe -> params: ["<account>", {"encoding": ...}].
        """
        method = subscription_type.value

        if subscription_type == SubscriptionType.ACCOUNT_SUBSCRIBE:
            params = [filters.get("account"), {"encoding": filters.get("encoding", "jsonParsed"), "commitment": filters.get("commitment", "confirmed")}]
        elif subscription_type == SubscriptionType.LOGS_SUBSCRIBE:
            params = [filters.get("mentions", []), {"commitment": filters.get("commitment", "confirmed")}]
        elif subscription_type == SubscriptionType.PROGRAM_SUBSCRIBE:
            params = [filters.get("program"), {"encoding": filters.get("encoding", "base64"), "commitment": filters.get("commitment", "confirmed")}]
        elif subscription_type in (SubscriptionType.SLOT_SUBSCRIBE, SubscriptionType.ROOT_SUBSCRIBE):
            params = []
        else:
            params = [filters]

        return {
            "jsonrpc": "2.0",
            "id": subscription_id,
            "method": method,
            "params": params
        }

    async def unsubscribe(self, subscription_id: int) -> bool:
        """
        Unsubscribe from data feed.

        Args:
            subscription_id: Local subscription ID to unsubscribe

        Returns:
            True if successful
        """
        with self._state_lock:
            subscription = self._subscriptions.get(subscription_id)
        if subscription is None:
            logger.warning(f"Subscription {subscription_id} not found")
            return False

        try:
            # Build unsubscribe request. The *Unsubscribe methods expect the
            # SERVER subscription id as a positional list param.
            unsubscribe_method = subscription.subscription_type.value.replace("Subscribe", "Unsubscribe")
            server_id = None
            with self._state_lock:
                for sid, local in self._server_to_local.items():
                    if local == subscription_id:
                        server_id = sid
                        break
            params = [server_id] if server_id is not None else []

            request = {
                "jsonrpc": "2.0",
                "id": subscription_id,
                "method": unsubscribe_method,
                "params": params
            }

            await self._websocket.send(json.dumps(request))
            self._stats.messages_sent += 1

            # Remove subscription
            with self._state_lock:
                self._subscriptions.pop(subscription_id, None)
                self._server_to_local = {
                    sid: local for sid, local in self._server_to_local.items()
                    if local != subscription_id
                }

            logger.info(f"Unsubscribed from {subscription.subscription_type.value} (ID: {subscription_id})")
            return True

        except Exception as e:
            logger.error(f"Unsubscribe failed: {e}")
            return False

    async def _resubscribe_all(self):
        """Resubscribe to all previous subscriptions after reconnection.

        Reuses the existing LOCAL ids (re-sending the request) so the registry
        does not duplicate entries on every reconnect; the server issues a new
        server id, which the confirmation handler maps to the same local id.
        """
        with self._state_lock:
            subscriptions = list(self._subscriptions.items())
        for subscription_id, subscription in subscriptions:
            try:
                request = self._build_subscription_request(
                    subscription.subscription_type, subscription_id, subscription.filters
                )
                await self._websocket.send(json.dumps(request))
                with self._state_lock:
                    self._pending_subs[subscription_id] = subscription_id
                self._stats.messages_sent += 1
                logger.debug(f"Resubscribed to {subscription.subscription_type.value} (ID: {subscription_id})")
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

        # A disconnect()/shutdown() during the backoff must abort the
        # reconnection instead of resurrecting a closed connection
        if not self._should_reconnect:
            return

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
        """Get all active subscriptions (thread-safe snapshot)."""
        with self._state_lock:
            return dict(self._subscriptions)

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

        with self._state_lock:
            active_subs = list(self._subscriptions.items())
        print(f"\nActive Subscriptions: {len(active_subs)}")
        for sub_id, sub in active_subs:
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
    """Reset the global WebSocket client (mainly for testing).

    NOTE: use ``await shutdown_websocket_client()`` for proper async cleanup;
    this sync variant only drops the reference.
    """
    global _client

    with _client_lock:
        _client = None


async def shutdown_websocket_client():
    """Shut the global client down cleanly (closes socket, cancels tasks)."""
    global _client

    with _client_lock:
        client = _client
        _client = None

    if client is not None:
        try:
            await client.shutdown()
        except Exception as e:
            logger.warning(f"WebSocket shutdown error: {e}")


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
